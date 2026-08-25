//! Apogee 桌面端入口。
//!
//! # 本文件承载的两个架构关键点
//!
//! 1. **`apogee://` 自定义协议 + Range**（03 文档 §3.1）
//!    大响应体绝不走 `invoke`。Tauri 的 `invoke` 是 JSON 序列化通道，10MB body
//!    过一次意味着「Rust 序列化 → 字符串传输 → JS 解析」三次全量拷贝。
//!    自定义协议让 WebView 自己按字节区间取，前端只拿首屏那一段。
//!
//! 2. **分阶段启动 + 首帧后显示窗口**（03 文档 §4.1）
//!    窗口以 `visible: false` 创建，前端首帧就绪后调 `app_ready` 再 show，
//!    避免白屏闪烁。带兜底超时，防止前端异常导致窗口永远不出现。

mod app_settings;
mod mem;
mod persistence;
mod protocol;
mod state;
mod workspace;
mod workspace_registry;
mod workspace_state;

use state::AppState;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

fn app_data_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    if let Some(path) = std::env::var_os("APOGEE_DATA_DIR") {
        return Ok(std::path::PathBuf::from(path));
    }
    app.path().app_data_dir().map_err(|error| error.to_string())
}

pub fn run() {
    install_panic_hook();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,apogee_desktop_lib=debug".into()),
        )
        .init();

    let mut context = tauri::generate_context!();
    apply_webview_runtime_settings(&mut context);

    let state = Arc::new(AppState::new().expect("初始化应用状态失败"));
    let proto_state = state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        // 注册 apogee:// 协议。异步版本很重要：同步 handler 会阻塞 WebView 线程，
        // 100MB 文件的 IO 卡在那里就等于界面卡死。
        //
        // ⚠️ scheme 名有三处必须一致：这里、tauri.conf.json 的 app.security.csp
        // （含 Windows 的 http://<scheme>.localhost 形态）、以及 src/ipc/index.ts 的
        // responseUrl()。漏掉任一处的表现都是 `TypeError: Failed to fetch`
        // ——被 CSP 静默拦掉，不报 CSP 错误，很难定位。见 index.html 的注释。
        .register_asynchronous_uri_scheme_protocol("apogee", move |_ctx, request, responder| {
            let st = proto_state.clone();
            std::thread::spawn(move || {
                responder.respond(protocol::handle(&st, &request));
            });
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_ready,
            commands::app_settings_load,
            commands::app_settings_save,
            commands::system_font_families,
            commands::app_restart,
            commands::spike_make_response,
            commands::spike_index_response,
            commands::spike_get_response_tree,
            commands::get_response_meta,
            commands::save_response,
            commands::index_response,
            commands::get_response_tree,
            commands::process_memory,
            commands::app_memory,
            commands::spike_report,
            commands::spike_autorun,
            commands::send_request,
            commands::cancel_request,
            commands::workspace_list,
            commands::workspace_create,
            commands::workspace_open,
            commands::workspace_close,
            commands::workspace_forget,
            commands::workspace_relocate,
            commands::workspace_reveal,
            commands::workspace_load,
            commands::workspace_read_request,
            commands::workspace_save_request,
            commands::workspace_list_debug_cases,
            commands::workspace_create_debug_case,
            commands::workspace_delete_debug_case,
            commands::workspace_read_container,
            commands::workspace_save_container,
            commands::workspace_create_collection,
            commands::workspace_create_folder,
            commands::workspace_create_request,
            commands::workspace_rename_node,
            commands::workspace_delete_node,
            commands::workspace_create_environment,
            commands::workspace_save_environment,
            commands::workspace_delete_environment,
            commands::workspace_save_state,
            commands::history_record,
            commands::history_record_error,
            commands::history_list,
            commands::history_read,
            commands::history_clear,
            commands::curl_import,
            commands::postman_import,
            commands::postman_import_file,
            commands::postman_import_workspace,
            commands::postman_import_environment_workspace,
            commands::yapi_analyze_import,
            commands::yapi_apply_import,
            commands::yapi_discard_import,
        ])
        .setup(move |app| {
            let data_dir = app_data_dir(app.handle()).map_err(std::io::Error::other)?;
            std::fs::create_dir_all(&data_dir)?;

            let settings = app_settings::AppSettingsRepository::new(&data_dir).load()?;
            let registry = workspace_registry::WorkspaceRegistry::new(&data_dir);
            if registry.list()?.is_empty() {
                let legacy_root = data_dir.join("workspace");
                if legacy_root.join("apogee.yaml").is_file() {
                    let legacy = workspace::WorkspaceRepository::open(&legacy_root)?;
                    registry.register(&legacy)?;
                }
                let example_root = std::path::PathBuf::from(&settings.default_workspace_root)
                    .join("example-workspace");
                if !example_root.exists() {
                    let example = workspace::WorkspaceRepository::create_example(example_root)?;
                    registry.register(&example)?;
                }
            }

            // 兜底：无论前端是否报告就绪，3 秒后强制显示窗口。
            // 没有这层兜底，一个前端异常就会变成「双击图标什么都不出现」，
            // 那是最难排查的一类用户报障。
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(3));
                if let Some(w) = handle.get_webview_window("main") {
                    if !w.is_visible().unwrap_or(true) {
                        tracing::warn!("前端未在 3s 内报告就绪，兜底显示窗口");
                        let _ = w.show();
                    }
                }
            });
            Ok(())
        })
        .run(context)
        .expect("Tauri 应用启动失败");
}

/// Wry 在创建 WebView2 环境时将该配置直接交给 Runtime，故必须在 Tauri
/// 创建窗口前写入 Context；进程环境变量不会可靠地覆盖这条路径。
fn apply_webview_runtime_settings(context: &mut tauri::Context<tauri::Wry>) {
    #[cfg(windows)]
    {
        // Wry 的 custom additionalBrowserArgs 会替代默认参数；无论是性能量测
        // 还是 E2E，都先保留这些安全/行为默认值再追加测试所需开关。
        const DEFAULT_BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required";

        // 只允许受控的本地 E2E 进程显式开启 CDP。该环境变量未设置时，发行版
        // 不会监听调试端口；端口解析失败或为 0 同样视为未启用。
        if let Some(port) = std::env::var("APOGEE_E2E_CDP_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port != 0)
        {
            let args = format!(
                "{DEFAULT_BROWSER_ARGS} --remote-debugging-port={port} --remote-allow-origins=*"
            );
            for window in &mut context.config_mut().app.windows {
                window.additional_browser_args = Some(args.clone());
            }
            return;
        }

        // 性能量测台显式控制 GPU 开关，不能被用户偏好污染。
        if std::env::var_os("APOGEE_SPIKE_AUTORUN").is_some() {
            return;
        }
        let Some(app_data) = std::env::var_os("APPDATA") else {
            return;
        };
        let settings_path = std::path::PathBuf::from(app_data)
            .join(&context.config().identifier)
            .join("settings.json");
        let Ok(contents) = std::fs::read(&settings_path) else {
            return;
        };
        let Ok(settings) = serde_json::from_slice::<app_settings::AppSettings>(&contents) else {
            return;
        };
        if settings.low_memory_mode {
            let args = format!("{DEFAULT_BROWSER_ARGS} --disable-gpu --disable-gpu-compositing");
            for window in &mut context.config_mut().app.windows {
                window.additional_browser_args = Some(args.clone());
            }
        }
    }
}

/// 记录不含运行时内容的 panic 信号。
///
/// panic payload 可能来自格式化的请求、响应或底层错误链，不能把它写入普通日志。
/// 默认只保留发生过 panic 的事实；需要可审计的 reveal 机制前，不回显原始 payload。
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|_| {
        eprintln!("[apogee panic] unexpected panic");
        tracing::error!("PANIC");
    }));
}

