//! 对外可见的数据传输对象与默认脱敏规则。
//!
//! `RequestSpec`、`ActualRequest`、`ResponseSummary` 和 `AppError` 是执行层的完整
//! 内部模型，可能包含认证信息、请求体、文件路径或诊断细节。任何会写入 history、
//! 返回给 WebView 或进入普通运行日志的值，都应先转换为本模块的 `Public*` DTO。

use crate::error::{AppError, ErrorKind, Hint};
use crate::model::{ActualRequest, KeyValue, RenderTier, Timing, ValueKind};
use crate::request::{HttpVersion, RequestSettings, RequestSpec};
use crate::response::ResponseSummary;
use serde::{Deserialize, Serialize};
use url::{form_urlencoded, Url};
use uuid::Uuid;

/// 已知敏感字段统一使用的展示值。
pub const REDACTED: &str = "[redacted]";
const OMITTED_FILE: &str = "[file omitted]";
const GENERIC_HINT: &str = "请检查请求配置后重试";

/// 可公开显示的参数或 Header 行。
///
/// `description` 故意不出现在公开结构中：它是自由文本，不能作为持久化或 IPC
/// 边界上的可信载体。`kind` 保留以便旧 history 的 `secret` / `file` 行能被再次
/// 识别并净化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicKeyValue {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub kind: PublicValueKind,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PublicValueKind {
    #[default]
    Text,
    File,
    Secret,
}

impl PublicKeyValue {
    /// Re-run sanitization after deserialization. This is intentional: legacy history may have
    /// been written using the internal model, so a newly parsed public DTO cannot be trusted yet.
    pub fn redacted(self) -> Self {
        self.redacted_with_sensitive_names(&[])
    }

    fn redacted_with_sensitive_names(mut self, sensitive_names: &[String]) -> Self {
        self.value = match self.kind {
            PublicValueKind::File => OMITTED_FILE.to_string(),
            PublicValueKind::Secret => REDACTED.to_string(),
            PublicValueKind::Text
                if is_sensitive_name(&self.key)
                    || is_sensitive_value(&self.value)
                    || is_marked_sensitive_name(&self.key, sensitive_names) =>
            {
                REDACTED.to_string()
            }
            PublicValueKind::Text => self.value,
        };
        self
    }

    fn contains_sensitive_value(&self) -> bool {
        matches!(self.kind, PublicValueKind::File | PublicValueKind::Secret)
            || is_sensitive_name(&self.key)
            || is_sensitive_value(&self.value)
    }
}

impl From<&KeyValue> for PublicKeyValue {
    fn from(value: &KeyValue) -> Self {
        Self {
            enabled: value.enabled,
            key: value.key.clone(),
            value: value.value.clone(),
            kind: match value.kind {
                ValueKind::Text => PublicValueKind::Text,
                ValueKind::File => PublicValueKind::File,
                ValueKind::Secret => PublicValueKind::Secret,
            },
        }
        .redacted()
    }
}

/// 请求设置的安全投影。显式代理 URL 属于连接凭据的一部分，不会离开执行层。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicRequestSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirects: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_tls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_version: Option<HttpVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encode_url: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_cookies: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_backoff_ms: Option<u64>,
}

impl From<&RequestSettings> for PublicRequestSettings {
    fn from(value: &RequestSettings) -> Self {
        Self {
            timeout_ms: value.timeout_ms,
            follow_redirects: value.follow_redirects,
            max_redirects: value.max_redirects,
            verify_tls: value.verify_tls,
            http_version: value.http_version,
            encode_url: value.encode_url,
            send_cookies: value.send_cookies,
            retry_count: value.retry_count,
            retry_backoff_ms: value.retry_backoff_ms,
        }
    }
}

/// Redaction-only compatibility metadata parsed from a legacy `auth` object.
///
/// This intentionally has no credential-value field and is never serialized. It only preserves
/// a custom API-key location/name long enough to mask a legacy actual-request snapshot while the
/// old history YAML remains untouched on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct LegacyAuthRedactionSource {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    location: String,
}

impl LegacyAuthRedactionSource {
    fn from_auth(value: &crate::request::AuthConfig) -> Self {
        match value {
            crate::request::AuthConfig::Inherit => Self::default(),
            crate::request::AuthConfig::None => Self {
                kind: "none".to_string(),
                ..Self::default()
            },
            crate::request::AuthConfig::Basic { .. } => Self {
                kind: "basic".to_string(),
                ..Self::default()
            },
            crate::request::AuthConfig::OAuth1 { .. }
            | crate::request::AuthConfig::OAuth2 { .. }
            | crate::request::AuthConfig::Jwt { .. }
            | crate::request::AuthConfig::Bearer { .. } => Self {
                kind: "bearer".to_string(),
                ..Self::default()
            },
            crate::request::AuthConfig::ApiKey { key, location, .. } => Self {
                kind: "apiKey".to_string(),
                key: key.clone(),
                location: match location {
                    crate::request::ApiKeyIn::Header => "header",
                    crate::request::ApiKeyIn::Query => "query",
                    crate::request::ApiKeyIn::Cookie => "cookie",
                }
                .to_string(),
            },
        }
    }
}

