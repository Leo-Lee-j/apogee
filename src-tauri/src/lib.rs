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

mod mem;
mod app_settings;
mod protocol;
mod state;
mod workspace;
mod workspace_registry;
mod workspace_state;

use state::AppState;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

pub fn run() {
    install_panic_hook();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,apogee_desktop_lib=debug".into()),
        )
        .init();

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
            commands::spike_make_response,
            commands::get_response_meta,
            commands::index_response,
            commands::get_response_tree,
            commands::process_memory,
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
            commands::history_list,
            commands::history_read,
            commands::history_clear,
            commands::curl_import,
        ])
        .setup(move |app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            std::fs::create_dir_all(&data_dir)?;

            let settings = app_settings::AppSettingsRepository::new(&data_dir).load()?;
            let registry = workspace_registry::WorkspaceRegistry::new(&data_dir);
            if registry.list()?.is_empty() {
                let legacy_root = data_dir.join("workspace");
                if legacy_root.join("apogee.yaml").is_file() {
                    let legacy = workspace::WorkspaceRepository::open(&legacy_root)?;
                    registry.register(&legacy)?;
                }
                let example_root = std::path::PathBuf::from(&settings.default_workspace_root).join("example-workspace");
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
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}

/// 把 panic 信息打到 stderr + tracing。
///
/// 在 `panic = "abort"` 下,Rust 会先调用 hook,再 abort,所以这是我们最后
/// 一道「能看到点啥」的窗口。不写文件 —— 用户机器上日志目录路径不一定稳定,
/// stderr 至少能被开发机抓到。
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // 先把信息打到 stderr(同步,即使后面的 tracing 已经失效也能落地)。
        eprintln!("[apogee panic] {}", info);
        // 再走 tracing —— release 下若还有订阅者,会写到日志。
        tracing::error!("PANIC: {}", info);
        previous(info);
    }));
}

mod commands {
    use crate::app_settings::{AppSettings, AppSettingsRepository};
    use super::state::AppState;
    use crate::workspace::WorkspaceRepository;
    use crate::workspace::{HistoryEntry, HistoryRecord};
    use crate::workspace_registry::{WorkspaceDescriptor, WorkspaceRegistry};
    use apogee_core::error::AppError;
    use apogee_core::request::RequestSpec;
    use apogee_core::response::{ResponseSummary, TreeNode};
    use apogee_core::vars::Environment;
    use apogee_core::workspace::{WorkspaceRequest, WorkspaceSnapshot};
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