mod commands {
    use super::state::AppState;
    use crate::app_settings::{AppSettings, AppSettingsRepository};
    use crate::workspace::WorkspaceRepository;
    use crate::workspace::{HistoryEntry, HistoryRecord};
    use crate::workspace_registry::{WorkspaceDescriptor, WorkspaceRegistry};
    use apogee_core::error::AppError;
    use apogee_core::public::{
        PublicActualRequest, PublicError, PublicRequestPreview, PublicResponseSummary,
    };
    use apogee_core::request::RequestSpec;
    use apogee_core::response::TreeNode;
    use apogee_core::vars::Environment;
    use apogee_core::workspace::{DebugCase, DebugCaseType, WorkspaceRequest, WorkspaceSnapshot};
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tauri::{Emitter, Manager, State};
    use uuid::Uuid;

    #[derive(Debug, Clone, Copy, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct BootMetrics {
        mount_ms: f64,
        ready_ms: f64,
        first_paint_ms: f64,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct WorkspaceOpenResult {
        descriptor: WorkspaceDescriptor,
        workspace_id: Uuid,
        session_id: Uuid,
        snapshot: WorkspaceSnapshot,
        state: crate::workspace_state::WorkspaceState,
    }

    /// 已将原始响应体完整复制到用户通过原生对话框选择的位置。
    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SaveResponseResult {
        bytes_written: u64,
    }