/// 可保存/显示的请求预览，而不是可执行请求定义。
///
/// 不含 auth、body 或 proxy URL。历史记录使用该类型，避免未来调用方把完整
/// `RequestSpec` 误当作可公开数据继续持久化或透传。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicRequestPreview {
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<PublicKeyValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_params: Vec<PublicKeyValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<PublicKeyValue>,
    #[serde(default, skip_serializing_if = "is_default_settings")]
    pub settings: PublicRequestSettings,
    /// Legacy input may contain `auth`; retain only its non-secret masking source. The source is
    /// deliberately skipped when serializing new history so auth can never re-enter YAML or IPC.
    #[serde(default, rename = "auth", skip_serializing)]
    legacy_auth_redaction_source: LegacyAuthRedactionSource,
}

fn is_default_settings(value: &PublicRequestSettings) -> bool {
    value == &PublicRequestSettings::default()
}

impl PublicRequestPreview {
    /// Sanitizes a newly deserialized value as well as values produced from internal models.
    pub fn redacted(mut self) -> Self {
        // Read all sources before the public rows mask their values. In particular, a neutral
        // name such as `q` can only be recognized from its `Secret`/`File` annotation.
        let sensitive_sources = SensitiveRequestSources::from(&self);
        self.url = sanitize_url(
            &self.url,
            &sensitive_sources.path_values,
            &sensitive_sources.query_names,
        );
        self.params = self
            .params
            .into_iter()
            .map(|value| value.redacted_with_sensitive_names(&sensitive_sources.query_names))
            .collect();
        self.path_params = self
            .path_params
            .into_iter()
            .map(|value| value.redacted_with_sensitive_names(&sensitive_sources.path_param_names))
            .collect();
        self.headers = self
            .headers
            .into_iter()
            .map(|value| value.redacted_with_sensitive_names(&sensitive_sources.header_names))
            .collect();
        self
    }

    /// Applies the still-raw `Secret` / `File` annotations from a legacy preview to an actual
    /// request snapshot. Call this before [`Self::redacted`] so the original values are available
    /// only for masking and never leave the public boundary.
    pub fn redact_actual_request(
        &self,
        actual_request: PublicActualRequest,
    ) -> PublicActualRequest {
        actual_request.redacted_with_sensitive_sources(&SensitiveRequestSources::from(self))
    }

    /// Applies the still-raw legacy preview annotations to the embedded actual request in a
    /// response summary, while also sanitizing the response headers themselves.
    pub fn redact_response_summary(
        &self,
        response: PublicResponseSummary,
    ) -> PublicResponseSummary {
        response.redacted_with_sensitive_sources(&SensitiveRequestSources::from(self))
    }
}

impl From<&RequestSpec> for PublicRequestPreview {
    fn from(value: &RequestSpec) -> Self {
        // Keep the raw sensitive path values only long enough to remove their occurrence from
        // the URL. `PublicKeyValue::from` masks them immediately, so deriving this list from the
        // public rows would be too late.
        let sensitive_path_values = value
            .path_params
            .iter()
            .filter(|item| {
                matches!(&item.kind, ValueKind::File | ValueKind::Secret)
                    || is_sensitive_name(&item.key)
                    || is_sensitive_value(&item.value)
            })
            .map(|item| item.value.clone())
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>();
        let mut preview = Self {
            method: value.method.clone(),
            url: value.url.clone(),
            params: value.params.iter().map(PublicKeyValue::from).collect(),
            path_params: value.path_params.iter().map(PublicKeyValue::from).collect(),
            headers: value.headers.iter().map(PublicKeyValue::from).collect(),
            settings: PublicRequestSettings::from(&value.settings),
            legacy_auth_redaction_source: LegacyAuthRedactionSource::from_auth(&value.auth),
        }
        .redacted();
        let sensitive_sources = SensitiveRequestSources::from(&preview);
        preview.url = sanitize_url(
            &value.url,
            &sensitive_path_values,
            &sensitive_sources.query_names,
        );
        preview
    }
}

/// 已解析请求的公开投影。完整 body、body 参数和文件路径绝不对外公开。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicActualRequest {
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_params: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub common_header_names: Vec<String>,
}

impl PublicActualRequest {
    pub fn redacted(self) -> Self {
        self.redacted_with_sensitive_sources(&SensitiveRequestSources::default())
    }

    fn redacted_with_sensitive_sources(mut self, sources: &SensitiveRequestSources) -> Self {
        let mut sensitive_path_values = self
            .path_params
            .iter()
            .filter(|(key, value)| {
                is_sensitive_name(key)
                    || is_sensitive_value(value)
                    || is_marked_sensitive_name(key, &sources.path_param_names)
                    || sources.path_values.iter().any(|source| source == value)
            })
            .map(|(_, value)| value.clone())
            .collect::<Vec<_>>();
        sensitive_path_values.extend(sources.path_values.iter().cloned());

        self.url = sanitize_url(&self.url, &sensitive_path_values, &sources.query_names);
        self.path_params = redact_pairs_with_sensitive_sources(
            self.path_params,
            &sources.path_param_names,
            &sources.path_values,
        );
        self.headers =
            redact_pairs_with_sensitive_sources(self.headers, &sources.header_names, &[]);
        self
    }

