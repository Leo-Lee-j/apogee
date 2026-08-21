//! `apogee://` 自定义协议 handler —— 大响应体的传输通道（03 文档 §3.1）。
//!
//! # 为什么需要它
//!
//! Tauri 的 `invoke` 走 JSON 序列化。10MB 响应体过一次 `invoke` 等于
//! 「Rust 端序列化成 JSON 字符串 → 传输 → JS 端 JSON.parse」三次全量拷贝，
//! 这正是 Electron 系客户端处理大响应体卡死的同类问题。
//!
//! 走自定义协议后：
//!   - 支持 HTTP Range 语义，前端只取首屏那一段（默认 64KB）
//!   - 传输是二进制，不经 JSON
//!   - 磁盘侧是 seek + 定长读，100MB 文件读 64KB 的成本和 64KB 文件相同
//!
//! # URL 形态
//!
//! Windows 上 Tauri 把自定义协议映射到 `http://apogee.localhost/...`，
//! 其它平台是 `apogee://localhost/...`。两者的 `uri().path()` 都是 `/response/{id}`。

use crate::state::AppState;
use tauri::http::{header, Request, Response, StatusCode};
use uuid::Uuid;

/// 单次响应的上限，与 apogee_core::response::MAX_RANGE_BYTES 对齐。
const MAX_CHUNK: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy)]
enum ProtocolResponseAccess {
    Workspace {
        workspace_id: Uuid,
        session_id: Uuid,
    },
    Synthetic,
}

pub fn handle(state: &AppState, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = request.uri().path().to_string();

    let Some(rest) = path.strip_prefix("/response/") else {
        return err(StatusCode::NOT_FOUND, "未知路径");
    };
    let id_str = rest.split('/').next().unwrap_or("");
    let Ok(id) = Uuid::parse_str(id_str) else {
        return err(StatusCode::BAD_REQUEST, "非法的响应 id");
    };
    let access = match parse_response_access(request) {
        Ok(access) => access,
        Err(message) => return err(StatusCode::FORBIDDEN, message),
    };

    let total = match response_size(state, access, id) {
        Ok(n) => n,
        Err(_) => return err(StatusCode::NOT_FOUND, "响应已过期或不存在"),
    };

    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_range(s, total));

    match range {
        Some((start, end)) => {
            let len = (end - start + 1).min(MAX_CHUNK);
            match response_range(state, access, id, start, len) {
                Ok(buf) => {
                    let real_end = start + buf.len().saturating_sub(1) as u64;
                    Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                        .header(header::ACCEPT_RANGES, "bytes")
                        .header(header::CONTENT_LENGTH, buf.len().to_string())
                        .header(
                            header::CONTENT_RANGE,
                            format!("bytes {start}-{real_end}/{total}"),
                        )
                        // 自定义协议的响应默认不带 CORS 头，fetch 会被拦。
                        // 这里显式放开 —— 请求方只可能是本应用的 WebView。
                        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                        .header(
                            header::ACCESS_CONTROL_EXPOSE_HEADERS,
                            "Content-Range, Content-Length, Accept-Ranges",
                        )
                        .body(buf)
                        .unwrap_or_else(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "构造响应失败"))
                }
                Err(_) => err(StatusCode::NOT_FOUND, "响应已过期或不存在"),
            }
        }
        None => {
            // 无 Range 头 = 整体拉取。
            // **这条路径是给 Spike 做对照用的**（量化「不做 Range」的代价），
            // 产品代码里不应该走到这里。所以加了硬上限，防止误用把 100MB
            // 一次性塞进 WebView。
            let len = total.min(MAX_CHUNK);
            match response_range(state, access, id, 0, len) {
                Ok(buf) => {
                    let truncated = total > len;
                    let mut b = Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                        .header(header::ACCEPT_RANGES, "bytes")
                        .header(header::CONTENT_LENGTH, buf.len().to_string())
                        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                        .header(
                            header::ACCESS_CONTROL_EXPOSE_HEADERS,
                            "Content-Range, Content-Length, Accept-Ranges, X-Apogee-Truncated",
                        )
                        .header(
                            header::CONTENT_RANGE,
                            format!("bytes 0-{}/{}", buf.len().saturating_sub(1), total),
                        );
                    if truncated {
                        // 明确告知被截断，而不是静默给一半数据
                        b = b.header("X-Apogee-Truncated", "1");
                    }
                    b.body(buf)
                        .unwrap_or_else(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "构造响应失败"))
                }
                Err(_) => err(StatusCode::NOT_FOUND, "响应已过期或不存在"),
            }
        }
    }
}

