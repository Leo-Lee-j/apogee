//! Desktop update boundary: callers never provide executable paths, URLs or hashes.
use apogee_core::update::{self as service, CancellationToken, Release};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager};

#[derive(Default)]
struct Inner {
    operation: Option<(String, CancellationToken)>,
    release: Option<Release>,
    downloaded: Option<Downloaded>,
}
#[derive(Default)]
pub struct UpdateState(Mutex<Inner>);
struct BusyGuard {
    state: Arc<UpdateState>,
    id: String,
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        let mut inner = self.state.0.lock().unwrap_or_else(|p| p.into_inner());
        if inner
            .operation
            .as_ref()
            .is_some_and(|(id, _)| id == &self.id)
        {
            inner.operation = None;
        }
    }
}
impl UpdateState {
    fn begin(self: &Arc<Self>, id: String) -> Result<(BusyGuard, CancellationToken), String> {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if inner.operation.is_some() {
            return Err("UPDATE_BUSY: 更新操作正在进行，请稍后重试".into());
        }
        let cancel = CancellationToken::new();
        inner.operation = Some((id.clone(), cancel.clone()));
        Ok((
            BusyGuard {
                state: self.clone(),
                id,
            },
            cancel,
        ))
    }
    fn cancel(&self, id: &str) -> bool {
        let inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((active, cancel)) = &inner.operation {
            if active == id {
                cancel.cancel();
                return true;
            }
        }
        false
    }
}
#[derive(Clone)]
struct Downloaded {
    path: PathBuf,
    release: Release,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    current_version: String,
    latest: Option<Release>,
    update_available: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResult {
    tag: String,
    filename: String,
    path: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    operation_id: String,
    downloaded: u64,
    total: u64,
}
fn supported() -> Result<(), String> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok(())
    } else {
        Err("UPDATE_UNSUPPORTED: 官方更新目前仅支持 Windows x64".into())
    }
}

#[tauri::command]
pub fn update_current_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
pub async fn update_check(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<UpdateState>>,
    include_prerelease: bool,
) -> Result<CheckResult, String> {
    supported()?;
    let (_guard, _) = state.begin(uuid::Uuid::new_v4().to_string())?;
    // A failed new check must not leave an old downloadable candidate behind.
    {
        let mut inner = state.inner().0.lock().unwrap_or_else(|p| p.into_inner());
        inner.release = None;
    }
    let latest = service::check(include_prerelease)
        .await
        .map_err(|e| e.to_string())?;
    let current_version = app.package_info().version.to_string();
    let update_available = latest
        .as_ref()
        .map(|r| service::is_newer(r, &current_version))
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or(false);
    state
        .inner()
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .release = if update_available {
        latest.clone()
    } else {
        None
    };
    Ok(CheckResult {
        current_version,
        latest,
        update_available,
    })
}

fn cache_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let root = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let cache = root.join("updates");
    std::fs::create_dir_all(&cache).map_err(|_| "无法创建更新缓存目录".to_string())?;
    let metadata =
        std::fs::symlink_metadata(&cache).map_err(|_| "更新缓存目录不可用".to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("更新缓存目录不是普通目录".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("更新缓存目录不能是链接或重解析点".into());
        }
    }
    Ok(cache)
}
fn disk_space(cache: &Path, required: u64) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let path: Vec<u16> = cache.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free = 0_u64;
        let success = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                path.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if success == 0 {
            return Err("无法检查更新缓存所在磁盘的剩余空间".into());
        }
        if free < required + 32 * 1024 * 1024 {
            return Err("磁盘空间不足，无法下载安装包".into());
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (cache, required);
    }
    Ok(())
}
#[tauri::command]
pub async fn update_download(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<UpdateState>>,
    operation_id: String,
    tag: String,
) -> Result<DownloadResult, String> {
    supported()?;
    uuid::Uuid::parse_str(&operation_id).map_err(|_| "更新操作 ID 无效".to_string())?;
    let (_guard, cancel) = state.begin(operation_id.clone())?;
    let release = state
        .inner()
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .release
        .clone()
        .filter(|r| r.tag == tag)
        .ok_or("请先重新检查更新，下载标签已失效")?;
    let cache = cache_dir(&app)?;
    disk_space(&cache, release.size)?;
    // Registration precedes the first progress event; cancellation is enabled after this event.
    let report = |downloaded| {
        let _ = app.emit(
            "update:progress",
            Progress {
                operation_id: operation_id.clone(),
                downloaded,
                total: release.size,
            },
        );
    };
    report(0);
    let path = service::download(&release, &cache, &cancel, report)
        .await
        .map_err(|e| e.to_string())?;
    let result = DownloadResult {
        tag: release.tag.clone(),
        filename: release.filename.clone(),
        path: path.to_string_lossy().into_owned(),
    };
    state
        .inner()
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .downloaded = Some(Downloaded { path, release });
    Ok(result)
}
#[tauri::command]
pub fn update_cancel(state: tauri::State<'_, Arc<UpdateState>>, operation_id: String) -> bool {
    state.cancel(&operation_id)
}

#[tauri::command]
pub async fn update_open_download(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<UpdateState>>,
    tag: String,
    reveal: bool,
) -> Result<(), String> {
    supported()?;
    let (_guard, _) = state.begin(uuid::Uuid::new_v4().to_string())?;
    let downloaded = state
        .inner()
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .downloaded
        .clone()
        .filter(|d| d.release.tag == tag)
        .ok_or("没有可用的已下载安装包，请重新下载")?;
    // Re-hash immediately before opening; only the backend's own exact cache file is usable.
    let cache = cache_dir(&app)?;
    if downloaded.path != cache.join(&downloaded.release.filename) {
        return Err("安装包缓存路径不匹配".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        service::verify_package(&downloaded.path, &downloaded.release)
            .map_err(|e| format!("UPDATE_PACKAGE_INVALID: {e}"))?;
        open_package(&downloaded.path, reveal)
    })
    .await
    .map_err(|_| "打开安装包任务失败".to_string())?
}
fn open_package(path: &Path, reveal: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if reveal {
            std::process::Command::new("explorer.exe")
                .arg("/select,")
                .arg(path)
                .spawn()
                .map_err(|_| "无法在文件夹中显示安装包".to_string())?;
        } else {
            // ShellExecute supports an installer's UAC elevation; never start cmd/PowerShell.
            let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
            let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let result = unsafe {
                windows_sys::Win32::UI::Shell::ShellExecuteW(
                    std::ptr::null_mut(),
                    verb.as_ptr(),
                    file.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    1,
                )
            };
            if result as isize <= 32 {
                return Err("无法启动安装包，可能已取消授权；可在文件夹中手动安装".into());
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (path, reveal);
        Err("安装入口仅支持 Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operations_are_serialized_and_cancel_is_scoped() {
        let state = Arc::new(UpdateState::default());
        let (guard, token) = state.begin("a".into()).unwrap();
        assert!(state.begin("b".into()).is_err());
        assert!(!state.cancel("b"));
        assert!(!token.is_cancelled());
        assert!(state.cancel("a"));
        assert!(token.is_cancelled());
        drop(guard);
        assert!(state.begin("b".into()).is_ok());
    }
}