    fn into_internal(self) -> ActualRequest {
        let value = self.redacted();
        ActualRequest {
            method: value.method,
            url: value.url,
            path_params: value.path_params,
            headers: value.headers,
            common_header_names: value.common_header_names,
            body: None,
            body_params: Vec::new(),
            sensitive_path_param_names: Vec::new(),
            sensitive_path_values: Vec::new(),
            sensitive_query_names: Vec::new(),
            sensitive_header_names: Vec::new(),
        }
    }
}

impl From<&ActualRequest> for PublicActualRequest {
    fn from(value: &ActualRequest) -> Self {
        let sensitive_sources = SensitiveRequestSources {
            path_param_names: value.sensitive_path_param_names.clone(),
            path_values: value.sensitive_path_values.clone(),
            query_names: value.sensitive_query_names.clone(),
            header_names: value.sensitive_header_names.clone(),
        };
        Self {
            method: value.method.clone(),
            url: value.url.clone(),
            path_params: value.path_params.clone(),
            headers: value.headers.clone(),
            common_header_names: value.common_header_names.clone(),
        }
        .redacted_with_sensitive_sources(&sensitive_sources)
    }
}

/// 安全响应摘要。响应体仍只在内部 spool 中，实际请求则使用公开投影。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicResponseSummary {
    #[serde(default)]
    pub id: Uuid,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default = "default_render_tier")]
    pub tier: RenderTier,
    #[serde(default)]
    pub syntax_highlight: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default)]
    pub indexed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_nodes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_bytes: Option<usize>,
    #[serde(default)]
    pub timing: Timing,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_request: Option<PublicActualRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    #[serde(default)]
    pub truncated: bool,
}

fn default_render_tier() -> RenderTier {
    RenderTier::Full
}

impl PublicResponseSummary {
    /// Sanitizes headers and the embedded request after every deserialization.
    pub fn redacted(self) -> Self {
        self.redacted_with_sensitive_sources(&SensitiveRequestSources::default())
    }

    fn redacted_with_sensitive_sources(mut self, sources: &SensitiveRequestSources) -> Self {
        self.headers = redact_pairs(self.headers);
        self.actual_request = self
            .actual_request
            .map(|actual_request| actual_request.redacted_with_sensitive_sources(sources));
        self
    }

    /// Convert only a history-safe summary back to the internal representation needed to restore
    /// its response body into the session spool. This never recreates an internal request body.
    pub fn into_internal_for_history_restore(self) -> ResponseSummary {
        let value = self.redacted();
        ResponseSummary {
            id: value.id,
            size_bytes: value.size_bytes,
            tier: value.tier,
            syntax_highlight: value.syntax_highlight,
            content_type: value.content_type,
            indexed: value.indexed,
            total_nodes: value.total_nodes,
            index_bytes: value.index_bytes,
            timing: value.timing,
            actual_request: value.actual_request.map(PublicActualRequest::into_internal),
            status: value.status,
            status_text: value.status_text,
            http_version: value.http_version,
            headers: value.headers,
            charset: value.charset,
            truncated: value.truncated,
        }
    }
}

impl From<&ResponseSummary> for PublicResponseSummary {
    fn from(value: &ResponseSummary) -> Self {
        Self {
            id: value.id,
            size_bytes: value.size_bytes,
            tier: value.tier,
            syntax_highlight: value.syntax_highlight,
            content_type: value.content_type.clone(),
            indexed: value.indexed,
            total_nodes: value.total_nodes,
            index_bytes: value.index_bytes,
            timing: value.timing,
            actual_request: value.actual_request.as_ref().map(PublicActualRequest::from),
            status: value.status,
            status_text: value.status_text.clone(),
            http_version: value.http_version.clone(),
            headers: value.headers.clone(),
            charset: value.charset.clone(),
            truncated: value.truncated,
        }
        .redacted()
    }
}

/// 可公开显示的错误提示。技术 `detail` 绝不属于该类型。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<PublicHint>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicHint {
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl PublicHint {
    pub fn redacted(mut self) -> Self {
        let label_is_sensitive = contains_sensitive_marker(&self.label);
        self.label = safe_hint_text(&self.label);
        self.action = self.action.filter(|action| {
            !(label_is_sensitive || contains_sensitive_marker(action) || !is_safe_action(action))
        });
        self
    }
}

impl From<&Hint> for PublicHint {
    fn from(value: &Hint) -> Self {
        Self {
            label: value.label.clone(),
            action: value.action.clone(),
        }
        .redacted()
    }
}

impl PublicError {
    /// A generic error for infrastructure failures that must not serialize an arbitrary error
    /// chain into a terminal event.
    pub fn internal() -> Self {
        Self {
            code: Some("INTERNAL".to_string()),
            kind: "io".to_string(),
            message: generic_error_message("io").to_string(),
            hints: vec![PublicHint {
                label: "请重试请求；若问题持续，请检查应用日志".to_string(),
                action: None,
            }],
        }
        .redacted()
    }

    /// Ensure legacy/error input data cannot retain a detail-like user controlled message.
    pub fn redacted(mut self) -> Self {
        self.kind = canonical_error_kind(&self.kind).to_string();
        self.code = self.code.as_deref().and_then(sanitize_error_code);
        self.message = generic_error_message(&self.kind).to_string();
        self.hints = self.hints.into_iter().map(PublicHint::redacted).collect();
        self
    }
}

