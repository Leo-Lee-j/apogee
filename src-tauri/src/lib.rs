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
mod protocol;
mod state;

use state::AppState;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,apogee_desktop_lib=debug".into()),
        )
        .init();

    let state = Arc::new(AppState::new().expect("初始化应用状态失败"));
    let proto_state = state.clone();

    tauri::Builder::default()
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
            commands::spike_make_response,
            commands::get_response_meta,
            commands::index_response,
            commands::get_response_tree,
            commands::process_memory,
            commands::spike_report,
            commands::spike_autorun,
        ])
        .setup(|app| {
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

mod commands {
    use super::state::AppState;
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tauri::{Manager, State};
    use uuid::Uuid;
    use apogee_core::response::{ResponseSummary, TreeNode};

    #[derive(Debug, Clone, Copy, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct BootMetrics {
        mount_ms: f64,
        ready_ms: f64,
        first_paint_ms: f64,
    }

    /// 前端首帧就绪 → 显示窗口（03 文档 §4.1 分阶段启动）。
    #[tauri::command]
    pub fn app_ready(
        window: tauri::Window,
        metrics: Option<BootMetrics>,
    ) -> Result<(), String> {
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

    fn parse_id(s: &str) -> Result<Uuid, String> {
        Uuid::parse_str(s).map_err(|_| format!("非法的响应 id：{s}"))
    }
}