    #[tauri::command]
    pub fn app_settings_load(app: tauri::AppHandle) -> Result<AppSettings, String> {
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        AppSettingsRepository::new(&dir).load().map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub fn app_settings_save(app: tauri::AppHandle, settings: AppSettings) -> Result<AppSettings, String> {
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let repo = AppSettingsRepository::new(&dir);
        repo.save(&settings).map_err(|e| e.to_string())?;
        repo.load().map_err(|e| e.to_string())
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
                let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
                let settings = AppSettingsRepository::new(&data_dir).load().map_err(|e| e.to_string())?;
                let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
                let slug = if slug.trim_matches('_').is_empty() { "workspace".to_string() } else { slug.trim_matches('_').to_string() };
                let base = slug;
                let mut candidate = PathBuf::from(&settings.default_workspace_root).join(&base);
                let mut suffix = 2;
                while candidate.exists() {
                    candidate = PathBuf::from(&settings.default_workspace_root).join(format!("{base}-{suffix}"));
                    suffix += 1;
                }
                candidate
            }
        };
        let repository = WorkspaceRepository::create(root, name)
            .map_err(|error| error.to_string())?;
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
        let snapshot = repository.snapshot().map_err(|error| error.to_string())?;
        let descriptor = registry(app)?
            .register(&repository)
            .map_err(|error| error.to_string())?;
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let mut workspace_state = crate::workspace_state::WorkspaceStateRepository::new(&data_dir)
            .load(snapshot.id)
            .map_err(|error| error.to_string())?;
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
        let repository = repository
            .with_runtime_root(&runtime_root)
            .map_err(|error| error.to_string())?;
        let (workspace_id, session_id) = state
            .open_workspace(repository, &runtime_root)
            .map_err(|error| error.to_string())?;
        Ok(WorkspaceOpenResult {
            descriptor,
            workspace_id,
            session_id,
            snapshot,
            state: workspace_state,
        })
    }

    fn registry(app: &tauri::AppHandle) -> Result<WorkspaceRegistry, String> {
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
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
    ) -> Result<ResponseSummary, String> {
        state
            .make_synthetic(target_bytes)
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub fn get_response_meta(
        state: State<'_, Arc<AppState>>,
        response_id: String,
    ) -> Result<ResponseSummary, String> {
        let id = parse_id(&response_id)?;
        state.summary(id).map_err(|e| e.to_string())
    }

    /// 显式建 JSON 索引。
    ///
    /// 大 body 下这一步是**显式的**：02 文档 §4.4 规定 2MB 以上树视图需要
    /// 用户手动点「解析」，不自动做。所以命令层也不隐式触发。
    #[tauri::command]
    pub fn index_response(
        state: State<'_, Arc<AppState>>,
        response_id: String,
    ) -> Result<ResponseSummary, String> {
        let id = parse_id(&response_id)?;
        state
            .index_response(id)
            .map_err(|e| format!("JSON 索引失败：{e}"))
    }

    /// 取一页树节点。前端拿到的是扁平数组，不做递归构造（03 文档 §4.4）。
    #[tauri::command]
    pub fn get_response_tree(
        state: State<'_, Arc<AppState>>,
        response_id: String,
        path: String,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TreeNode>, String> {
        let id = parse_id(&response_id)?;
        state
            .tree_page(id, &path, offset, limit)
            .map_err(|error| error.to_string())
    }

    /// 当前进程内存。供「性能自检」面板与性能基准使用（PRD §5.2）。
    #[tauri::command]
    pub fn process_memory() -> super::mem::MemInfo {
        super::mem::current()
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
        spec: apogee_core::request::RequestSpec,
        ctx: Option<apogee_core::request::ExecContext>,
        workspace_id: String,
        session_id: String,
    ) -> Result<apogee_core::request::ExecHandle, String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace(workspace_id, session_id, |_| Ok(()))
            .map_err(|error| error.to_string())?;
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
        let cancel_rx = state.register_inflight(id, tab_id);

        let spool = state
            .spool
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let state2 = state.inner().clone();
        let app2 = app.clone();

        // 必须保证:无论 spawned future 是否 panic,都会发出一个终结事件。
        // 否则前端按钮永远卡在「等待中」。
        //
        // release 下 `panic = "abort"`,task panic 直接整进程 abort,所以这里
        // 用 `FutureExt::catch_unwind` 主动抓:tokio runtime 的默认行为是
        // 在 panic hook 上报后继续运行,但我们要 fallback 事件,不能依赖 runtime。
        tokio::spawn(async move {
            use futures_util::FutureExt;
            use std::panic::AssertUnwindSafe;

            let app_for_cleanup = app2.clone();
            let id_for_cleanup = id;

            let cleanup = |event: &str, payload: serde_json::Value| {
                state2.finish_inflight(id_for_cleanup);
                let _ = app_for_cleanup.emit(event, payload);
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
                let _ = app2.emit(
                    "request:progress",
                    serde_json::json!({
                        "id": id,
                        "received": progress.received,
                        "total": progress.total,
                    }),
                );
            };

            let body = AssertUnwindSafe(async {
                let outcome =
                    apogee_core::http::execute(
                        id,
                        &spec,
                        environment.as_ref(),
                        &spool,
                        cancel_rx,
                        on_progress,
                    )
                    .await;

                match outcome {
                    apogee_core::http::Outcome::Done(meta) => {
                        if let Err(error) = state2.record_http_meta(
                            id, meta.size_bytes, meta.timing, meta.content_type.clone(),
                        ) {
                            tracing::error!(?error, "记录 HTTP 元信息失败");
                            cleanup(
                                "request:error",
                                serde_json::json!({
                                    "id": id,
                                    "error": { "kind": "Internal", "message": error.to_string() },
                                }),
                            );
                            return;
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
                            if let Err(error) = state2.index_response(id) {
                                tracing::debug!(
                                    %error,
                                    content_type = ?meta.content_type,
                                    "响应体不是可索引的 JSON，跳过树索引"
                                );
                            }
                        }

                        let summary = match state2.summarize_http(&meta) {
                            Ok(summary) => summary,
                            Err(error) => {
                                tracing::error!(?error, "汇总响应失败");
                                cleanup(
                                    "request:error",
                                    serde_json::json!({
                                        "id": id,
                                        "error": { "kind": "Internal", "message": error.to_string() },
                                    }),
                                );
                                return;
                            }
                        };

                        cleanup(
                            "request:done",
                            serde_json::json!({ "response": summary }),
                        );
                    }
                    apogee_core::http::Outcome::Cancelled => {
                        cleanup(
                            "request:cancelled",
                            serde_json::json!({ "id": id }),
                        );
                    }
                    apogee_core::http::Outcome::Failed(error) => {
                        cleanup(
                            "request:error",
                            serde_json::json!({
                                "id": id,
                                "error": error,
                            }),
                        );
                    }
                }
            })
            .catch_unwind()
            .await;

            if let Err(panic_payload) = body {
                // 兜底:即使 catch_unwind 抓到,后续 emit 仍可能 panic(emit 本身
                // 不会,但保险起见再包一层 try)。
                let detail = if let Some(s) = panic_payload.downcast_ref::<&'static str>() {
                    (*s).to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "未知 panic".to_string()
                };
                tracing::error!(
                    panic = %detail,
                    request_id = %id,
                    "send_request 后台任务 panic,改为发送 request:error 兜底事件"
                );
                cleanup(
                    "request:error",
                    serde_json::json!({
                        "id": id,
                        "error": {
                            "kind": "Internal",
                            "message": format!("后台任务异常:{}", detail),
                        },
                    }),
                );
            }
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

    #[tauri::command]
    pub fn history_record(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
        created_at_ms: u64,
        spec: apogee_core::request::RequestSpec,
        response: ResponseSummary,
    ) -> Result<(), String> {
        let body = state
            .spool
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .path_for(response.id);
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace(workspace_id, session_id, |repository| {
                repository.record_history(created_at_ms, spec, response, &body)
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
        let (mut record, body) = state
            .with_workspace(workspace_id, session_id, |repository| {
                repository.read_history(id)
            })
            .map_err(|error| error.to_string())?;
        record.response = state
            .restore_history_response(record.response, &body)
            .map_err(|error| error.to_string())?;
        Ok(record)
    }

    #[tauri::command]
    pub fn history_clear(
        state: State<'_, Arc<AppState>>,
        workspace_id: String,
        session_id: String,
    ) -> Result<(), String> {
        let (workspace_id, session_id) = parse_session(&workspace_id, &session_id)?;
        state
            .with_workspace(workspace_id, session_id, |repository| {
                repository.clear_history()
            })
            .map_err(|error| error.to_string())
    }

    #[tauri::command]
    pub fn curl_import(command: String) -> Result<apogee_core::curl::CurlImportResult, AppError> {
        apogee_core::curl::import(&command)
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
            .with_workspace(workspace_id, session_id, |repository| {
                repository.save_request(id, name, spec)
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
            .with_workspace(workspace_id, session_id, |repository| {
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
            .with_workspace(workspace_id, session_id, |repository| {
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
            .with_workspace(workspace_id, session_id, |repository| {
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
        state
            .with_workspace(workspace_id, session_id, |repository| {
                if let Some(environment_id) = workspace_state.active_environment_id {
                    repository.read_environment(environment_id)?;
                }
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        crate::workspace_state::WorkspaceStateRepository::new(&data_dir)
            .save(workspace_id, &workspace_state)
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
            .with_workspace(workspace_id, session_id, |repository| {
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
            .with_workspace(workspace_id, session_id, |repository| {
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
            .with_workspace(workspace_id, session_id, |repository| {
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
            .with_workspace(workspace_id, session_id, |repository| {
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
            .with_workspace(workspace_id, session_id, |repository| {
                repository.delete_node(id)
            })
            .map_err(|error| error.to_string())
    }

    fn parse_id(s: &str) -> Result<Uuid, String> {
        Uuid::parse_str(s).map_err(|_| format!("非法的响应 id：{s}"))
    }

    fn parse_session(workspace_id: &str, session_id: &str) -> Result<(Uuid, Uuid), String> {
        Ok((parse_id(workspace_id)?, parse_id(session_id)?))
    }
}
