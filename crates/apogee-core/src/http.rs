//! HTTP 执行 —— 把 `RequestSpec` 变成真实网络请求（03 文档 §4.5）。
//!
//! # 三条设计约束
//!
//! 1. **响应体边收边写盘。** 全程不在内存里持有完整 body，这是「100MB 响应不崩」
//!    的前提（§4.3）。所以这里用 `chunk().await` 循环 + `SpoolWriter`，
//!    而不是 `response.bytes().await`（后者会把整个 body 读进一个 `Vec`）。
//!
//! 2. **Client 全局单例、按配置分池。** §4.5 要求连接池复用。每次请求新建
//!    `Client` 会丢掉连接池和 DNS 缓存，把「请求发送开销 < 5ms」直接打穿。
//!
//! 3. **取消要能中断等待中的连接。** 只在分块之间查标志位是不够的 —— 请求最常
//!    卡在 connect / TTFB 阶段，那时一个 chunk 都还没到。所以用 `tokio::select!`
//!    把取消信号和网络 future 一起等。

use crate::error::{AppError, ErrorKind, Hint};
use crate::model::{ResponseMeta, Timing};
use crate::request::{ApiKeyIn, AuthConfig, Body, HttpVersion, RequestSpec, ResolvedSettings};
use crate::response::Spool;
use crate::vars::{resolve_request, Environment};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

/// 单次请求允许落盘的最大 body。超过则截断并置 `truncated`。
///
/// 不设上限的后果是一个 `Transfer-Encoding: chunked` 的无限流能把磁盘写满。
pub const MAX_BODY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────
// URL 构建
// ─────────────────────────────────────────────────────────────────────

/// 把 spec 里的 URL、path 参数、query 参数拼成最终 URL。
///
/// 独立成纯函数是为了能不联网单测 —— 这段是 §4.5 里「微秒级同步完成」的部分，
/// 也是最容易出边界问题的部分（已有 query 的 URL 该用 `?` 还是 `&`）。
pub fn build_url(spec: &RequestSpec) -> Result<String, AppError> {
    let mut url = spec.url.trim().to_string();
    if url.is_empty() {
        return Err(AppError::new(ErrorKind::Parse, "URL 为空").hint(Hint::new(
            "在地址栏输入完整 URL，例如 https://api.example.com/users",
        )));
    }

    // 没写协议时补 https。用户在地址栏打 `api.example.com/users` 是极常见的输入，
    // 直接报「非法 URL」属于明知用户意图却不做事。补 https 而非 http：
    // 默认走加密，要降级得显式写出来。
    if !url.contains("://") {
        url = format!("https://{url}");
    }

    // path 参数：`/users/:id` → `/users/42`
    for param in spec.path_params.iter().filter(|item| item.enabled) {
        if param.key.is_empty() {
            continue;
        }
        let placeholder = format!(":{}", param.key);
        url = url.replace(&placeholder, &urlencoding_path(&param.value));
    }

    // query 参数
    let mut pairs: Vec<String> = spec
        .params
        .iter()
        .filter(|item| item.enabled && !item.key.is_empty())
        .map(|item| {
            format!(
                "{}={}",
                urlencoding_query(&item.key),
                urlencoding_query(&item.value)
            )
        })
        .collect();

    // `location: query` 的 API Key 也是一个 query 参数，必须在这里拼进 URL。
    // 放在这里而不是 `apply_auth` 里：query 段的拼装规则（`?` 还是 `&`、
    // 怎么编码）只应该有一处实现，否则两边迟早漂移。
    if let AuthConfig::ApiKey {
        key,
        value,
        location: ApiKeyIn::Query,
    } = &spec.auth
    {
        if !key.trim().is_empty() {
            pairs.push(format!(
                "{}={}",
                urlencoding_query(key),
                urlencoding_query(value)
            ));
        }
    }

    if !pairs.is_empty() {
        // URL 里可能已经带了 query（用户粘贴过来的常常带），那时要用 `&` 接。
        // 判断依据是 `?` 而不是「有没有参数」—— `?a=1` 与 `?` 都算已有 query 段。
        let sep = if url.contains('?') { '&' } else { '?' };
        url.push(sep);
        url.push_str(&pairs.join("&"));
    }

    Ok(url)
}

