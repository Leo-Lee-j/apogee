//! 工作区持久化的共享安全基础。
//!
//! 这里的原子发布只承诺进程级的“旧完整文件或新完整文件”可见性；不同文件系统、
//! 平台和断电时机的耐久性差异不在此处夸大为“永不丢失”。多文件可恢复语义由
//! workspace 层的 journal/staging 协议建立。

use anyhow::{bail, Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use uuid::Uuid;

const WORKSPACE_LOCK_FILE: &str = ".apogee.lock";

/// 同一 workspace session 内的短提交串行器。
///
/// 它不用于包住网络、解析、长时间 staging 或 UI 事件；调用方只能在最终读改写和
/// 发布窗口持有 guard。mutex 中毒不会让用户工作区永久无法打开，后续调用会接管
/// guard 并继续通过正常的 `Result` 路径报告文件系统错误。
#[derive(Default)]
pub(crate) struct MutationGate {
    inner: Mutex<()>,
    /// 测试仅在真实观察到竞争后通知控制器，用于构造确定性竞争窗口。
    #[cfg(test)]
    contention_hook: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl MutationGate {
    #[cfg(test)]
    pub(crate) fn is_locked(&self) -> bool {
        self.inner.try_lock().is_err()
    }

    #[cfg(test)]
    pub(crate) fn set_contention_hook(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self
            .contention_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn clear_contention_hook(&self) {
        *self
            .contention_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    #[cfg(test)]
    fn notify_if_contended(&self) {
        if !matches!(
            self.inner.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ) {
            return;
        }
        let hook = self
            .contention_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(hook) = hook.as_ref() {
            hook();
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, ()> {
        #[cfg(test)]
        self.notify_if_contended();
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 同一 workspace session 内多文件读取与提交的一致性边界。
///
/// 读取持共享 guard，提交和 recovery 持独占 guard，因此读取者只能观察事务前或
/// 事务后的完整状态。锁中毒不会阻止后续访问，调用方会接管 guard 并走正常错误路径。
#[derive(Default)]
pub(crate) struct WorkspaceAccessGate {
    inner: RwLock<()>,
    /// 测试仅在写入方确认被活动读取阻塞后通知控制器。
    #[cfg(test)]
    write_contention_hook: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl WorkspaceAccessGate {
    pub(crate) fn read(&self) -> RwLockReadGuard<'_, ()> {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn set_write_contention_hook(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self
            .write_contention_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn clear_write_contention_hook(&self) {
        *self
            .write_contention_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    #[cfg(test)]
    fn notify_if_write_contended(&self) {
        if !matches!(
            self.inner.try_write(),
            Err(std::sync::TryLockError::WouldBlock)
        ) {
            return;
        }
        let hook = self
            .write_contention_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(hook) = hook.as_ref() {
            hook();
        }
    }

    pub(crate) fn write(&self) -> RwLockWriteGuard<'_, ()> {
        #[cfg(test)]
        self.notify_if_write_contended();
        self.inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 对一个 canonical workspace root 的 OS advisory exclusive lock。
///
/// 文件句柄在 session 内保持打开；关闭最后一个持有它的 `Arc` 会由操作系统释放锁。
/// 锁文件本身不是占用状态的依据，因此遗留的 `.apogee.lock` 空文件不会阻止后续打开。
#[derive(Debug)]
pub(crate) struct WorkspaceWriteLock {
    root: PathBuf,
    _file: File,
}

impl WorkspaceWriteLock {
    pub(crate) fn canonical_root(root: &Path) -> Result<PathBuf> {
        root.canonicalize()
            .with_context(|| format!("无法解析工作区路径：{}", root.display()))
    }

    #[cfg(test)]
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        Self::acquire_canonical(Self::canonical_root(root)?)
    }

    pub(crate) fn acquire_canonical(root: PathBuf) -> Result<Self> {
        let path = root.join(WORKSPACE_LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            // 锁文件只承载 OS lock；不得因再次打开而清空任何既有内容。
            .truncate(false)
            .open(&path)
            .with_context(|| format!("无法打开工作区写锁文件：{}", path.display()))?;

        if let Err(error) = try_lock_exclusive(&file) {
            if is_lock_contention(&error) {
                bail!("WORKSPACE_LOCKED");
            }
            return Err(error).with_context(|| format!("无法获取工作区写锁：{}", path.display()));
        }

        Ok(Self { root, _file: file })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    // SAFETY: `OVERLAPPED` is zero-initialized for a synchronous whole-file lock, and the raw
    // handle comes from a live `std::fs::File` retained by `WorkspaceWriteLock` on success.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let locked = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if locked == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    unsafe extern "C" {
        fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
    }

    const LOCK_EX: std::ffi::c_int = 2;
    const LOCK_NB: std::ffi::c_int = 4;
    // SAFETY: `file` owns a valid descriptor for the entire call. `flock` neither takes
    // ownership nor writes through Rust aliases; it only changes the kernel lock state.
    let result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(windows, unix)))]
fn try_lock_exclusive(_file: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "当前平台不支持工作区 advisory lock",
    ))
}

fn is_lock_contention(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(windows)]
    {
        // ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION.
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(unix)]
    {
        // EAGAIN / EWOULDBLOCK. `ErrorKind::WouldBlock` covers supported targets, but retain
        // the raw codes for libc implementations that do not map them consistently.
        matches!(error.raw_os_error(), Some(11 | 35))
    }
    #[cfg(not(any(windows, unix)))]
    {
        false
    }
}

/// 将完整字节写到同目录、唯一的临时文件，sync 后使用平台 replace 原语发布。
///
/// 临时文件必须与 target 同目录，保证 `rename`/`ReplaceFileW` 不会退化成跨卷复制。
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with_replace(path, bytes, atomic_replace)
}

/// 为确定性故障测试保留发布边界；生产代码使用 [`atomic_replace`]。
pub(crate) fn atomic_write_with_replace(
    path: &Path,
    bytes: &[u8],
    replace: impl FnOnce(&Path, &Path) -> Result<()>,
) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("目标文件没有父目录：{}", path.display()))?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("apogee-data");
    let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));

    let write_result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        // Close the handle before ReplaceFileW/MoveFileExW. This is required on Windows and
        // makes the publish point explicit on every platform.
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        return cleanup_temporary_after_failure(&temporary, error.into(), "写入并同步临时文件失败");
    }

    if let Err(error) = replace(&temporary, path) {
        return cleanup_temporary_after_failure(&temporary, error, "发布临时文件失败");
    }

    Ok(())
}