impl From<&AppError> for PublicError {
    fn from(value: &AppError) -> Self {
        Self {
            code: value.code.clone(),
            kind: error_kind_name(value.kind).to_string(),
            message: value.message.clone(),
            hints: value.hints.iter().map(PublicHint::from).collect(),
        }
        .redacted()
    }
}

#[derive(Debug, Default)]
struct SensitiveRequestSources {
    path_param_names: Vec<String>,
    path_values: Vec<String>,
    query_names: Vec<String>,
    header_names: Vec<String>,
}

impl SensitiveRequestSources {
    fn push_name(names: &mut Vec<String>, name: &str) {
        let name = name.trim();
        if !name.is_empty() && !is_marked_sensitive_name(name, names) {
            names.push(name.to_string());
        }
    }

    fn add_legacy_auth(&mut self, auth: &LegacyAuthRedactionSource) {
        if ["basic", "bearer", "oauth1", "oauth2", "jwt"]
            .iter()
            .any(|kind| auth.kind.eq_ignore_ascii_case(kind))
        {
            Self::push_name(&mut self.header_names, "authorization");
            return;
        }
        if !auth.kind.eq_ignore_ascii_case("apiKey") {
            return;
        }

        if auth.location.eq_ignore_ascii_case("header") {
            Self::push_name(&mut self.header_names, &auth.key);
        } else if auth.location.eq_ignore_ascii_case("query") {
            Self::push_name(&mut self.query_names, &auth.key);
        } else if auth.location.eq_ignore_ascii_case("cookie") {
            Self::push_name(&mut self.header_names, "cookie");
        }
    }
}

impl From<&PublicRequestPreview> for SensitiveRequestSources {
    fn from(value: &PublicRequestPreview) -> Self {
        let mut sources = Self {
            path_param_names: sensitive_names(&value.path_params),
            path_values: sensitive_values(&value.path_params),
            query_names: sensitive_names(&value.params),
            header_names: sensitive_names(&value.headers),
        };
        sources.add_legacy_auth(&value.legacy_auth_redaction_source);
        sources
    }
}

fn sensitive_names(values: &[PublicKeyValue]) -> Vec<String> {
    let mut names = Vec::new();
    for value in values.iter().filter(|value| {
        value.enabled && !value.key.trim().is_empty() && value.contains_sensitive_value()
    }) {
        if !is_marked_sensitive_name(&value.key, &names) {
            names.push(value.key.clone());
        }
    }
    names
}

fn sensitive_values(values: &[PublicKeyValue]) -> Vec<String> {
    values
        .iter()
        .filter(|value| value.contains_sensitive_value())
        .map(|value| value.value.clone())
        .filter(|value| !value.is_empty() && value != REDACTED && value != OMITTED_FILE)
        .collect()
}

fn redact_pairs(values: Vec<(String, String)>) -> Vec<(String, String)> {
    redact_pairs_with_sensitive_sources(values, &[], &[])
}

fn redact_pairs_with_sensitive_sources(
    values: Vec<(String, String)>,
    sensitive_names: &[String],
    sensitive_values: &[String],
) -> Vec<(String, String)> {
    values
        .into_iter()
        .map(|(key, value)| {
            let value = if is_sensitive_name(&key)
                || is_sensitive_value(&value)
                || is_marked_sensitive_name(&key, sensitive_names)
                || sensitive_values.iter().any(|source| source == &value)
            {
                REDACTED.to_string()
            } else {
                value
            };
            (key, value)
        })
        .collect()
}

fn is_marked_sensitive_name(name: &str, sensitive_names: &[String]) -> bool {
    sensitive_names
        .iter()
        .any(|sensitive_name| sensitive_name.eq_ignore_ascii_case(name))
}

fn sanitize_url(
    value: &str,
    sensitive_path_values: &[String],
    sensitive_query_names: &[String],
) -> String {
    let value = redact_literal_values(value, sensitive_path_values);
    let Ok(mut parsed) = Url::parse(&value) else {
        return sanitize_unparseable_url(&value, sensitive_query_names);
    };

    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_fragment(None);

    if parsed.query().is_some() {
        let pairs = parsed
            .query_pairs()
            .map(|(key, value)| {
                let value = if is_sensitive_name(&key)
                    || is_marked_sensitive_name(&key, sensitive_query_names)
                    || is_sensitive_value(&value)
                {
                    REDACTED.to_string()
                } else {
                    value.into_owned()
                };
                (key.into_owned(), value)
            })
            .collect::<Vec<_>>();
        let mut query = parsed.query_pairs_mut();
        query.clear();
        for (key, value) in pairs {
            query.append_pair(&key, &value);
        }
    }

    parsed.to_string()
}

fn percent_encoded_path_matches(candidate: &[u8], encoded: &[u8]) -> bool {
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'%' {
            if index + 2 >= encoded.len()
                || candidate[index] != b'%'
                || !candidate[index + 1].eq_ignore_ascii_case(&encoded[index + 1])
                || !candidate[index + 2].eq_ignore_ascii_case(&encoded[index + 2])
            {
                return false;
            }
            index += 3;
        } else if candidate[index] != encoded[index] {
            return false;
        } else {
            index += 1;
        }
    }
    true
}