/// query 分量的百分号编码。
///
/// 不引 `urlencoding` crate：规则就这几行，且我们需要**自己控制**哪些字符保留
/// （比如 `{{var}}` 未解析时不该被编码成 `%7B%7B`，那会让报错信息变得无法阅读）。
fn urlencoding_query(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// path 分量：比 query 多保留 `/`，否则 path 参数里带斜杠会被编码成 `%2F`
/// 而打断路径结构。
fn urlencoding_path(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// 请求体编码
// ─────────────────────────────────────────────────────────────────────

/// 编码后的请求体：字节 + 该带的 `Content-Type`。
///
/// `content_type` 是 `Option` 而不是空串：`None` 表示**不要设这个头**。
/// 对 `Body::None` 来说这两者有区别 —— 发一个带 `Content-Type: ` 空值的 GET
/// 会让某些服务端返回 400。
#[derive(Debug)]
pub struct EncodedBody {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

/// 把 `Body` 编码成字节。
///
/// 注意这里**不流式**：请求体来自用户在编辑器里敲的内容，量级是 KB。
/// 需要流式的是 `Body::Binary`（上传大文件）和 `FormData` 的文件项，
/// 文件 body 由 `apply_body` 走流式路径，不会调用本函数。
pub fn encode_body(body: &Body) -> Result<EncodedBody, AppError> {
    Ok(match body {
        Body::None => EncodedBody {
            bytes: Vec::new(),
            content_type: None,
        },
        Body::Json { content } => EncodedBody {
            bytes: content.as_bytes().to_vec(),
            content_type: Some("application/json".into()),
        },
        Body::Text {
            content,
            content_type,
        } => EncodedBody {
            bytes: content.as_bytes().to_vec(),
            // 用户可能把 content_type 留空，那就不设头，交给服务端猜。
            content_type: (!content_type.trim().is_empty()).then(|| content_type.clone()),
        },
        Body::UrlEncoded { rows } => {
            let joined = rows
                .iter()
                .filter(|row| row.enabled && !row.key.is_empty())
                .map(|row| {
                    format!(
                        "{}={}",
                        urlencoding_query(&row.key),
                        urlencoding_query(&row.value)
                    )
                })
                .collect::<Vec<_>>()
                .join("&");
            EncodedBody {
                bytes: joined.into_bytes(),
                content_type: Some("application/x-www-form-urlencoded".into()),
            }
        }
        Body::GraphQl {
            query,
            variables,
            operation_name,
        } => {
            // GraphQL over HTTP 就是一个固定形状的 JSON POST。
            // `variables` 是用户敲的原文，解析失败时不能静默丢掉 —— 那会让
            // 「变量没生效」变成一个查不出原因的问题，所以明确报错。
            let vars: serde_json::Value = if variables.trim().is_empty() {
                serde_json::Value::Object(Default::default())
            } else {
                serde_json::from_str(variables).map_err(|error| {
                    AppError::new(ErrorKind::Parse, "GraphQL variables 不是合法 JSON")
                        .detail(error.to_string())
                        .hint(Hint::new("检查 Variables 面板里的 JSON 语法"))
                })?
            };
            let mut payload = serde_json::json!({ "query": query, "variables": vars });
            if let Some(name) = operation_name.as_ref().filter(|n| !n.trim().is_empty()) {
                payload["operationName"] = serde_json::Value::String(name.clone());
            }
            EncodedBody {
                bytes: serde_json::to_vec(&payload).map_err(|error| {
                    AppError::new(ErrorKind::Parse, "构造 GraphQL 请求体失败")
                        .detail(error.to_string())
                })?,
                content_type: Some("application/json".into()),
            }
        }
        Body::FormData { .. } => {
            return Err(AppError::new(
                ErrorKind::Parse,
                "FormData 必须通过流式请求构造",
            ))
        }
        Body::Binary { .. } => {
            return Err(AppError::new(
                ErrorKind::Parse,
                "Binary 必须通过流式请求构造",
            ))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{KeyValue, ValueKind};

    fn kv(key: &str, value: &str, enabled: bool) -> KeyValue {
        KeyValue {
            enabled,
            key: key.into(),
            value: value.into(),
            description: None,
            kind: ValueKind::Text,
        }
    }

    fn spec(url: &str) -> RequestSpec {
        RequestSpec {
            method: "GET".into(),
            url: url.into(),
            params: Vec::new(),
            path_params: Vec::new(),
            headers: Vec::new(),
            body: Body::None,
            auth: Default::default(),
            settings: Default::default(),
        }
    }

    #[test]
    fn bare_host_gets_https_not_http() {
        // 用户在地址栏打 `api.example.com/users` 是极常见输入。
        // 补 https 而非 http：默认加密，降级必须显式写。
        let got = build_url(&spec("api.example.com/users")).unwrap();
        assert_eq!(got, "https://api.example.com/users");
    }

    #[test]
    fn existing_scheme_is_left_alone() {
        for url in ["http://localhost:8080/x", "https://a.test/y"] {
            assert_eq!(build_url(&spec(url)).unwrap(), url);
        }
    }

    #[test]
    fn empty_url_is_actionable_error() {
        let error = build_url(&spec("   ")).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Parse);
        // 空 hints 是设计缺陷（error.rs 的约定）
        assert!(!error.hints.is_empty());
    }

    #[test]
    fn disabled_params_are_omitted() {
        let mut s = spec("https://a.test/x");
        s.params = vec![kv("keep", "1", true), kv("drop", "2", false)];
        let got = build_url(&s).unwrap();
        assert_eq!(got, "https://a.test/x?keep=1");
    }

    #[test]
    fn url_with_existing_query_appends_with_ampersand() {
        // 粘贴过来的 URL 常常已带 query。用 `?` 接会产生两个 `?`，
        // 服务端把第二个当成参数值的一部分 —— 症状是「参数莫名其妙丢了」。
        let mut s = spec("https://a.test/x?already=1");
        s.params = vec![kv("added", "2", true)];
        assert_eq!(build_url(&s).unwrap(), "https://a.test/x?already=1&added=2");
    }

    #[test]
    fn trailing_question_mark_counts_as_existing_query() {
        let mut s = spec("https://a.test/x?");
        s.params = vec![kv("a", "1", true)];
        assert_eq!(build_url(&s).unwrap(), "https://a.test/x?&a=1");
    }

    #[test]
    fn param_values_are_percent_encoded() {
        let mut s = spec("https://a.test/s");
        s.params = vec![kv("q", "hello world&x=1", true)];
        let got = build_url(&s).unwrap();
        assert_eq!(got, "https://a.test/s?q=hello%20world%26x%3D1");
    }

    #[test]
    fn path_params_substitute_and_keep_slashes() {
        let mut s = spec("https://a.test/users/:id/repos/:name");
        s.path_params = vec![kv("id", "42", true), kv("name", "org/repo", true)];
        // path 分量保留 `/`，否则 `org/repo` 会变成 org%2Frepo 而打断路径结构
        assert_eq!(
            build_url(&s).unwrap(),
            "https://a.test/users/42/repos/org/repo"
        );
    }

    #[test]
    fn json_body_sets_content_type_and_passes_raw() {
        // 刻意不校验 JSON：调试时经常要故意发不合法的 JSON 看服务端反应。
        let got = encode_body(&Body::Json {
            content: "{not valid".into(),
        })
        .unwrap();
        assert_eq!(got.bytes, b"{not valid");
        assert_eq!(got.content_type.as_deref(), Some("application/json"));
    }

    #[test]
    fn none_body_sets_no_content_type() {
        // 发一个带空 Content-Type 的 GET 会让某些服务端返回 400，
        // 所以这里必须是 None（不设头）而不是空串。
        let got = encode_body(&Body::None).unwrap();
        assert!(got.bytes.is_empty());
        assert_eq!(got.content_type, None);
    }

    #[test]
    fn text_body_with_blank_content_type_omits_header() {
        let got = encode_body(&Body::Text {
            content: "hi".into(),
            content_type: "  ".into(),
        })
        .unwrap();
        assert_eq!(got.content_type, None);
    }

    #[test]
    fn urlencoded_skips_disabled_and_encodes() {
        let got = encode_body(&Body::UrlEncoded {
            rows: vec![
                kv("a", "1 2", true),
                kv("skip", "x", false),
                kv("", "y", true),
            ],
        })
        .unwrap();
        assert_eq!(String::from_utf8(got.bytes).unwrap(), "a=1%202");
        assert_eq!(
            got.content_type.as_deref(),
            Some("application/x-www-form-urlencoded")
        );
    }

    #[test]
    fn graphql_bad_variables_is_reported_not_swallowed() {
        // 静默丢掉解析失败的 variables 会让「变量没生效」变成查不出原因的问题。
        let error = encode_body(&Body::GraphQl {
            query: "{ me }".into(),
            variables: "{oops".into(),
            operation_name: None,
        })
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Parse);
        assert!(!error.hints.is_empty());
    }

    #[test]
    fn graphql_empty_variables_becomes_empty_object() {
        let got = encode_body(&Body::GraphQl {
            query: "{ me }".into(),
            variables: "".into(),
            operation_name: Some("Me".into()),
        })
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&got.bytes).unwrap();
        assert_eq!(parsed["variables"], serde_json::json!({}));
        assert_eq!(parsed["operationName"], "Me");
    }

    #[test]
    fn file_bodies_build_streaming_requests() {
        let path = std::env::temp_dir().join(format!("apogee-upload-{}", Uuid::new_v4()));
        std::fs::write(&path, b"stream me").unwrap();

        let mut binary = spec("https://a.test/upload");
        binary.method = "POST".into();
        binary.body = Body::Binary { path: path.clone() };
        let request = built(&binary);
        assert_eq!(header_values(&request, "content-length"), vec!["9"]);
        assert!(request.body().is_some());
        drop(request);

        let mut multipart = spec("https://a.test/upload");
        multipart.method = "POST".into();
        multipart.body = Body::FormData {
            rows: vec![KeyValue {
                enabled: true,
                key: "attachment".into(),
                value: path.to_string_lossy().into_owned(),
                description: None,
                kind: ValueKind::File,
            }],
        };
        let request = built(&multipart);
        assert!(header_values(&request, "content-type")[0]
            .starts_with("multipart/form-data; boundary="));
        assert!(request.body().is_some());
        drop(request);
        std::fs::remove_file(path).unwrap();
    }

    // ── 请求组装 ─────────────────────────────────────────────────────
    //
    // 这几条测的是「密钥/头最终去了哪里」。这类错误联网测不出来：请求发得出去、
    // 服务端返回 401 或 400，看起来像对方的问题。

    fn built(spec: &RequestSpec) -> reqwest::Request {
        let client = client_for(&spec.settings.resolve()).expect("建 client");
        build_request(&client, spec)
            .expect("组装请求")
            .0
            .build()
            .expect("落成 Request")
    }

    fn header_values(request: &reqwest::Request, name: &str) -> Vec<String> {
        request
            .headers()
            .get_all(name)
            .iter()
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
            .collect()
    }

    #[test]
    fn api_key_in_query_lands_in_url_not_in_a_header() {
        let mut s = spec("https://a.test/x");
        s.auth = AuthConfig::ApiKey {
            key: "api_key".into(),
            value: "s3cr3t/=".into(),
            location: ApiKeyIn::Query,
        };
        let request = built(&s);
        assert_eq!(request.url().query(), Some("api_key=s3cr3t%2F%3D"));
        // 密钥绝不能同时以同名 header 的形式漏出去
        assert!(header_values(&request, "api_key").is_empty());
    }

    #[test]
    fn api_key_in_query_appends_after_existing_params() {
        let mut s = spec("https://a.test/x?already=1");
        s.params = vec![kv("p", "2", true)];
        s.auth = AuthConfig::ApiKey {
            key: "k".into(),
            value: "v".into(),
            location: ApiKeyIn::Query,
        };
        assert_eq!(
            built(&s).url().as_str(),
            "https://a.test/x?already=1&p=2&k=v"
        );
    }

    #[test]
    fn api_key_in_header_and_cookie_go_where_asked() {
        let mut s = spec("https://a.test/x");
        s.auth = AuthConfig::ApiKey {
            key: "X-Api-Key".into(),
            value: "v".into(),
            location: ApiKeyIn::Header,
        };
        assert_eq!(header_values(&built(&s), "x-api-key"), vec!["v"]);
        assert_eq!(built(&s).url().query(), None);

        s.auth = AuthConfig::ApiKey {
            key: "sid".into(),
            value: "v".into(),
            location: ApiKeyIn::Cookie,
        };
        assert_eq!(header_values(&built(&s), "cookie"), vec!["sid=v"]);
    }

    #[test]
    fn user_authorization_header_replaces_auth_panel_one() {
        // `RequestBuilder::header` 是 append 不是 insert：两次设 Authorization
        // 会把两个头一起发出去，多数服务端只取第一个 —— 症状是「手写了 token
        // 还是 401」。所以鉴权面板必须给手写的头让位。
        for auth in [
            AuthConfig::Bearer {
                token: "from-panel".into(),
            },
            AuthConfig::Basic {
                username: "u".into(),
                password: "p".into(),
            },
        ] {
            let mut s = spec("https://a.test/x");
            s.auth = auth;
            s.headers = vec![kv("Authorization", "Bearer from-user", true)];
            assert_eq!(
                header_values(&built(&s), "authorization"),
                vec!["Bearer from-user"]
            );
        }
    }

    #[test]
    fn auth_panel_still_applies_when_user_wrote_no_header() {
        let mut s = spec("https://a.test/x");
        s.auth = AuthConfig::Bearer { token: "t".into() };
        assert_eq!(header_values(&built(&s), "authorization"), vec!["Bearer t"]);
    }

    #[test]
    fn user_content_type_replaces_body_derived_one() {
        // 两个 Content-Type 是明确非法的请求，不少服务端直接 400。
        let mut s = spec("https://a.test/x");
        s.method = "POST".into();
        s.body = Body::Json {
            content: "{}".into(),
        };
        s.headers = vec![kv("content-type", "application/vnd.api+json", true)];
        assert_eq!(
            header_values(&built(&s), "content-type"),
            vec!["application/vnd.api+json"]
        );
    }

    #[test]
    fn disabled_user_header_does_not_suppress_auth() {
        // 勾掉的头等于不存在，此时鉴权面板应该照常生效。
        let mut s = spec("https://a.test/x");
        s.auth = AuthConfig::Bearer { token: "t".into() };
        s.headers = vec![kv("Authorization", "Bearer disabled", false)];
        assert_eq!(header_values(&built(&s), "authorization"), vec!["Bearer t"]);
    }
}

// ─────────────────────────────────────────────────────────────────────
// Client 池
// ─────────────────────────────────────────────────────────────────────

/// 分池的键。
///
/// 只有会改变**连接行为**的设置才配得上一个独立的池。超时是 per-request 的
/// （见 `client_for` 里为什么不设 builder 级 timeout），所以不进这个键 ——
/// 否则每换一次超时值就白建一个连接池，把 §4.5 的连接复用直接废掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PoolKey {
    verify_tls: bool,
    http_version: HttpVersion,
    follow_redirects: bool,
    max_redirects: u8,
}

static POOL: OnceLock<Mutex<HashMap<PoolKey, reqwest::Client>>> = OnceLock::new();

/// 安装 rustls 的进程级加密后端。**在建任何 `Client` 之前必须调用。**
///
/// reqwest 的 `rustls-no-provider` feature 的含义是「provider 由调用方给」：
/// 它在 `ClientBuilder::build()` 里先看 `CryptoProvider::get_default()`，
/// 取不到就直接 `panic!("No provider set")`
/// （reqwest 0.13 `src/async_impl/client.rs::default_rustls_crypto_provider`，
/// 那条 fallback 只认 `aws-lc-rs`）。
///
/// 关键陷阱：**rustls 自身的 `ring` feature 不足以让 reqwest 找到 provider。**
/// `ring` 只让 `rustls::ClientConfig::builder()` 这条路能用
/// `get_default_or_install_from_crate_features()` 自举；reqwest 走的是
/// `builder_with_provider()`，它不会去碰 crate feature 自举那条逻辑。
/// 漏了这一步的症状是：编译通过、命令注册正常、按钮点下去变「等待中」，
/// 然后进程在 `panic = "abort"` 下直接消失 —— 也就是「闪退」。
///
/// 装不上（别处已经装过）不算错误：进程级默认只需要一个，谁先装谁生效。
fn ensure_crypto_provider() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        if rustls::crypto::ring::default_provider()
            .install_default()
            .is_err()
        {
            tracing::debug!("rustls provider 已由别处安装，跳过");
        }
    });
}