fn parse_response_access(
    request: &Request<Vec<u8>>,
) -> Result<ProtocolResponseAccess, &'static str> {
    let mut workspace_id = None;
    let mut session_id = None;
    let mut scope = None;
    for pair in request.uri().query().unwrap_or_default().split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "workspaceId" => workspace_id = Some(value),
            "sessionId" => session_id = Some(value),
            "scope" => scope = Some(value),
            _ => {}
        }
    }

    match (workspace_id, session_id, scope) {
        (Some(workspace_id), Some(session_id), None) => {
            let workspace_id =
                Uuid::parse_str(workspace_id).map_err(|_| "响应 URL 的 workspaceId 非法")?;
            let session_id =
                Uuid::parse_str(session_id).map_err(|_| "响应 URL 的 sessionId 非法")?;
            Ok(ProtocolResponseAccess::Workspace {
                workspace_id,
                session_id,
            })
        }
        (None, None, Some("spike")) => Ok(ProtocolResponseAccess::Synthetic),
        _ => Err("响应 URL 缺少有效 session"),
    }
}

fn response_size(
    state: &AppState,
    access: ProtocolResponseAccess,
    response_id: Uuid,
) -> anyhow::Result<u64> {
    match access {
        ProtocolResponseAccess::Workspace {
            workspace_id,
            session_id,
        } => state.response_size(workspace_id, session_id, response_id),
        ProtocolResponseAccess::Synthetic => state.synthetic_response_size(response_id),
    }
}

fn response_range(
    state: &AppState,
    access: ProtocolResponseAccess,
    response_id: Uuid,
    start: u64,
    len: u64,
) -> anyhow::Result<Vec<u8>> {
    match access {
        ProtocolResponseAccess::Workspace {
            workspace_id,
            session_id,
        } => state.response_range(workspace_id, session_id, response_id, start, len),
        ProtocolResponseAccess::Synthetic => {
            state.synthetic_response_range(response_id, start, len)
        }
    }
}

/// 解析 `bytes=start-end`。只支持单区间 —— 多区间对我们的场景没有意义。
fn parse_range(raw: &str, total: u64) -> Option<(u64, u64)> {
    let spec = raw.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let a = a.trim();
    let b = b.trim();

    if a.is_empty() {
        // `bytes=-500`：末尾 500 字节
        let n: u64 = b.parse().ok()?;
        if n == 0 || total == 0 {
            return None;
        }
        let start = total.saturating_sub(n);
        return Some((start, total - 1));
    }
    let start: u64 = a.parse().ok()?;
    if start >= total {
        return None;
    }
    let end = if b.is_empty() {
        total - 1
    } else {
        b.parse::<u64>().ok()?.min(total - 1)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

fn err(code: StatusCode, msg: &str) -> Response<Vec<u8>> {
    // builder 在已知 status + 合法 header 名字下不会失败,这里 unwrap 只是
    // 满足返回类型。改用 ok_or_else 兜底,避免 release abort 整进程。
    match Response::builder()
        .status(code)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(msg.as_bytes().to_vec())
    {
        Ok(response) => response,
        Err(error) => {
            tracing::error!("构造错误响应失败:{};改用最简 fallback", error);
            Response::new(msg.as_bytes().to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn parses_normal_range() {
        assert_eq!(parse_range("bytes=0-65535", 10_000_000), Some((0, 65535)));
        assert_eq!(parse_range("bytes=100-199", 1000), Some((100, 199)));
    }

    #[test]
    fn clamps_end_to_total() {
        assert_eq!(parse_range("bytes=0-999999", 500), Some((0, 499)));
    }

    #[test]
    fn open_ended_range() {
        assert_eq!(parse_range("bytes=100-", 500), Some((100, 499)));
    }

    #[test]
    fn suffix_range() {
        assert_eq!(parse_range("bytes=-100", 500), Some((400, 499)));
    }

    #[test]
    fn rejects_invalid() {
        assert_eq!(parse_range("items=0-1", 500), None);
        assert_eq!(parse_range("bytes=600-700", 500), None); // start 越界
        assert_eq!(parse_range("bytes=200-100", 500), None); // end < start
        assert_eq!(parse_range("bytes=0-10,20-30", 500), None); // 多区间不支持
    }
}