/// Redact a path value that appears in a URL with lowercase or uppercase percent escapes.
/// Unescaped bytes remain exact-match-only, so this does not broaden masking by case-folding a
/// normal path segment.
fn redact_percent_encoded_path_value(value: String, encoded: &str) -> String {
    if !encoded.as_bytes().contains(&b'%') {
        return value;
    }

    let encoded = encoded.as_bytes();
    let mut cursor = 0;
    let mut redacted = String::new();
    while let Some(relative) = value.as_bytes()[cursor..]
        .windows(encoded.len())
        .position(|candidate| percent_encoded_path_matches(candidate, encoded))
    {
        let index = cursor + relative;
        redacted.push_str(&value[cursor..index]);
        redacted.push_str(REDACTED);
        cursor = index + encoded.len();
    }

    if redacted.is_empty() {
        value
    } else {
        redacted.push_str(&value[cursor..]);
        redacted
    }
}

fn redact_literal_values(value: &str, sensitive_values: &[String]) -> String {
    let mut ordered = sensitive_values
        .iter()
        .map(String::as_str)
        .filter(|sensitive| {
            !sensitive.is_empty() && *sensitive != REDACTED && *sensitive != OMITTED_FILE
        })
        .collect::<Vec<_>>();
    ordered.sort_by_key(|sensitive| std::cmp::Reverse(sensitive.len()));

    ordered
        .into_iter()
        .fold(value.to_string(), |url, sensitive| {
            let url = url.replace(sensitive, REDACTED);
            let encoded = crate::http::urlencoding_path(sensitive);
            if encoded == sensitive {
                url
            } else {
                redact_percent_encoded_path_value(url, &encoded)
            }
        })
}