/// 取（或建）一个匹配该配置的 `Client`。
///
/// §4.5 要求连接池复用与 DNS 缓存。每次请求新建 `Client` 会把两者都丢掉，
/// 「请求发送开销 < 5ms」直接打穿。
fn client_for(settings: &ResolvedSettings) -> Result<reqwest::Client, AppError> {
    ensure_crypto_provider();

    let key = PoolKey {
        verify_tls: settings.verify_tls,
        http_version: settings.http_version,
        follow_redirects: settings.follow_redirects,
        max_redirects: settings.max_redirects,
    };

    let pool = POOL.get_or_init(|| Mutex::new(HashMap::new()));
    // 锁只护 HashMap 的查改，不跨越网络调用。`Client` 自身是 Arc 语义，
    // clone 很便宜，所以持锁时间是常数级。
    let mut guard = pool.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(client) = guard.get(&key) {
        return Ok(client.clone());
    }

    let redirect = if settings.follow_redirects {
        reqwest::redirect::Policy::limited(settings.max_redirects as usize)
    } else {
        reqwest::redirect::Policy::none()
    };

    let mut builder = reqwest::Client::builder()
        .redirect(redirect)
        // 刻意**不设** builder 级 timeout：那是「整个请求含 body 下载」的总时限，
        // 会让一个 3 分钟的 500MB 正常下载被判超时。超时改为只覆盖「发出请求到
        // 拿到响应头」，在 execute 里用 tokio::time::timeout 实现。
        //
        // `danger_accept_invalid_certs` 只在用户显式关掉校验时开。默认永远是 true
        // （见 `ResolvedSettings::default`）—— 这个开关是给自签证书的内网环境用的，
        // 不能因为「省事」而默认放开。
        .danger_accept_invalid_certs(!settings.verify_tls);

    builder = match settings.http_version {
        // 只用 HTTP/1.1：不协商升级。某些老网关看到 ALPN 里的 h2 会直接断连。
        HttpVersion::Http1 => builder.http1_only(),
        // 让 ALPN 自己谈。不用 http2_prior_knowledge —— 那会对不支持 h2 的
        // 服务端直接失败，而绝大多数 API 还是 1.1。
        HttpVersion::Http2 => builder,
    };

    let client = builder.build().map_err(|error| {
        AppError::new(ErrorKind::Io, "初始化 HTTP 客户端失败")
            .detail(error.to_string())
            .hint(Hint::new("这通常是 TLS 后端问题，请反馈日志"))
    })?;
    guard.insert(key, client.clone());
    Ok(client)
}