pub(crate) fn atomic_replace(temporary: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, ReplaceFileW};

        let from = windows_wide_path(temporary);
        let to = windows_wide_path(target);

        if target.exists() {
            let ok = unsafe {
                ReplaceFileW(
                    to.as_ptr(),
                    from.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            if ok == 0 {
                let replace_error = io::Error::last_os_error();
                bail!("无法原子替换文件：{}（{replace_error}）", target.display());
            }
            return Ok(());
        }

        let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) };
        if ok == 0 {
            let move_error = io::Error::last_os_error();
            bail!("无法发布新文件：{}（{move_error}）", target.display());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(temporary, target)?;
        Ok(())
    }
}

/// 将绝对 Windows 路径转换为 Win32 长路径形式，供直接调用宽字符文件 API 使用。
/// 相对路径维持原样，以保留调用者显式选择的当前目录语义。
#[cfg(windows)]
fn windows_wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    const BACKSLASH: u16 = b'\\' as u16;
    const SLASH: u16 = b'/' as u16;
    const QUESTION: u16 = b'?' as u16;
    const UPPER_U: u16 = b'U' as u16;
    const UPPER_N: u16 = b'N' as u16;
    const UPPER_C: u16 = b'C' as u16;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if path.is_absolute() {
        for character in &mut wide {
            if *character == SLASH {
                *character = BACKSLASH;
            }
        }
    }

    let verbatim_prefix = [BACKSLASH, BACKSLASH, QUESTION, BACKSLASH];
    if path.is_absolute() && !wide.starts_with(&verbatim_prefix) {
        let mut extended = if wide.starts_with(&[BACKSLASH, BACKSLASH]) {
            vec![
                BACKSLASH, BACKSLASH, QUESTION, BACKSLASH, UPPER_U, UPPER_N, UPPER_C, BACKSLASH,
            ]
        } else {
            vec![BACKSLASH, BACKSLASH, QUESTION, BACKSLASH]
        };
        extended.extend_from_slice(if wide.starts_with(&[BACKSLASH, BACKSLASH]) {
            &wide[2..]
        } else {
            &wide
        });
        wide = extended;
    }

    wide.push(0);
    wide
}

fn cleanup_temporary_after_failure(
    temporary: &Path,
    error: anyhow::Error,
    action: &str,
) -> Result<()> {
    match fs::remove_file(temporary) {
        Ok(()) => {}
        Err(cleanup_error) if cleanup_error.kind() == io::ErrorKind::NotFound => {}
        Err(cleanup_error) => {
            return Err(error.context(format!(
                "{action}，且无法清理临时文件 {}：{cleanup_error}",
                temporary.display()
            )));
        }
    }
    Err(error.context(format!("{action}：{}", temporary.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_workspace_lock_rejects_second_handle_and_releases_on_drop() {
        let root =
            std::env::temp_dir().join(format!("apogee-workspace-lock-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();

        let first = WorkspaceWriteLock::acquire(&root).unwrap();
        let error = WorkspaceWriteLock::acquire(&root).unwrap_err();
        assert_eq!(error.to_string(), "WORKSPACE_LOCKED");
        drop(first);

        let second = WorkspaceWriteLock::acquire(&root).unwrap();
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_publish_preserves_target_and_cleans_temporary_file() {
        let root =
            std::env::temp_dir().join(format!("apogee-atomic-write-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("state.json");
        fs::write(&target, b"old").unwrap();

        let error =
            atomic_write_with_replace(&target, b"new", |_, _| bail!("injected publish failure"))
                .unwrap_err();

        assert!(format!("{error:#}").contains("injected publish failure"));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
        fs::remove_dir_all(root).unwrap();
    }
}