    #[tauri::command]
    pub fn app_settings_load(app: tauri::AppHandle) -> Result<AppSettings, String> {
        let dir = super::app_data_dir(&app)?;
        AppSettingsRepository::new(&dir)
            .load()
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub fn app_settings_save(
        app: tauri::AppHandle,
        settings: AppSettings,
    ) -> Result<AppSettings, String> {
        let dir = super::app_data_dir(&app)?;
        let repo = AppSettingsRepository::new(&dir);
        repo.save(&settings).map_err(|e| e.to_string())?;
        repo.load().map_err(|e| e.to_string())
    }

    /// 返回当前操作系统已安装字体的家族名称。字体文件扫描可能较慢，因此只在用户
    /// 打开设置页时由前端调用，而不是在应用启动路径中执行。
    #[tauri::command]
    pub fn system_font_families() -> Vec<String> {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();

        let mut families = database
            .faces()
            .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
            .collect::<Vec<_>>();
        families.sort_unstable_by_key(|name| name.to_lowercase());
        families.dedup();
        families
    }

    #[tauri::command]
    pub fn workspace_list(app: tauri::AppHandle) -> Result<Vec<WorkspaceDescriptor>, String> {
        registry(&app)?.list().map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_create(
        app: tauri::AppHandle,
        state: State<'_, Arc<AppState>>,
        path: Option<String>,
        name: String,
    ) -> Result<WorkspaceOpenResult, String> {
        let root = match path.filter(|value| !value.trim().is_empty()) {
            Some(path) => PathBuf::from(path),
            None => {
                let data_dir = super::app_data_dir(&app)?;
                let settings = AppSettingsRepository::new(&data_dir)
                    .load()
                    .map_err(|e| e.to_string())?;
                let slug: String = name
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let slug = if slug.trim_matches('_').is_empty() {
                    "workspace".to_string()
                } else {
                    slug.trim_matches('_').to_string()
                };
                let base = slug;
                let mut candidate = PathBuf::from(&settings.default_workspace_root).join(&base);
                let mut suffix = 2;
                while candidate.exists() {
                    candidate = PathBuf::from(&settings.default_workspace_root)
                        .join(format!("{base}-{suffix}"));
                    suffix += 1;
                }
                candidate
            }
        };
        let repository =
            WorkspaceRepository::create(root, name).map_err(|error| error.to_string())?;
        open_repository(&app, &state, repository)
    }

    #[tauri::command]
    pub fn workspace_open(
        app: tauri::AppHandle,
        state: State<'_, Arc<AppState>>,
        workspace_id: Option<String>,
        path: Option<String>,
    ) -> Result<WorkspaceOpenResult, String> {
        let root = match (path, workspace_id) {
            (Some(path), _) => PathBuf::from(path),
            (None, Some(id)) => {
                let id = parse_id(&id)?;
                PathBuf::from(
                    registry(&app)?
                        .find(id)
                        .map_err(|error| error.to_string())?
                        .path,
                )
            }
            (None, None) => return Err("必须提供工作区 id 或路径".into()),
        };
        let repository = WorkspaceRepository::open(root).map_err(|error| error.to_string())?;
        open_repository(&app, &state, repository)
    }

    #[tauri::command]
    pub fn workspace_close(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
    ) -> Result<(), String> {
        state
            .close_workspace(parse_id(&workspace_id)?, parse_id(&session_id)?)
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_forget(app: tauri::AppHandle, workspace_id: String) -> Result<(), String> {
        registry(&app)?
            .forget(parse_id(&workspace_id)?)
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_relocate(
        app: tauri::AppHandle,
        workspace_id: String,
        path: String,
    ) -> Result<WorkspaceDescriptor, String> {
        registry(&app)?
            .relocate(parse_id(&workspace_id)?, path)
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_reveal(app: tauri::AppHandle, workspace_id: String) -> Result<(), String> {
        let descriptor = registry(&app)?
            .find(parse_id(&workspace_id)?)
            .map_err(|error| error.to_string())?;
        let path = std::path::PathBuf::from(&descriptor.path);
        if !path.exists() {
            return Err("工作区目录不存在".into());
        }
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("explorer.exe")
                .arg(format!("/select,{}", path.display()))
                .spawn()
                .map_err(|error| error.to_string())?;
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg("-R")
                .arg(&path)
                .spawn()
                .map_err(|error| error.to_string())?;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::process::Command::new("xdg-open")
                .arg(&path)
                .spawn()
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn open_repository(
        app: &tauri::AppHandle,
        state: &State<'_, Arc<AppState>>,
        repository: WorkspaceRepository,
    ) -> Result<WorkspaceOpenResult, String> {
        // 打开前的 recovery、snapshot、注册表与 runtime migration 必须是同一个
        // open/close transition 的一部分；同根 reload 会先等待旧 mutation 完成，避免
        // recovery 观察到 journal/staging/tombstone 的活动中间状态。
        let root = repository.root().to_path_buf();
        state
            .with_workspace_open_transition(&root, |write_lock| {
                repository.recover_pending_transactions()?;
                let snapshot = repository.snapshot()?;
                let descriptor = registry(app)
                    .map_err(anyhow::Error::msg)?
                    .register(&repository)
                    .map_err(anyhow::Error::msg)?;
                let data_dir = super::app_data_dir(app).map_err(anyhow::Error::msg)?;
                let mut workspace_state =
                    crate::workspace_state::WorkspaceStateRepository::new(&data_dir)
                        .load(snapshot.id)?;
                if !workspace_state.active_environment_set {
                    workspace_state.active_environment_id = snapshot
                        .environments
                        .first()
                        .map(|environment| environment.id);
                } else if workspace_state.active_environment_id.is_some_and(|id| {
                    !snapshot
                        .environments
                        .iter()
                        .any(|environment| environment.id == id)
                }) {
                    workspace_state.active_environment_id = None;
                }
                let runtime_root = data_dir.join("runtime").join(snapshot.id.to_string());
                let repository = repository.with_runtime_root(&runtime_root)?;
                let (workspace_id, session_id) =
                    state.open_workspace_with_write_lock(repository, &runtime_root, write_lock)?;
                Ok(WorkspaceOpenResult {
                    descriptor,
                    workspace_id,
                    session_id,
                    snapshot,
                    state: workspace_state,
                })
            })
            .map_err(|error| error.to_string())
    }

    fn registry(app: &tauri::AppHandle) -> Result<WorkspaceRegistry, String> {
        let data_dir = super::app_data_dir(app)?;
        Ok(WorkspaceRegistry::new(&data_dir))
    }

    /// 前端首帧就绪 → 显示窗口（03 文档 §4.1 分阶段启动）。
    #[tauri::command]
    pub fn app_ready(window: tauri::Window, metrics: Option<BootMetrics>) -> Result<(), String> {
        if let Some(w) = window.get_webview_window("main") {
            w.show().map_err(|e| e.to_string())?;
        }
        write_s1_ready_marker(metrics.as_ref())?;
        Ok(())
    }

    /// S1 外部量测的跨进程就绪信号。正常运行时没有这个环境变量，因此零开销。
    fn write_s1_ready_marker(metrics: Option<&BootMetrics>) -> Result<(), String> {
        let Some(path) = std::env::var_os("APOGEE_S1_READY_FILE").map(PathBuf::from) else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
            * 1000.0;
        let payload = serde_json::json!({
            "processId": std::process::id(),
            "readyEpochMs": epoch_ms,
            "frontend": metrics,
        });
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(&payload).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        std::fs::rename(temporary, path).map_err(|error| error.to_string())
    }

    /// Spike S2：在 Rust 侧生成指定体积的合成 JSON 并流式落盘。
    ///
    /// 这不是产品功能，只在 Spike 面板里用。放在这里是因为量测必须覆盖
    /// 「生成 + 落盘」这一段，否则算不清首屏耗时到底花在哪。
    #[tauri::command]
    pub fn spike_make_response(
        state: State<'_, Arc<AppState>>,
        target_bytes: usize,
    ) -> Result<PublicResponseSummary, String> {
        state
            .make_synthetic(target_bytes)
            .map(|summary| PublicResponseSummary::from(&summary))
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub fn app_restart(app: tauri::AppHandle) -> Result<(), String> {
        app.request_restart();
        Ok(())
    }

    #[tauri::command]
    pub fn get_response_meta(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        response_id: String,
    ) -> Result<PublicResponseSummary, String> {
        let id = parse_id(&response_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .response_summary(workspace_id, session_id, id)
            .map(|summary| PublicResponseSummary::from(&summary))
            .map_err(|error| error.to_string())
    }

    /// 使用原生保存对话框将指定 response spool 直接复制到用户选定的位置。
    ///
    /// 目标路径不会经过 IPC，因此 WebView 无法把它变成任意文件写入能力。先在
    /// 短校验中确认 response 属于调用方的 workspace session，再由 AppState 在复制
    /// 前重新验证并持有旧 runtime lease；关闭/切换工作区不会读到新 session 的文件。
    #[tauri::command]
    pub async fn save_response(
        app: tauri::AppHandle,
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        response_id: String,
        suggested_name: String,
    ) -> Result<Option<SaveResponseResult>, String> {
        let response_id = parse_id(&response_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        let state = state.inner().clone();
        let file_name = safe_save_file_name(&suggested_name, response_id);

        tokio::task::spawn_blocking(move || {
            use tauri_plugin_dialog::{DialogExt, FilePath};

            let summary = state
                .response_summary(workspace_id, session_id, response_id)
                .map_err(|error| error.to_string())?;
            if summary.truncated {
                return Err("响应体已达到缓存上限并被截断，不能另存为完整文件".to_string());
            }

            let Some(selection) = app
                .dialog()
                .file()
                .set_title("另存响应体")
                .set_file_name(file_name)
                .blocking_save_file()
            else {
                return Ok(None);
            };
            let destination = match selection {
                FilePath::Path(path) => path,
                FilePath::Url(url) => url
                    .to_file_path()
                    .map_err(|_| "当前平台返回的保存位置不是本地文件路径".to_string())?,
            };
            let bytes_written = state
                .copy_response_to(workspace_id, session_id, response_id, &destination)
                .map_err(|error| error.to_string())?;
            Ok(Some(SaveResponseResult { bytes_written }))
        })
        .await
        .map_err(|error| format!("等待保存响应体任务失败：{error}"))?
    }

    /// 显式建 JSON 索引。
    ///
    /// 大 body 下这一步是**显式的**：02 文档 §4.4 规定 2MB 以上树视图需要
    /// 用户手动点「解析」，不自动做。所以命令层也不隐式触发。
    #[tauri::command]
    pub fn index_response(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        response_id: String,
    ) -> Result<PublicResponseSummary, String> {
        let id = parse_id(&response_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .index_response(workspace_id, session_id, id)
            .map(|summary| PublicResponseSummary::from(&summary))
            .map_err(|error| format!("JSON 索引失败：{error}"))
    }

    /// 取一页树节点。前端拿到的是扁平数组，不做递归构造（03 文档 §4.4）。
    #[tauri::command]
    pub fn get_response_tree(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        response_id: String,
        path: String,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TreeNode>, String> {
        let id = parse_id(&response_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .response_tree_page(workspace_id, session_id, id, &path, offset, limit)
            .map_err(|error| error.to_string())
    }

    /// Spike 使用显式的 Synthetic response scope；无论是否打开工作区，都不能通过
    /// 此命令读取普通响应。
    #[tauri::command]
    pub fn spike_index_response(
        state: State<'_, Arc<AppState>>,
        response_id: String,
    ) -> Result<PublicResponseSummary, String> {
        let id = parse_id(&response_id)?;
        state
            .index_synthetic_response(id)
            .map(|summary| PublicResponseSummary::from(&summary))
            .map_err(|error| format!("JSON 索引失败：{error}"))
    }

    #[tauri::command]
    pub fn spike_get_response_tree(
        state: State<'_, Arc<AppState>>,
        response_id: String,
        path: String,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TreeNode>, String> {
        let id = parse_id(&response_id)?;
        state
            .synthetic_tree_page(id, &path, offset, limit)
            .map_err(|error| error.to_string())
    }

    /// 当前进程内存。供「性能自检」面板与性能基准使用（PRD §5.2）。
    #[tauri::command]
    pub fn process_memory() -> super::mem::MemInfo {
        super::mem::current()
    }

    /// 当前 Apogee 进程树的内存总量与分类明细，供状态栏内存监测组件使用。
    #[tauri::command]
    pub fn app_memory() -> super::mem::AppMemInfo {
        super::mem::current_app()
    }

    /// 把 Spike 量测结果落盘。前端没有文件系统权限（能力最小化），所以走这里。
    #[tauri::command]
    pub fn spike_report(name: String, json: String) -> Result<String, String> {
        // 防目录穿越：只允许简单名字
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("非法的报告名".into());
        }
        let dir = super::state::spike_results_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let p = dir.join(format!("{name}.json"));
        std::fs::write(&p, json).map_err(|e| e.to_string())?;
        Ok(p.display().to_string())
    }

    /// 该次启动要不要自动跑某个 Spike，由 `APOGEE_SPIKE_AUTORUN` 环境变量决定。
    ///
    /// 为什么不用 `import.meta.env.VITE_AUTORUN_*`（S2 的做法）：那是**构建期**开关，
    /// 前端产物被 `generate_context!` 嵌进 exe，换一次开关就要重链一遍 release
    /// （`lto="fat"` 下几分钟）。S3 要在同一个二进制上跑「GPU 开」「GPU 关」两种配置，
    /// 构建期开关会让量测台每换一次配置就重建一次，既慢又无法保证两轮测的是同一份代码。
    ///
    /// 环境变量不存在时返回 `None`，正常运行零行为差异。
    #[tauri::command]
    pub fn spike_autorun() -> Option<String> {
        std::env::var("APOGEE_SPIKE_AUTORUN")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
    }

    /// 发送一个 HTTP 请求（03 文档 §3.2 / §4.5）。
    ///
    /// 立即返回句柄，不等网络。请求在后台执行，进度与结果通过事件推送。
    #[tauri::command]
    pub async fn send_request(
        app: tauri::AppHandle,
        state: State<'_, Arc<AppState>>,
        mut spec: apogee_core::request::RequestSpec,
        ctx: Option<apogee_core::request::ExecContext>,
        workspace_id: String,
        session_id: String,
    ) -> Result<apogee_core::request::ExecHandle, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace(workspace_id, session_id, |_| Ok(()))
            .map_err(|error| error.to_string())?;
        if let Some(raw_id) = ctx
            .as_ref()
            .and_then(|item| item.request_node_id.as_deref())
        {
            let request_id =
                Uuid::parse_str(raw_id).map_err(|_| format!("非法的请求节点 id：{raw_id}"))?;
            spec = state
                .with_workspace(workspace_id, session_id, |repository| {
                    repository.resolve_request_inheritance(request_id, spec)
                })
                .map_err(|error| error.to_string())?;
        }
        // 应用级网络设置只作为默认值，显式配置在请求/集合层的字段优先。
        let data_dir = super::app_data_dir(&app)?;
        let settings = crate::app_settings::AppSettingsRepository::new(&data_dir)
            .load()
            .map_err(|error| error.to_string())?;
        if spec.settings.timeout_ms.is_none() {
            spec.settings.timeout_ms = Some(settings.timeout_ms);
        }
        if spec.settings.verify_tls.is_none() {
            spec.settings.verify_tls = Some(settings.verify_tls);
        }
        if spec.settings.proxy_url.is_none() {
            spec.settings.proxy_url = match settings.proxy_mode.as_str() {
                "manual" => settings.proxy_url.filter(|v| !v.trim().is_empty()),
                "none" => Some("direct://".to_string()),
                _ => None,
            };
        }
        let environment = match ctx.as_ref().and_then(|item| item.environment_id.as_deref()) {
            Some(raw_id) => {
                let environment_id =
                    Uuid::parse_str(raw_id).map_err(|_| format!("非法的环境 id：{raw_id}"))?;
                Some(
                    state
                        .with_workspace(workspace_id, session_id, |repository| {
                            repository.read_environment(environment_id)
                        })
                        .map_err(|error| error.to_string())?,
                )
            }
            None => None,
        };

        let id = Uuid::new_v4();
        let tab_id = ctx.as_ref().and_then(|item| item.tab_id.clone());
        let lease = state
            .begin_request(workspace_id, session_id, id, tab_id)
            .map_err(|error| error.to_string())?;
        let (runtime, cancel_rx, response_access) = lease.into_execution_parts();
        let state2 = state.inner().clone();
        let app2 = app.clone();

        // 正常的 `Result` 路径必须发出终结事件，不能依赖 panic 恢复来维持前端状态。
        //
        // 在支持 unwind 的开发/测试构建中，`FutureExt::catch_unwind` 会把未预期的 task
        // panic 转成诊断性 `request:error`。release 采用 `panic = "abort"`，进程会在
        // 任何 Rust 兜底逻辑前结束，因此这不是 release 下的可靠性承诺。
        tokio::spawn(async move {
            use futures_util::FutureExt;
            use std::panic::AssertUnwindSafe;

            let app_for_cleanup = app2.clone();
            let app_for_done = app2.clone();
            let app_for_progress = app2.clone();
            let state_for_progress = state2.clone();
            let id_for_cleanup = id;
            let event_workspace_id = response_access.workspace_id();
            let event_session_id = response_access.session_id();

            fn response_access_lost(error: &anyhow::Error) -> bool {
                matches!(
                    error.to_string().as_str(),
                    "当前没有打开的工作区"
                        | "WORKSPACE_SESSION_EXPIRED"
                        | "RESPONSE_SESSION_EXPIRED"
                        | "RESPONSE_NOT_FOUND"
                )
            }

            // 终结事件必须在仍有效的 workspace session 临界区内入队。若 close 已先
            // 发生，整个 emit 会被跳过；若 emit 已入队，payload 里的 session 仍让前端
            // 能在延迟送达且已切换工作区时安全丢弃它。
            let cleanup = |event: &str, payload: serde_json::Value| {
                let emitted = state2.with_active_response_access(response_access, || {
                    app_for_cleanup
                        .emit(event, payload)
                        .map_err(|error| anyhow::anyhow!(error))
                });
                state2.finish_inflight(id_for_cleanup);
                if let Err(error) = emitted {
                    if !response_access_lost(&error) {
                        tracing::debug!(
                            event,
                            operation = "emit_terminal_event",
                            "投递请求终结事件失败"
                        );
                    }
                }
            };

            // 进度事件必须节流。`execute` 每收到一个 chunk 就回调一次，而 hyper
            // 的 chunk 是 KB 级 —— 一个 100MB 响应会产生上万次回调。每次 emit 都是
            // 一次 JSON 序列化 + 一次跨进程投递,不节流等于用进度条把 IPC 通道堵死,
            // 正好打在 §4.3「大响应体不卡界面」这条要求上。
            //
            // 50ms 一次 = 最多 20 帧/秒的进度更新,人眼看不出与逐块更新的差别。
            // 不需要补发最后一次:终结值由 `request:done` 的 summary 带过去。
            const PROGRESS_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
            let mut last_progress: Option<std::time::Instant> = None;
            let on_progress = move |progress: apogee_core::http::Progress| {
                let now = std::time::Instant::now();
                if last_progress.is_some_and(|at| now.duration_since(at) < PROGRESS_MIN_INTERVAL) {
                    return;
                }
                last_progress = Some(now);
                let _ = state_for_progress.with_active_response_access(response_access, || {
                    app_for_progress
                        .emit(
                            "request:progress",
                            serde_json::json!({
                                "id": id,
                                "workspaceId": event_workspace_id,
                                "sessionId": event_session_id,
                                "received": progress.received,
                                "total": progress.total,
                            }),
                        )
                        .map_err(|error| anyhow::anyhow!(error))
                });
            };

            let body = AssertUnwindSafe(async {
                let outcome =
                    apogee_core::http::execute(
                        id,
                        &spec,
                        environment.as_ref(),
                        runtime.spool(),
                        cancel_rx,
                        on_progress,
                    )
                    .await;

                match outcome {
                    apogee_core::http::Outcome::Done(meta) => {
                        match state2.record_http_response(response_access, &meta) {
                            Ok(_) => {}
                            Err(error) if response_access_lost(&error) => {
                                // A 的网络任务可能在 close 后才结束。此时 response state 已经
                                // 随 session 清理，不能把它重新注册到 B，也不能向 B 发送终结事件。
                                state2.finish_inflight(id);
                                return;
                            }
                            Err(_error) => {
                                tracing::error!(operation = "record_http_response", "记录 HTTP 响应失败");
                                cleanup(
                                    "request:error",
                                    serde_json::json!({
                                        "id": id,
                                        "workspaceId": event_workspace_id,
                                        "sessionId": event_session_id,
                                        "error": PublicError::internal(),
                                        "actualRequest": PublicActualRequest::from(&meta.actual_request),
                                    }),
                                );
                                return;
                            }
                        }

                        // 2MB 以下顺手建 JSON 索引，让树视图不必再等一次往返
                        // （2MB 以上按 02 文档 §4.4 必须用户显式点「解析」）。
                        //
                        // 尝试是**尽力而为**的：非 JSON 响应（HTML、图片、纯文本）
                        // 必然解析失败,那不是故障。所以走 debug 而不是 error ——
                        // 每发一个网页请求就刷一条 ERROR 会让真正的错误被淹掉。
                        // 也不按 Content-Type 预先筛:相当多的 API 返回 JSON 却标着
                        // text/plain,按头筛会让这些响应白丢树视图。
                        if meta.size_bytes < apogee_core::model::TIER_HIGHLIGHT_MAX {
                            match state2.index_response_for_access(response_access, id) {
                                Ok(_) => {}
                                Err(error) if response_access_lost(&error) => {
                                    state2.finish_inflight(id);
                                    return;
                                }
                                Err(_error) => {
                                    tracing::debug!(
                                        operation = "index_response",
                                        "响应体不是可索引的 JSON，跳过树索引"
                                    );
                                }
                            }
                        }

                        // 索引或 metadata 写入期间也可能发生 close。摘要构造和
                        // `request:done` 入队必须共用同一个 response/session 临界区；否则
                        // 关闭可能插入在最后一次校验和 emit 之间。
                        let done = state2.with_response_summary_for_access(
                            response_access,
                            id,
                            |summary| {
                                app_for_done
                                    .emit(
                                        "request:done",
                                        serde_json::json!({
                                            "workspaceId": event_workspace_id,
                                            "sessionId": event_session_id,
                                            "response": PublicResponseSummary::from(&summary),
                                        }),
                                    )
                                    .map_err(|error| anyhow::anyhow!(error))
                            },
                        );
                        state2.finish_inflight(id);
                        match done {
                            Ok(()) => {}
                            Err(error) if response_access_lost(&error) => {}
                            Err(_error) => {
                                tracing::error!(operation = "emit_request_done", "汇总响应或投递 request:done 失败");
                                cleanup(
                                    "request:error",
                                    serde_json::json!({
                                        "id": id,
                                        "workspaceId": event_workspace_id,
                                        "sessionId": event_session_id,
                                        "error": PublicError::internal(),
                                        "actualRequest": PublicActualRequest::from(&meta.actual_request),
                                    }),
                                );
                            }
                        }
                    }
                    apogee_core::http::Outcome::Cancelled => {
                        cleanup(
                            "request:cancelled",
                            serde_json::json!({
                                "id": id,
                                "workspaceId": event_workspace_id,
                                "sessionId": event_session_id,
                            }),
                        );
                    }
                    apogee_core::http::Outcome::Failed {
                        error,
                        actual_request,
                    } => {
                        cleanup(
                            "request:error",
                            serde_json::json!({
                                "id": id,
                                "workspaceId": event_workspace_id,
                                "sessionId": event_session_id,
                                "error": PublicError::from(&error),
                                "actualRequest": actual_request
                                    .as_ref()
                                    .as_ref()
                                    .map(PublicActualRequest::from),
                            }),
                        );
                    }
                }
            })
            .catch_unwind()
            .await;

            if body.is_err() {
                // The panic hook already records a generic signal. Do not serialize or log the
                // payload: panics raised by formatters or dependency code may contain request
                // credentials or response fragments.
                tracing::error!(
                    request_id = %id,
                    operation = "send_request",
                    "send_request 后台任务 panic，发送通用 request:error 事件"
                );
                cleanup(
                    "request:error",
                    serde_json::json!({
                        "id": id,
                        "workspaceId": event_workspace_id,
                        "sessionId": event_session_id,
                        "error": PublicError::internal(),
                    }),
                );
            }

            // 保持 runtime lease 到后台任务的全部终结路径完成。若 workspace 已切换，
            // 最后一个 lease 的 drop 才会安全回收旧 session 的 spool 目录。
            drop(runtime);
        });

        Ok(apogee_core::request::ExecHandle { id })
    }

    /// 取消一个正在执行的请求。
    ///
    /// 幂等：请求不存在或已结束返回 `false`，前端据此决定要不要提示用户。
    #[tauri::command]
    pub fn cancel_request(
        state: State<'_, Arc<AppState>>,
        handle_id: String,
    ) -> Result<bool, String> {
        let id = parse_id(&handle_id)?;
        Ok(state.cancel_inflight(id))
    }

    /// 成功请求历史命令的 IPC 输入；前端仅需在 `input` 下传入当前扁平字段。
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct HistoryRecordInput {
        pub workspace_id: String,
        pub session_id: String,
        pub created_at_ms: u64,
        pub spec: PublicRequestPreview,
        pub response: PublicResponseSummary,
        pub debug_case_id: Option<String>,
        pub environment_id: Option<String>,
    }

    /// 未收到 HTTP 响应时写入失败历史的 IPC 输入。
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct HistoryRecordErrorInput {
        pub workspace_id: String,
        pub session_id: String,
        pub created_at_ms: u64,
        pub spec: PublicRequestPreview,
        pub error: PublicError,
        pub actual_request: Option<PublicActualRequest>,
        pub duration_ms: f64,
        pub debug_case_id: Option<String>,
        pub environment_id: Option<String>,
    }

    #[tauri::command]
    pub fn history_record(
        state: State<'_, Arc<AppState>>,
        input: HistoryRecordInput,
    ) -> Result<(), String> {
        let HistoryRecordInput {
            workspace_id,
            session_id,
            created_at_ms,
            spec,
            response,
            debug_case_id,
            environment_id,
        } = input;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        let debug_case_id = debug_case_id.as_deref().map(parse_id).transpose()?;
        let environment_id = environment_id.as_deref().map(parse_id).transpose()?;
        state
            .with_workspace_and_response_mutation(
                workspace_id,
                session_id,
                response.id,
                |repository, runtime| {
                    let body = runtime.spool().path_for(response.id);
                    repository.record_history(
                        created_at_ms,
                        spec,
                        response,
                        &body,
                        debug_case_id,
                        environment_id,
                    )
                },
            )
            .map_err(|error| error.to_string())
    }

    /// 将未收到 HTTP 响应的失败运行写进历史。失败不访问 response spool，也不创建空 body。
    #[tauri::command]
    pub fn history_record_error(
        state: State<'_, Arc<AppState>>,
        input: HistoryRecordErrorInput,
    ) -> Result<(), String> {
        let HistoryRecordErrorInput {
            workspace_id,
            session_id,
            created_at_ms,
            spec,
            error,
            actual_request,
            duration_ms,
            debug_case_id,
            environment_id,
        } = input;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        let debug_case_id = debug_case_id.as_deref().map(parse_id).transpose()?;
        let environment_id = environment_id.as_deref().map(parse_id).transpose()?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.record_failed_history(crate::workspace::FailedHistoryWriteInput {
                    created_at_ms,
                    spec,
                    error,
                    actual_request,
                    duration_ms,
                    debug_case_id,
                    environment_id,
                })
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn history_list(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
    ) -> Result<Vec<HistoryEntry>, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace(workspace_id, session_id, |repository| {
                repository.list_history()
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn history_read(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        history_id: String,
    ) -> Result<HistoryRecord, String> {
        let id = parse_id(&history_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .read_history(workspace_id, session_id, id)
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn history_clear(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
    ) -> Result<(), String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.clear_history()
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn curl_import(command: String) -> Result<apogee_core::curl::CurlImportResult, AppError> {
        apogee_core::curl::import(&command)
    }

    #[tauri::command]
    pub fn postman_import(
        json: String,
    ) -> Result<apogee_core::postman::PostmanImportResult, AppError> {
        apogee_core::postman::import_json(&json)
    }

    #[tauri::command]
    pub fn postman_import_file(
        path: String,
    ) -> Result<apogee_core::postman::PostmanImportResult, AppError> {
        let json = std::fs::read_to_string(path).map_err(|e| {
            AppError::new(apogee_core::error::ErrorKind::Io, "无法读取 Postman 文件")
                .detail(e.to_string())
        })?;
        apogee_core::postman::import_json(&json)
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PostmanWorkspaceImportResult {
        pub collection_id: Uuid,
        pub imported_requests: usize,
        pub imported_folders: usize,
        pub warnings: Vec<String>,
    }

    #[derive(Clone, Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PostmanImportProgress {
        import_id: String,
        current: usize,
        total: usize,
        phase: &'static str,
    }

    fn emit_postman_import_progress(
        app: &tauri::AppHandle,
        import_id: &str,
        current: usize,
        total: usize,
        phase: &'static str,
    ) {
        let _ = app.emit(
            "postman-import:progress",
            PostmanImportProgress {
                import_id: import_id.to_owned(),
                current,
                total,
                phase,
            },
        );
    }

    /// Imports a Postman collection into the active workspace, preserving folder paths.
    #[tauri::command]
    pub async fn postman_import_workspace(
        app: tauri::AppHandle,
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        path: String,
        import_id: String,
    ) -> Result<PostmanWorkspaceImportResult, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        let state = state.inner().clone();
        tokio::task::spawn_blocking(move || {
            emit_postman_import_progress(&app, &import_id, 0, 100, "读取文件");
            let json = std::fs::read_to_string(&path)
                .map_err(|error| format!("无法读取 Postman 文件: {error}"))?;
            emit_postman_import_progress(&app, &import_id, 15, 100, "解析集合");
            let imported =
                apogee_core::postman::import_json(&json).map_err(|error| error.to_string())?;
            if imported.requests.is_empty() {
                return Err(imported
                    .warnings
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Postman 集合中没有可导入的请求".into()));
            }
            state
                .with_workspace_mutation(workspace_id, session_id, |repository| {
                    use apogee_core::workspace::WorkspaceNodeKind;
                    use std::collections::HashMap;

                    let collection_snapshot =
                        repository.create_collection(imported.name.clone())?;
                    emit_postman_import_progress(&app, &import_id, 20, 100, "创建集合");
                    let collection = collection_snapshot
                        .nodes
                        .iter()
                        .rev()
                        .find(|node| {
                            matches!(node.kind, WorkspaceNodeKind::Collection)
                                && node.name == imported.name
                        })
                        .ok_or_else(|| anyhow::anyhow!("导入集合创建后无法定位节点"))?;
                    let collection_id = collection.id;
                    let mut folders = HashMap::<String, Uuid>::new();
                    let mut imported_folders = 0;
                    let mut imported_requests = 0;
                    let mut last_progress = 20;

                    for item in &imported.requests {
                        let mut parent_id = collection_id;
                        let mut path_key = String::new();
                        for folder_name in &item.folder {
                            if !path_key.is_empty() {
                                path_key.push('/');
                            }
                            path_key.push_str(folder_name);
                            if let Some(id) = folders.get(&path_key) {
                                parent_id = *id;
                                continue;
                            }
                            let snapshot =
                                repository.create_folder(parent_id, folder_name.clone())?;
                            let folder = snapshot
                                .nodes
                                .iter()
                                .rev()
                                .find(|node| {
                                    node.parent_id == Some(parent_id)
                                        && matches!(node.kind, WorkspaceNodeKind::Folder)
                                        && node.name == *folder_name
                                })
                                .ok_or_else(|| anyhow::anyhow!("导入文件夹创建后无法定位节点"))?;
                            folders.insert(path_key.clone(), folder.id);
                            parent_id = folder.id;
                            imported_folders += 1;
                        }
                        let request = repository.create_request(parent_id, item.name.clone())?;
                        repository.save_request(
                            request.id,
                            item.name.clone(),
                            item.spec.clone(),
                        )?;
                        imported_requests += 1;
                        let progress = 20 + imported_requests * 80 / imported.requests.len();
                        if progress > last_progress || imported_requests == imported.requests.len()
                        {
                            emit_postman_import_progress(
                                &app,
                                &import_id,
                                progress,
                                100,
                                "导入请求",
                            );
                            last_progress = progress;
                        }
                    }
                    Ok(PostmanWorkspaceImportResult {
                        collection_id,
                        imported_requests,
                        imported_folders,
                        warnings: imported.warnings.clone(),
                    })
                })
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| format!("Postman 导入任务异常结束: {error}"))?
    }

    /// Imports a Postman Environment export into the active workspace.
    #[tauri::command]
    pub async fn postman_import_environment_workspace(
        app: tauri::AppHandle,
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        path: String,
        import_id: String,
    ) -> Result<apogee_core::postman::PostmanEnvironmentImportResult, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        let state = state.inner().clone();
        tokio::task::spawn_blocking(move || {
            emit_postman_import_progress(&app, &import_id, 0, 100, "读取文件");
            let json = std::fs::read_to_string(&path)
                .map_err(|error| format!("无法读取 Postman 文件: {error}"))?;
            emit_postman_import_progress(&app, &import_id, 25, 100, "解析环境变量");
            let imported = apogee_core::postman::import_environment_json(&json)
                .map_err(|error| error.to_string())?;
            let variable_count = imported.environment.variables.len();
            state
                .with_workspace_mutation(workspace_id, session_id, |repository| {
                    emit_postman_import_progress(&app, &import_id, 55, 100, "创建环境");
                    let created =
                        repository.create_environment(imported.environment.name.clone())?;
                    let mut environment = imported.environment.clone();
                    environment.id = created.id;
                    let environment = repository.save_environment(environment)?;
                    emit_postman_import_progress(
                        &app,
                        &import_id,
                        if variable_count == 0 { 100 } else { 95 },
                        100,
                        "写入变量",
                    );
                    Ok(apogee_core::postman::PostmanEnvironmentImportResult {
                        environment,
                        warnings: imported.warnings.clone(),
                    })
                })
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| format!("Postman 环境导入任务异常结束: {error}"))?
    }

    /// Imports a YApi project in two phases. Analyze performs network access and keeps
    /// the transformed plan in Rust memory; only apply may modify the workspace.
    #[tauri::command]
    pub async fn yapi_analyze_import(
        state: State<'_, Arc<AppState>>,
        base_url: String,
        token: String,
    ) -> Result<apogee_core::yapi::YapiAnalyzeResult, AppError> {
        let plan = apogee_core::yapi::analyze(&base_url, &token).await?;
        let plan_id = Uuid::new_v4();
        let summary = plan.summary(plan_id);
        state
            .yapi_plans
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(plan_id, plan);
        Ok(summary)
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct YapiApplyOptions {
        pub collection_name: Option<String>,
        #[serde(default = "default_true")]
        pub import_environments: bool,
    }

    fn default_true() -> bool {
        true
    }

    #[tauri::command]
    pub fn yapi_apply_import(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        plan_id: String,
        options: YapiApplyOptions,
    ) -> Result<crate::workspace::YapiWorkspaceImportResult, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        let plan_id = parse_id(&plan_id)?;
        // 先校验会话，避免失效窗口消耗仍可在新会话使用的计划。
        state
            .with_workspace(workspace_id, session_id, |_| Ok(()))
            .map_err(|error| error.to_string())?;
        let plan = state
            .yapi_plans
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&plan_id)
            .ok_or_else(|| "YApi 导入计划已失效，请重新分析项目".to_string())?;
        let result = state.with_workspace_mutation(workspace_id, session_id, |repository| {
            repository.import_yapi_plan(
                &plan,
                options.collection_name.as_deref(),
                options.import_environments,
            )
        });
        match result {
            Ok(imported) => Ok(imported),
            Err(error) => {
                state
                    .yapi_plans
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(plan_id, plan);
                Err(error.to_string())
            }
        }
    }

    #[tauri::command]
    pub fn yapi_discard_import(
        state: State<'_, Arc<AppState>>,
        plan_id: String,
    ) -> Result<(), String> {
        let plan_id = parse_id(&plan_id)?;
        state
            .yapi_plans
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&plan_id);
        Ok(())
    }

    /// 读取当前工作区的扁平树。
    #[tauri::command]
    pub fn workspace_load(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
    ) -> Result<WorkspaceSnapshot, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace(workspace_id, session_id, |repository| repository.snapshot())
            .map_err(|error| error.to_string())
    }

    /// 读取一个请求的完整编辑模型。
    #[tauri::command]
    pub fn workspace_read_request(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        request_id: String,
    ) -> Result<WorkspaceRequest, String> {
        let id = parse_id(&request_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace(workspace_id, session_id, |repository| {
                repository.read_request(id)
            })
            .map_err(|error| error.to_string())
    }

    /// 自动保存请求编辑器的持久化字段。前端在 300ms 防抖后调用。
    #[tauri::command]
    pub fn workspace_save_request(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        request_id: String,
        name: String,
        spec: RequestSpec,
    ) -> Result<(), String> {
        let id = parse_id(&request_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.save_request(id, name, spec)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_list_debug_cases(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        request_id: String,
    ) -> Result<Vec<DebugCase>, String> {
        let request_id = parse_id(&request_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace(workspace_id, session_id, |repository| {
                repository.list_debug_cases(request_id)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_create_debug_case(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        request_id: String,
        name: String,
        case_type: DebugCaseType,
        spec: RequestSpec,
    ) -> Result<DebugCase, String> {
        let request_id = parse_id(&request_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.create_debug_case(request_id, name, case_type, spec)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_delete_debug_case(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        request_id: String,
        case_id: String,
    ) -> Result<(), String> {
        let request_id = parse_id(&request_id)?;
        let case_id = parse_id(&case_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.delete_debug_case(request_id, case_id)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_read_container(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        node_id: String,
    ) -> Result<apogee_core::workspace::WorkspaceContainer, String> {
        let node_id = parse_id(&node_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace(workspace_id, session_id, |repository| {
                repository.read_container(node_id)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_save_container(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        container: apogee_core::workspace::WorkspaceContainer,
    ) -> Result<apogee_core::workspace::WorkspaceContainer, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.save_container(container)
            })
            .map_err(|error| error.to_string())
    }

    /// 创建一个空环境。前端随后可通过 workspace_save_environment 写入变量。
    #[tauri::command]
    pub fn workspace_create_environment(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        name: String,
    ) -> Result<Environment, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.create_environment(name)
            })
            .map_err(|error| error.to_string())
    }

    /// 原子保存环境名称与变量值。
    #[tauri::command]
    pub fn workspace_save_environment(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        environment: Environment,
    ) -> Result<Environment, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.save_environment(environment)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_delete_environment(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        environment_id: String,
    ) -> Result<WorkspaceSnapshot, String> {
        let environment_id = parse_id(&environment_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.delete_environment(environment_id)
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn workspace_save_state(
        app: tauri::AppHandle,
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        workspace_state: crate::workspace_state::WorkspaceState,
    ) -> Result<(), String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        if !workspace_state.active_environment_set {
            return Err("工作区状态缺少环境选择标记".into());
        }
        let data_dir = super::app_data_dir(&app)?;
        let workspace_state_repository =
            crate::workspace_state::WorkspaceStateRepository::new(&data_dir);
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                if let Some(environment_id) = workspace_state.active_environment_id {
                    repository.read_environment(environment_id)?;
                }
                workspace_state_repository.save(workspace_id, &workspace_state)
            })
            .map_err(|error| error.to_string())
    }

    /// 创建集合并返回刷新后的扁平树。
    #[tauri::command]
    pub fn workspace_create_collection(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        name: String,
    ) -> Result<WorkspaceSnapshot, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.create_collection(name)
            })
            .map_err(|error| error.to_string())
    }

    /// 在集合或文件夹下创建文件夹。
    #[tauri::command]
    pub fn workspace_create_folder(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        parent_id: String,
        name: String,
    ) -> Result<WorkspaceSnapshot, String> {
        let parent = parse_id(&parent_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.create_folder(parent, name)
            })
            .map_err(|error| error.to_string())
    }

    /// 在集合或文件夹下创建请求，并返回初始请求模型。
    #[tauri::command]
    pub fn workspace_create_request(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        parent_id: String,
        name: String,
    ) -> Result<WorkspaceRequest, String> {
        let parent = parse_id(&parent_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.create_request(parent, name)
            })
            .map_err(|error| error.to_string())
    }

    /// 重命名集合、文件夹或请求。
    #[tauri::command]
    pub fn workspace_rename_node(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        node_id: String,
        name: String,
    ) -> Result<WorkspaceSnapshot, String> {
        let id = parse_id(&node_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.rename_node(id, name)
            })
            .map_err(|error| error.to_string())
    }

    /// 删除集合、文件夹或请求。删除容器会递归删除其目录。
    #[tauri::command]
    pub fn workspace_delete_node(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        node_id: String,
    ) -> Result<WorkspaceSnapshot, String> {
        let id = parse_id(&node_id)?;
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace_mutation(workspace_id, session_id, |repository| {
                repository.delete_node(id)
            })
            .map_err(|error| error.to_string())
    }

    /// 原生保存对话框的初始文件名仍是外部输入（响应 header / WebView），必须在 Rust
    /// 侧再做一次跨平台清理。它只影响默认值，用户仍可在系统对话框中自行编辑。
    fn safe_save_file_name(value: &str, response_id: Uuid) -> String {
        let fallback = format!("response-{}.bin", response_id.simple());
        let leaf = value
            .rsplit(|character| matches!(character, '/' | '\\'))
            .next()
            .unwrap_or_default();
        let cleaned: String = leaf
            .chars()
            // Remove bidi overrides/isolates too: they can make an executable-looking extension
            // appear as a benign one in a native save dialog.
            .filter(|character| !disallowed_file_name_character(*character))
            .map(|character| {
                if matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                    '_'
                } else {
                    character
                }
            })
            .collect();
        let trimmed = cleaned
            .trim()
            .trim_end_matches(|character: char| matches!(character, '.' | ' '));
        let limited: String = trimmed.chars().take(180).collect();
        if limited.is_empty()
            || limited == "."
            || limited == ".."
            || windows_reserved_file_name(&limited)
        {
            return fallback;
        }
        limited
    }

    fn disallowed_file_name_character(character: char) -> bool {
        character.is_control()
            || matches!(
                character,
                '\u{00AD}'
                    | '\u{061C}'
                    | '\u{180E}'
                    | '\u{200B}'..='\u{200F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2060}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{FEFF}'
            )
    }

    fn windows_reserved_file_name(name: &str) -> bool {
        let stem = name
            .split_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(name)
            .to_ascii_lowercase();
        matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
            || matches!(
                stem.as_str(),
                "com1"
                    | "com2"
                    | "com3"
                    | "com4"
                    | "com5"
                    | "com6"
                    | "com7"
                    | "com8"
                    | "com9"
                    | "lpt1"
                    | "lpt2"
                    | "lpt3"
                    | "lpt4"
                    | "lpt5"
                    | "lpt6"
                    | "lpt7"
                    | "lpt8"
                    | "lpt9"
            )
    }

    fn parse_id(s: &str) -> Result<Uuid, String> {
        Uuid::parse_str(s).map_err(|_| format!("非法的响应 id：{s}"))
    }

    fn parse_session(workspace_id: &str, session_id: &str) -> Result<(Uuid, Uuid), String> {
        Ok((parse_id(workspace_id)?, parse_id(session_id)?))
    }
}