// ─────────────────────────────────────────────────────────────────────
// 执行
// ─────────────────────────────────────────────────────────────────────

/// 下载进度。`total` 来自 `Content-Length`，chunked 编码下没有这个头，
/// 所以是 `Option` —— UI 那时该显示「已收 N MB」而不是一个假的百分比。
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub received: u64,
    pub total: Option<u64>,
}

/// 执行结果。
///
/// 取消**不是**错误，所以单独一个变体。混进 `Err` 会逼前端去猜「这个失败
/// 是用户自己点的取消，还是网络真的断了」，而这两者该有完全不同的 UI。
#[derive(Debug)]
pub enum Outcome {
    Done(Box<ResponseMeta>),
    Cancelled,
    Failed(AppError),
}

/// 发一个请求，body 边收边写进 spool。
///
/// `on_progress` 让调用方决定怎么上报 —— src-tauri 转成 Tauri 事件，CLI 可以
/// 画进度条。核心库因此不必知道 Tauri 的存在（apogee-core 的架构约束）。
pub async fn execute(
    id: Uuid,
    spec: &RequestSpec,
    environment: Option<&Environment>,
    spool: &Spool,
    cancel: tokio::sync::oneshot::Receiver<()>,
    on_progress: impl FnMut(Progress),
) -> Outcome {
    match execute_inner(id, spec, environment, spool, cancel, on_progress).await {
        Ok(Some(meta)) => Outcome::Done(Box::new(meta)),
        Ok(None) => Outcome::Cancelled,
        Err(error) => Outcome::Failed(error),
    }
}

