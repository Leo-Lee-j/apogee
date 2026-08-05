//! 请求模型 —— `send_request` 的输入（03 文档 §3.2 / §5.2）。
//!
//! # 为什么这里的 `RequestSpec` 不等于 03 文档 §5.2 的 `Request`
//!
//! 文档里的 `Request` 是**持久化**形状：带 `id` / `name` / `docs` / `sort_order` /
//! `scripts`，那些字段属于集合树与脚本引擎，对「把一个 HTTP 请求发出去」毫无用处。
//!
//! `RequestSpec` 是**执行**形状：只保留发送真正需要的东西。好处是执行路径不必
//! 等集合、脚本、断言落地就能独立工作和独立测试；日后 `Request` 会把 `RequestSpec`
//! 包起来，而不是反过来。
//!
//! 约定沿用 model.rs：`settings` 里的 `Option` 为 `None` 表示**继承上级**。
//! 但继承在执行前就该被解析掉 —— 见 `ResolvedSettings`。

use crate::model::KeyValue;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 请求体。对应 03 文档 §5.2 的 `Body`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Body {
    #[default]
    None,
    /// 原文直传，不做 JSON 校验 —— 调试时经常要故意发不合法的 JSON 看服务端反应。
    Json { content: String },
    #[serde(rename_all = "camelCase")]
    Text {
        content: String,
        content_type: String,
    },
    /// `kind == File` 时 `value` 是文件路径，由 HTTP 层流式读取。
    FormData { rows: Vec<KeyValue> },
    UrlEncoded { rows: Vec<KeyValue> },
    Binary { path: PathBuf },
    #[serde(rename_all = "camelCase")]
    GraphQl {
        query: String,
        variables: String,
        operation_name: Option<String>,
    },
}

/// 执行用的请求描述。字段顺序与 03 文档 §5.2 的 `Request` 对齐，便于日后包装。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSpec {
    pub method: String,
    /// 已含 `{{变量}}` 与 `:path` 占位符的原始 URL。变量解析发生在执行前。
    pub url: String,
    #[serde(default)]
    pub params: Vec<KeyValue>,
    #[serde(default)]
    pub path_params: Vec<KeyValue>,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub body: Body,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub settings: RequestSettings,
}

/// 执行上下文（03 文档 §3.2 的 `ExecContext`）。
///
/// 变量作用域、Cookie jar、代理配置最终都挂在这里。当前切片接入两项：
/// 标签 id 用于事件路由，环境 id 用于在 Rust 侧读取同一份环境快照并解析请求。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecContext {
    #[serde(default)]
    pub tab_id: Option<String>,
    #[serde(default)]
    pub environment_id: Option<String>,
}

/// `send_request` 的返回值（03 文档 §3.2）。命令**立即**返回它，不等网络。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecHandle {
    /// 同时也是响应 id：句柄一发出，body 将来落在 spool 的哪个文件就定了。
    /// 前端因此不必等 `request:done` 就能把 URL 拼出来。
    pub id: uuid::Uuid,
}

/// 鉴权。`Inherit` 是默认值，与 03 文档 §5.2 一致。
///
/// OAuth2 / JWT / AwsSigV4 需要各自的密码学栈与令牌缓存，不在本切片内；
/// 保留变体是为了让 `serde` 反序列化前端已有的配置时不报错，执行阶段会明确拒绝。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AuthConfig {
    #[default]
    Inherit,
    None,
    Basic { username: String, password: String },
    Bearer { token: String },
    #[serde(rename_all = "camelCase")]
    ApiKey {
        key: String,
        value: String,
        location: ApiKeyIn,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiKeyIn {
    Header,
    Query,
    Cookie,
}

/// `Hash` 是给 `http.rs` 的连接池分池键用的（`PoolKey`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HttpVersion {
    /// 只用 HTTP/1.1，不协商升级
    Http1,
    /// 允许 ALPN 协商到 h2
    Http2,
}

/// 请求级设置。全部 `Option`：`None` = 继承上级（03 文档 §5.2 尾注）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSettings {
    pub timeout_ms: Option<u64>,
    pub follow_redirects: Option<bool>,
    pub max_redirects: Option<u8>,
    pub verify_tls: Option<bool>,
    pub http_version: Option<HttpVersion>,
    pub encode_url: Option<bool>,
    pub send_cookies: Option<bool>,
}

/// 三级继承（集合 → 文件夹 → 请求）**解析之后**的设置。
///
/// 为什么要单独一个类型：执行层不该关心继承。如果直接把 `RequestSettings`
/// 传进去，每个用到超时的地方都要写一次 `.unwrap_or(默认值)`，默认值就会散落
/// 在各处并慢慢漂移。这里收敛成一次解析、一个来源。
#[derive(Debug, Clone, Copy)]
pub struct ResolvedSettings {
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub max_redirects: u8,
    pub verify_tls: bool,
    pub http_version: HttpVersion,
}

/// 默认超时。Postman 默认无限等待，那会让「服务挂了」表现为「客户端卡住」，
/// 30s 是能明确报错的折中值。
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_MAX_REDIRECTS: u8 = 10;

impl Default for ResolvedSettings {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_TIMEOUT_MS,
            follow_redirects: true,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            verify_tls: true,
            http_version: HttpVersion::Http2,
        }
    }
}

impl RequestSettings {
    /// 把 `None`（继承）落到具体值。上级链暂时只有内置默认值一层。
    pub fn resolve(&self) -> ResolvedSettings {
        let base = ResolvedSettings::default();
        ResolvedSettings {
            timeout_ms: self.timeout_ms.unwrap_or(base.timeout_ms),
            follow_redirects: self.follow_redirects.unwrap_or(base.follow_redirects),
            max_redirects: self.max_redirects.unwrap_or(base.max_redirects),
            verify_tls: self.verify_tls.unwrap_or(base.verify_tls),
            http_version: self.http_version.unwrap_or(base.http_version),
        }
    }
}