fn sanitize_unparseable_url(value: &str, sensitive_query_names: &[String]) -> String {
    let value = value.split('#').next().unwrap_or_default();
    let (base, query) = match value.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (value, None),
    };
    let base = strip_url_userinfo(base);
    let Some(query) = query else {
        return base;
    };
    let query = query
        .split('&')
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let decoded_key = form_urlencoded::parse(format!("{key}=").as_bytes())
                .next()
                .map(|(key, _)| key.into_owned())
                .unwrap_or_else(|| key.to_string());
            if is_sensitive_name(&decoded_key)
                || is_marked_sensitive_name(&decoded_key, sensitive_query_names)
                || is_sensitive_value(value)
            {
                format!("{key}={REDACTED}")
            } else {
                pair.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

fn strip_url_userinfo(value: &str) -> String {
    let Some(scheme_end) = value.find("://") else {
        return value.to_string();
    };
    let authority_start = scheme_end + 3;
    let authority_and_path = &value[authority_start..];
    let split_at = authority_and_path
        .find('/')
        .unwrap_or(authority_and_path.len());
    let (authority, suffix) = authority_and_path.split_at(split_at);
    let Some(at) = authority.rfind('@') else {
        return value.to_string();
    };
    format!(
        "{}{}{}",
        &value[..authority_start],
        &authority[at + 1..],
        suffix
    )
}

pub(crate) fn is_sensitive_name(name: &str) -> bool {
    let normalized = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if normalized.is_empty() {
        return false;
    }

    matches!(
        normalized.as_str(),
        "authorization"
            | "proxyauthorization"
            | "cookie"
            | "setcookie"
            | "apikey"
            | "xapikey"
            | "accesskey"
            | "secret"
            | "clientsecret"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "password"
            | "passwd"
            | "pwd"
            | "signature"
            | "sig"
            | "credential"
            | "credentials"
            | "bearer"
            | "privatekey"
            | "session"
            | "sessionid"
            | "key"
    ) || [
        "apikey",
        "token",
        "secret",
        "password",
        "signature",
        "credential",
        "authorization",
        "cookie",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

pub(crate) fn is_sensitive_value(value: &str) -> bool {
    let value = value.trim_start().to_ascii_lowercase();
    value.starts_with("bearer ")
        || value.starts_with("basic ")
        || value.starts_with("token ")
        || value.starts_with("apikey ")
        || value.starts_with("api-key ")
}

fn contains_sensitive_marker(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "bearer ",
        "basic ",
        "token",
        "secret",
        "password",
        "authorization",
        "cookie",
        "api key",
        "api_key",
        "apikey",
        "signature",
        "credential",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

fn safe_hint_text(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() || contains_sensitive_marker(value) {
        return GENERIC_HINT.to_string();
    }
    truncate(value, 240)
}

fn is_safe_action(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn sanitize_error_code(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        None
    } else {
        Some(value.to_string())
    }
}

fn canonical_error_kind(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "dns" => "dns",
        "connect" => "connect",
        "tls" => "tls",
        "timeout" => "timeout",
        "http" => "http",
        "parse" => "parse",
        "script" => "script",
        "var" => "var",
        "io" | "internal" => "io",
        _ => "io",
    }
}

fn error_kind_name(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Dns => "dns",
        ErrorKind::Connect => "connect",
        ErrorKind::Tls => "tls",
        ErrorKind::Timeout => "timeout",
        ErrorKind::Http => "http",
        ErrorKind::Io => "io",
        ErrorKind::Parse => "parse",
        ErrorKind::Script => "script",
        ErrorKind::Var => "var",
    }
}

fn generic_error_message(kind: &str) -> &'static str {
    match kind {
        "dns" => "无法解析服务器地址",
        "connect" => "无法连接到服务器",
        "tls" => "TLS 连接失败",
        "timeout" => "请求超时",
        "http" => "HTTP 请求失败",
        "parse" => "响应解析失败",
        "script" => "请求脚本执行失败",
        "var" => "请求变量解析失败",
        _ => "请求执行失败，请重试",
    }
}

fn truncate(value: &str, maximum_chars: usize) -> String {
    if value.chars().count() <= maximum_chars {
        value.to_string()
    } else {
        format!("{}…", value.chars().take(maximum_chars).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{ApiKeyIn, AuthConfig, Body};
    use std::path::PathBuf;

    fn key_value(key: &str, value: &str, kind: ValueKind) -> KeyValue {
        KeyValue {
            enabled: true,
            key: key.to_string(),
            value: value.to_string(),
            description: Some("自由文本也不属于公开 DTO".to_string()),
            kind,
        }
    }

    fn secret_request_spec(body: Body) -> RequestSpec {
        RequestSpec {
            method: "POST".to_string(),
            url: "https://alice:password@example.test/v1/secret-path?api_key=query-secret&safe=ok#private".to_string(),
            params: vec![
                key_value("page", "1", ValueKind::Text),
                key_value("token", "param-secret", ValueKind::Text),
                key_value("masked", "kind-secret", ValueKind::Secret),
            ],
            path_params: vec![key_value("secret", "secret-path", ValueKind::Text)],
            headers: vec![
                key_value("Authorization", "Bearer header-secret", ValueKind::Text),
                key_value("X-Trace", "safe", ValueKind::Text),
                key_value("X-File", "C:\\private\\upload.bin", ValueKind::File),
            ],
            body,
            auth: AuthConfig::ApiKey {
                key: "X-API-Key".to_string(),
                value: "auth-secret".to_string(),
                location: ApiKeyIn::Header,
            },
            settings: RequestSettings {
                proxy_url: Some("http://proxy-user:proxy-secret@proxy.test:8080".to_string()),
                ..Default::default()
            },
        }
    }

    fn assert_absent(value: &str, secrets: &[&str]) {
        for secret in secrets {
            assert!(
                !value.contains(secret),
                "public DTO unexpectedly contained {secret:?}: {value}"
            );
        }
    }

    #[test]
    fn new_auth_and_legacy_auth_never_enter_public_history() {
        for kind in ["oauth1", "oauth2", "jwt"] {
            let value = serde_json::json!({"method":"GET","url":"https://example.test/", "auth":{"type":kind,
                "consumerSecret":"private-secret","accessToken":"private-secret","secret":"private-secret","payload":"private-claims"},
                "headers":[{"key":"Authorization","value":"manual-private","enabled":true,"kind":"text"}]});
            let preview: PublicRequestPreview = serde_json::from_value(value).unwrap();
            let preview = preview.redacted();
            assert_eq!(preview.headers[0].value, REDACTED);
            assert_absent(
                &serde_json::to_string(&preview).unwrap(),
                &["private-secret", "private-claims", "manual-private"],
            );
        }
    }

    #[test]
    fn request_preview_omits_every_body_and_auth_variant() {
        let bodies = vec![
            Body::Json {
                content: "json-body-secret".to_string(),
            },
            Body::Text {
                content: "text-body-secret".to_string(),
                content_type: "text/plain".to_string(),
            },
            Body::FormData {
                rows: vec![key_value(
                    "upload",
                    "C:\\private\\form.bin",
                    ValueKind::File,
                )],
            },
            Body::UrlEncoded {
                rows: vec![key_value("password", "form-secret", ValueKind::Text)],
            },
            Body::Binary {
                path: PathBuf::from("C:\\private\\binary-secret.bin"),
            },
            Body::GraphQl {
                query: "query { privateSecret }".to_string(),
                variables: "{\"token\":\"graphql-secret\"}".to_string(),
                operation_name: Some("privateOperation".to_string()),
            },
        ];

        for body in bodies {
            let preview = PublicRequestPreview::from(&secret_request_spec(body));
            let encoded = serde_json::to_string(&preview).unwrap();
            assert_eq!(
                preview.url,
                "https://example.test/v1/[redacted]?api_key=%5Bredacted%5D&safe=ok"
            );
            assert_eq!(preview.params[0].value, "1");
            assert_eq!(preview.params[1].value, REDACTED);
            assert_eq!(preview.params[2].value, REDACTED);
            assert_eq!(preview.headers[0].value, REDACTED);
            assert_eq!(preview.headers[1].value, "safe");
            assert_eq!(preview.headers[2].value, OMITTED_FILE);
            assert_absent(
                &encoded,
                &[
                    "password",
                    "query-secret",
                    "param-secret",
                    "kind-secret",
                    "header-secret",
                    "auth-secret",
                    "proxy-secret",
                    "json-body-secret",
                    "text-body-secret",
                    "form-secret",
                    "graphql-secret",
                    "C:\\private",
                    "privateOperation",
                ],
            );
            assert!(!encoded.contains("\"body\""));
            assert!(!encoded.contains("\"auth\""));
            assert!(!encoded.contains("proxyUrl"));
        }
    }

    #[test]
    fn actual_request_and_response_headers_are_redacted() {
        let actual = ActualRequest {
            method: "POST".to_string(),
            url: "https://alice:password@example.test/users/path-secret?token=query-secret&safe=ok"
                .to_string(),
            path_params: vec![("secret".to_string(), "path-secret".to_string())],
            headers: vec![
                (
                    "Authorization".to_string(),
                    "Basic base64-secret".to_string(),
                ),
                ("Cookie".to_string(), "session=cookie-secret".to_string()),
                ("Accept".to_string(), "application/json".to_string()),
            ],
            common_header_names: vec!["Accept".to_string()],
            body: Some("actual-body-secret".to_string()),
            body_params: vec![("password".to_string(), "body-param-secret".to_string())],
            sensitive_path_param_names: Vec::new(),
            sensitive_path_values: Vec::new(),
            sensitive_query_names: Vec::new(),
            sensitive_header_names: Vec::new(),
        };
        let mut response = ResponseSummary::without_http(Uuid::new_v4(), 3, Timing::default());
        response.actual_request = Some(actual);
        response.headers = vec![
            (
                "Set-Cookie".to_string(),
                "session=response-cookie-secret".to_string(),
            ),
            ("X-Request-Id".to_string(), "safe-response".to_string()),
        ];

        let public = PublicResponseSummary::from(&response);
        let encoded = serde_json::to_string(&public).unwrap();
        let actual = public.actual_request.as_ref().unwrap();
        assert_eq!(
            actual.url,
            "https://example.test/users/[redacted]?token=%5Bredacted%5D&safe=ok"
        );
        assert_eq!(actual.path_params[0].1, REDACTED);
        assert_eq!(actual.headers[0].1, REDACTED);
        assert_eq!(actual.headers[1].1, REDACTED);
        assert_eq!(actual.headers[2].1, "application/json");
        assert_eq!(public.headers[0].1, REDACTED);
        assert_eq!(public.headers[1].1, "safe-response");
        assert_absent(
            &encoded,
            &[
                "password",
                "path-secret",
                "query-secret",
                "base64-secret",
                "cookie-secret",
                "actual-body-secret",
                "body-param-secret",
                "response-cookie-secret",
            ],
        );
        assert!(!encoded.contains("bodyParams"));
    }

    #[test]
    fn actual_request_metadata_masks_neutral_names_and_encoded_paths() {
        let actual = ActualRequest {
            method: "GET".to_string(),
            url: "https://example.test/users/a%20b?q=query-secret&safe=ok".to_string(),
            path_params: vec![("id".to_string(), "a b".to_string())],
            headers: vec![
                ("X-My-Key".to_string(), "header-secret".to_string()),
                ("X-Trace".to_string(), "safe-header".to_string()),
            ],
            common_header_names: Vec::new(),
            body: None,
            body_params: Vec::new(),
            sensitive_path_param_names: vec!["id".to_string()],
            sensitive_path_values: vec!["a b".to_string()],
            sensitive_query_names: vec!["q".to_string()],
            sensitive_header_names: vec!["X-My-Key".to_string()],
        };

        let public = PublicActualRequest::from(&actual);
        let encoded = serde_json::to_string(&public).unwrap();
        assert_eq!(
            public.url,
            "https://example.test/users/[redacted]?q=%5Bredacted%5D&safe=ok"
        );
        assert_eq!(public.path_params[0].1, REDACTED);
        assert_eq!(public.headers[0].1, REDACTED);
        assert_eq!(public.headers[1].1, "safe-header");
        assert_absent(&encoded, &["a b", "a%20b", "query-secret", "header-secret"]);
    }

    #[test]
    fn public_preview_and_actual_request_mask_lowercase_percent_encoded_paths() {
        let actual = ActualRequest {
            method: "GET".to_string(),
            url: "https://example.test/users/%c3%a9".to_string(),
            path_params: vec![("id".to_string(), "é".to_string())],
            headers: Vec::new(),
            common_header_names: Vec::new(),
            body: None,
            body_params: Vec::new(),
            sensitive_path_param_names: vec!["id".to_string()],
            sensitive_path_values: vec!["é".to_string()],
            sensitive_query_names: Vec::new(),
            sensitive_header_names: Vec::new(),
        };
        let public_actual = PublicActualRequest::from(&actual);
        assert_eq!(public_actual.url, "https://example.test/users/[redacted]");

        let legacy: PublicRequestPreview = serde_json::from_str(
            r#"{
                "method":"GET",
                "url":"https://example.test/users/%c3%a9",
                "pathParams":[{"enabled":true,"key":"id","value":"é","kind":"secret"}]
            }"#,
        )
        .unwrap();
        let public_preview = legacy.redacted();
        assert_eq!(public_preview.url, "https://example.test/users/[redacted]");
        assert_absent(
            &serde_json::to_string(&(public_actual, public_preview)).unwrap(),
            &["é", "%c3%a9"],
        );
    }

    #[test]
    fn request_preview_masks_neutral_secret_query_names_before_and_after_deserialization() {
        let mut current_spec = secret_request_spec(Body::None);
        current_spec.url = "https://example.test/search?q=current-query-secret&safe=ok".to_string();
        current_spec.params = vec![key_value("q", "current-query-secret", ValueKind::Secret)];
        let current = PublicRequestPreview::from(&current_spec);
        assert_eq!(
            current.url,
            "https://example.test/search?q=%5Bredacted%5D&safe=ok"
        );
        assert_absent(
            &serde_json::to_string(&current).unwrap(),
            &["current-query-secret"],
        );

        let legacy: PublicRequestPreview = serde_json::from_str(
            r#"{
                "method":"GET",
                "url":"https://example.test/search?q=legacy-query-secret&safe=ok",
                "params":[{"enabled":true,"key":"q","value":"legacy-query-secret","kind":"secret"}]
            }"#,
        )
        .unwrap();
        let legacy = legacy.redacted();
        assert_eq!(
            legacy.url,
            "https://example.test/search?q=%5Bredacted%5D&safe=ok"
        );
        assert_absent(
            &serde_json::to_string(&legacy).unwrap(),
            &["legacy-query-secret"],
        );
    }

    #[test]
    fn request_preview_uses_api_key_header_source_to_mask_manual_header() {
        let mut spec = secret_request_spec(Body::None);
        spec.headers = vec![key_value(
            "X-My-Key",
            "manual-header-secret",
            ValueKind::Text,
        )];
        spec.auth = AuthConfig::ApiKey {
            key: "X-My-Key".to_string(),
            value: "configured-api-key-secret".to_string(),
            location: ApiKeyIn::Header,
        };

        let preview = PublicRequestPreview::from(&spec);
        assert_eq!(preview.headers[0].value, REDACTED);
        assert_absent(
            &serde_json::to_string(&preview).unwrap(),
            &["manual-header-secret", "configured-api-key-secret"],
        );
    }

    #[test]
    fn legacy_secret_rows_redact_neutral_actual_and_response_snapshots() {
        let preview = PublicRequestPreview {
            method: "GET".to_string(),
            url: "https://example.test/users/:id".to_string(),
            params: vec![PublicKeyValue {
                enabled: true,
                key: "q".to_string(),
                value: "legacy-query-secret".to_string(),
                kind: PublicValueKind::Secret,
            }],
            path_params: vec![PublicKeyValue {
                enabled: true,
                key: "id".to_string(),
                value: "legacy path secret".to_string(),
                kind: PublicValueKind::Secret,
            }],
            headers: vec![PublicKeyValue {
                enabled: true,
                key: "X-My-Key".to_string(),
                value: "legacy-header-secret".to_string(),
                kind: PublicValueKind::Secret,
            }],
            settings: PublicRequestSettings::default(),
            ..Default::default()
        };
        let actual = PublicActualRequest {
            method: "GET".to_string(),
            url: "https://example.test/users/legacy%20path%20secret?q=legacy-query-secret"
                .to_string(),
            path_params: vec![("id".to_string(), "legacy path secret".to_string())],
            headers: vec![("X-My-Key".to_string(), "legacy-header-secret".to_string())],
            common_header_names: Vec::new(),
        };

        let redacted_actual = preview.redact_actual_request(actual.clone());
        let mut legacy_response = PublicResponseSummary::from(&ResponseSummary::without_http(
            Uuid::new_v4(),
            0,
            Timing::default(),
        ));
        legacy_response.actual_request = Some(actual);
        let redacted_response = preview.redact_response_summary(legacy_response);
        let encoded = serde_json::to_string(&(redacted_actual, redacted_response)).unwrap();
        assert_absent(
            &encoded,
            &[
                "legacy path secret",
                "legacy%20path%20secret",
                "legacy-query-secret",
                "legacy-header-secret",
            ],
        );
    }

    #[test]
    fn public_error_discards_detail_and_uses_safe_message() {
        let error = AppError::new(
            ErrorKind::Connect,
            "连接 https://alice:password@example.test 失败",
        )
        .code("CONNECT_FAILED")
        .detail("Bearer detail-secret; request body body-secret")
        .hint(Hint::new("使用 token detail-secret 重试"))
        .hint(Hint::with_action("检查网络连接", "settings.network"));

        let public = PublicError::from(&error);
        let encoded = serde_json::to_string(&public).unwrap();
        assert_eq!(public.kind, "connect");
        assert_eq!(public.message, "无法连接到服务器");
        assert_eq!(public.code.as_deref(), Some("CONNECT_FAILED"));
        assert_eq!(public.hints[0].label, GENERIC_HINT);
        assert_eq!(public.hints[1].action.as_deref(), Some("settings.network"));
        assert_absent(
            &encoded,
            &["password", "detail-secret", "body-secret", "detail"],
        );
    }

    #[test]
    fn legacy_shapes_are_sanitized_after_deserialization() {
        let raw = secret_request_spec(Body::Json {
            content: "legacy-body-secret".to_string(),
        });
        let serialized = serde_json::to_string(&raw).unwrap();
        let legacy: PublicRequestPreview = serde_json::from_str(&serialized).unwrap();
        let encoded = serde_json::to_string(&legacy.redacted()).unwrap();
        assert_absent(
            &encoded,
            &[
                "legacy-body-secret",
                "auth-secret",
                "proxy-secret",
                "param-secret",
                "header-secret",
            ],
        );
    }
}