/// `Ok(None)` 表示被取消。用 `Option` 而不是再加一个错误变体，是为了让整条
/// 正常路径继续能用 `?`。
async fn execute_inner(
    id: Uuid,
    spec: &RequestSpec,
    environment: Option<&Environment>,
    spool: &Spool,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
    on_progress: impl FnMut(Progress),
) -> Result<Option<ResponseMeta>, AppError> {
    // 解析在执行层入口完成，保证 URL、参数、Header、Body 和鉴权共享同一份
    // 环境快照。原始 spec 仍保留模板，不会被解析结果覆盖。
    let resolved = resolve_request(spec, environment)?;
    let settings = resolved.settings.resolve();
    let client = client_for(&settings)?;
    let (builder, url) = build_request(&client, &resolved)?;

    let t0 = Instant::now();

    // 超时只覆盖「发出 → 拿到响应头」。理由见 client_for 里那段注释：
    // 把 body 下载算进超时会让正常的大文件下载被误杀。
    let head = tokio::time::timeout(
        std::time::Duration::from_millis(settings.timeout_ms),
        builder.send(),
    );

    // 取消必须能中断**等待中**的连接。请求最常卡在 connect / TTFB 阶段，
    // 那时一个 chunk 都还没到，只在分块之间查标志位是查不到的。
    let response = tokio::select! {
        biased;
        _ = &mut cancel => return Ok(None),
        result = head => match result {
            Err(_elapsed) => {
                return Err(AppError::new(
                    ErrorKind::Timeout,
                    format!("{} ms 内没有收到响应头", settings.timeout_ms),
                )
                .detail(format!("URL: {url}"))
                .hint(Hint::with_action(
                    "调高该请求的超时",
                    "settings.request.timeout",
                ))
                .hint(Hint::new("确认服务地址可达、没被代理或防火墙拦住")))
            }
            Ok(Err(error)) => return Err(map_reqwest_error(&error, &url)),
            Ok(Ok(response)) => response,
        },
    };

    let ttfb_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let status = response.status();
    let http_version = format!("{:?}", response.version());
    let headers = collect_headers(response.headers());
    let (content_type, charset) = parse_content_type(response.headers());
    let declared_len = response.content_length();

    let streamed = stream_to_spool(id, response, spool, declared_len, cancel, on_progress).await?;
    let Some((size_bytes, truncated)) = streamed else {
        return Ok(None);
    };

    let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
    Ok(Some(ResponseMeta {
        id,
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_string(),
        http_version,
        headers,
        size_bytes,
        body_path: spool.path_for(id),
        content_type,
        charset,
        timing: Timing {
            // DNS / TCP / TLS 分解需要 reqwest 的连接级 hook，它没暴露。
            // 这里诚实地留 0，而不是把 ttfb 硬拆成几段假数字 —— 02 文档 §4.4
            // 的时序瀑布图会照这些值画，编出来的分解比空着更有害。
            dns_ms: 0.0,
            tcp_ms: 0.0,
            tls_ms: 0.0,
            request_ms: 0.0,
            ttfb_ms,
            download_ms: (total_ms - ttfb_ms).max(0.0),
            total_ms,
        },
        from_cache: false,
        truncated,
    }))
}

/// 组装一个 reqwest 请求：URL、方法、鉴权、头、body。
///
/// 独立出来是为了能不联网单测 —— 「密钥去了哪个位置」「头有没有重复」这类问题
/// 靠联网测是测不出来的，而它们恰恰是最容易静默出错的部分。同时返回最终 URL，
/// 调用方的超时/连接错误信息要用它。
fn build_request(
    client: &reqwest::Client,
    spec: &RequestSpec,
) -> Result<(reqwest::RequestBuilder, String), AppError> {
    let url = build_url(spec)?;
    let method = reqwest::Method::from_bytes(spec.method.trim().as_bytes()).map_err(|_| {
        AppError::new(
            ErrorKind::Parse,
            format!("非法的 HTTP 方法：{}", spec.method),
        )
        .hint(Hint::new("方法名只能含 token 字符，如 GET / POST / PATCH"))
    })?;

    let mut builder = client.request(method, &url);

    // 用户在 Header 面板里显式写的头名。先收集，因为鉴权和 body 的
    // `Content-Type` 都要给它让位 —— 见 `apply_auth` 里关于 append 语义的说明。
    let user_headers: HashSet<String> = spec
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.trim().is_empty())
        .map(|h| h.key.trim().to_ascii_lowercase())
        .collect();

    builder = apply_auth(builder, &spec.auth, &user_headers);

    builder = apply_body(builder, &spec.body, &user_headers)?;
    for header in spec
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.trim().is_empty())
    {
        builder = builder.header(header.key.trim(), &header.value);
    }
    Ok((builder, url))
}

fn open_stream(path: &Path) -> Result<(reqwest::Body, u64), AppError> {
    let file = std::fs::File::open(path).map_err(|error| {
        AppError::new(
            ErrorKind::Io,
            format!("无法打开上传文件：{}", path.display()),
        )
        .detail(error.to_string())
        .hint(Hint::new("检查文件路径、权限以及文件是否仍然存在"))
    })?;
    let length = file
        .metadata()
        .map_err(|error| {
            AppError::new(
                ErrorKind::Io,
                format!("无法读取上传文件信息：{}", path.display()),
            )
            .detail(error.to_string())
        })?
        .len();
    let stream = ReaderStream::new(tokio::fs::File::from_std(file));
    Ok((reqwest::Body::wrap_stream(stream), length))
}

fn apply_body(
    mut builder: reqwest::RequestBuilder,
    body: &Body,
    user_headers: &HashSet<String>,
) -> Result<reqwest::RequestBuilder, AppError> {
    match body {
        Body::Binary { path } => {
            if path.as_os_str().is_empty() {
                return Err(AppError::new(ErrorKind::Parse, "Binary body 尚未选择文件")
                    .hint(Hint::new("在 Body 面板填写要上传的本地文件路径")));
            }
            let (stream, length) = open_stream(path)?;
            Ok(builder
                .body(stream)
                .header(reqwest::header::CONTENT_LENGTH, length))
        }
        Body::FormData { rows } => {
            if user_headers.contains("content-type") {
                return Err(AppError::new(ErrorKind::Parse, "form-data 不能手动设置 Content-Type")
                    .detail("multipart boundary 必须由客户端生成，手写 Content-Type 会导致服务端无法拆分字段。")
                    .hint(Hint::new("禁用 Header 面板中的 Content-Type 后重试")));
            }
            let mut form = reqwest::multipart::Form::new();
            for row in rows
                .iter()
                .filter(|row| row.enabled && !row.key.trim().is_empty())
            {
                match row.kind {
                    crate::model::ValueKind::File => {
                        let path = Path::new(&row.value);
                        let (stream, length) = open_stream(path)?;
                        let file_name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("upload.bin")
                            .to_string();
                        let part = reqwest::multipart::Part::stream_with_length(stream, length)
                            .file_name(file_name);
                        form = form.part(row.key.clone(), part);
                    }
                    _ => form = form.text(row.key.clone(), row.value.clone()),
                }
            }
            Ok(builder.multipart(form))
        }
        _ => {
            let encoded = encode_body(body)?;
            if let Some(content_type) = encoded.content_type.as_deref() {
                if !user_headers.contains("content-type") {
                    builder = builder.header(reqwest::header::CONTENT_TYPE, content_type);
                }
            }
            if !encoded.bytes.is_empty() {
                builder = builder.body(encoded.bytes);
            }
            Ok(builder)
        }
    }
}

/// 把响应体边收边写盘。返回 `None` 表示被取消。
///
/// 用 `chunk().await` 循环而不是 `response.bytes().await`：后者会把整个 body
/// 收进一个 `Vec`，100MB 响应就是 100MB 常驻堆 —— 那正是 §4.3 要避免的。
async fn stream_to_spool(
    id: Uuid,
    mut response: reqwest::Response,
    spool: &Spool,
    declared_len: Option<u64>,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
    mut on_progress: impl FnMut(Progress),
) -> Result<Option<(u64, bool)>, AppError> {
    let mut writer = spool.writer(id).map_err(|error| {
        AppError::new(ErrorKind::Io, "创建响应缓存文件失败")
            .detail(error.to_string())
            .hint(Hint::new("检查临时目录是否可写、磁盘是否已满"))
    })?;
    let mut truncated = false;

    loop {
        let chunk = tokio::select! {
            biased;
            // 取消时直接 return：writer 的 Drop 会清掉 .part 残片
            _ = &mut cancel => return Ok(None),
            next = response.chunk() => next,
        };

        match chunk {
            Ok(Some(bytes)) => {
                // 不设上限的后果是一个无限 chunked 流能把磁盘写满
                if writer.written() + bytes.len() as u64 > MAX_BODY_BYTES {
                    truncated = true;
                    break;
                }
                writer.write(&bytes).map_err(|error| {
                    AppError::new(ErrorKind::Io, "写入响应缓存失败")
                        .detail(error.to_string())
                        .hint(Hint::new("检查磁盘剩余空间"))
                })?;
                on_progress(Progress {
                    received: writer.written(),
                    total: declared_len,
                });
            }
            Ok(None) => break,
            Err(error) => return Err(map_reqwest_error(&error, "响应体读取中断")),
        }
    }

    let size = writer.finish().map_err(|error| {
        AppError::new(ErrorKind::Io, "发布响应缓存失败")
            .detail(error.to_string())
            .hint(Hint::new("检查临时目录权限"))
    })?;
    Ok(Some((size, truncated)))
}

/// 把鉴权配置贴到请求上。
///
/// `Inherit` 在这里等于 `None`：继承链要在执行**之前**解析掉（同 `ResolvedSettings`
/// 的思路）。执行层收到 `Inherit` 说明上游没解析，那就不加头 —— 静默猜一个
/// 上级配置会更糟。
///
/// `user_headers` 是用户在 Header 面板里显式写的头名（小写）。命中的头一律跳过：
/// 手写的头是更强的意图表达。
///
/// **为什么必须显式跳过而不能靠「后写覆盖」**：`RequestBuilder::header` 内部是
/// `HeaderMap::append` 而不是 `insert`。两次设 `Authorization` 的结果是**两个**
/// `Authorization` 头一起发出去，不是后者覆盖前者。多数服务端只取第一个（也就是
/// 鉴权面板那个），少数直接 400 —— 症状是「我明明手写了 token，怎么还是 401」。
fn apply_auth(
    mut builder: reqwest::RequestBuilder,
    auth: &AuthConfig,
    user_headers: &HashSet<String>,
) -> reqwest::RequestBuilder {
    let taken = |name: &str| user_headers.contains(name);

    match auth {
        AuthConfig::Inherit | AuthConfig::None => {}
        AuthConfig::Basic { username, password } if !taken("authorization") => {
            builder = builder.basic_auth(username, Some(password));
        }
        AuthConfig::Bearer { token } if !taken("authorization") => {
            builder = builder.bearer_auth(token);
        }
        AuthConfig::ApiKey {
            key,
            value,
            location,
        } => {
            builder = match location {
                ApiKeyIn::Header if !taken(&key.trim().to_ascii_lowercase()) => {
                    builder.header(key.trim(), value.as_str())
                }
                ApiKeyIn::Cookie if !taken("cookie") => {
                    builder.header(reqwest::header::COOKIE, format!("{key}={value}"))
                }
                // `location: query` 在 `build_url` 里就拼进 URL 了 —— 那里是 query
                // 段唯一的组装点。这里必须是 no-op，否则密钥会同时出现在 URL 和
                // 一个同名 header 上。
                _ => builder,
            };
        }
        // 用户已手写同名头，鉴权面板让位。
        _ => {}
    }
    builder
}

/// 收集响应头。
///
/// 用 `Vec` 而不是 `HashMap`：HTTP 允许同名头重复出现，`Set-Cookie` 就是常态。
/// map 会静默丢掉重复项，而「cookie 少了一个」是极难排查的一类问题。
/// 非 UTF-8 的头值做有损转换而不是丢弃 —— 看到乱码好过看不到这个头存在。
fn collect_headers(map: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    map.iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect()
}

/// 把 `Content-Type` 拆成 MIME 与 charset。
///
/// 分开是因为用途不同：MIME 决定用哪个查看器（JSON 树 / 图片 / 下载），
/// charset 决定怎么解码字节。前端的 `TextDecoder` 需要后者。
fn parse_content_type(map: &reqwest::header::HeaderMap) -> (Option<String>, Option<String>) {
    let Some(raw) = map
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return (None, None);
    };

    let mut parts = raw.split(';');
    let mime = parts
        .next()
        .map(|item| item.trim().to_ascii_lowercase())
        .filter(|item| !item.is_empty());

    let charset = parts.find_map(|part| {
        let (key, value) = part.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let value = value.trim().trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_ascii_lowercase())
    });

    (mime, charset)
}

/// 把 reqwest 的底层错误映射成用户能看懂的可操作错误。
///
/// `context` 是出错时正在干什么，会放进 `detail`，帮用户定位：
/// 「在发送请求时连接被拒」跟「在读取响应体时连接断开」是两种排查方向。
fn map_reqwest_error(error: &reqwest::Error, context: &str) -> AppError {
    // 超时这个细分很重要：如果返回泛化的「网络错误」，用户会去排查 DNS/代理/防火墙，
    // 而真正原因是时限太短。
    if error.is_timeout() {
        return AppError::new(ErrorKind::Timeout, "请求超时")
            .detail(format!("{context}：等待超时"))
            .hint(Hint::with_action("调高超时", "settings.request.timeout"));
    }
    if error.is_connect() {
        return AppError::new(ErrorKind::Connect, "无法建立连接")
            .detail(format!("{context}：{}", error))
            .hint(Hint::new("确认服务地址可达、没被代理或防火墙拦住"))
            .hint(Hint::new("检查 URL 的 host 与端口是否正确"));
    }
    if error.is_redirect() {
        return AppError::new(ErrorKind::Http, "重定向循环或过多")
            .detail(format!("{context}：{}", error))
            .hint(Hint::new("检查重定向链是否形成了环"))
            .hint(Hint::with_action(
                "关闭跟随重定向",
                "request.settings.redirect",
            ));
    }

    // 其余统一落进 Http —— dnstype / tls / io / body 的细分等 Tauri 环境
    // 下能拿到 reqwest 的 source chain 再做。
    let hint = if error.is_body() {
        Hint::new("响应体读取被服务端中断，可能是 Content-Length 不匹配")
    } else if error.is_decode() {
        Hint::new("响应体解码失败，尝试关闭 Accept-Encoding: gzip")
    } else {
        Hint::new("检查网络连接、VPN、代理设置")
    };

    AppError::new(ErrorKind::Http, "网络请求失败")
        .detail(format!("{context}：{error}"))
        .hint(hint)
}
