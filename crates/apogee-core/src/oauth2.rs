//! Explicit OAuth2 credential operations. Bodies and tokens never enter response spool/history.
use crate::{
    error::{AppError, ErrorKind, Hint},
    request::ResolvedSettings,
    vars::Environment,
};
use serde::{Deserialize, Serialize};
use std::fmt;

mod pkce;
pub use pkce::authorize;

pub const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_TOKEN_BYTES: usize = 16 * 1024;

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2TokenRequest {
    pub token_url: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub client_auth: OAuth2ClientAuth,
    #[serde(default, skip_serializing_if = "OAuth2Flow::is_default")]
    pub grant_type: OAuth2Flow,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub authorization_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redirect_uri: String,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OAuth2Flow {
    #[default]
    ClientCredentials,
    AuthorizationCode,
}
impl OAuth2Flow {
    fn is_default(&self) -> bool {
        *self == Self::ClientCredentials
    }
}
// Credential configs must not accidentally become diagnostics through {:?}.
impl fmt::Debug for OAuth2TokenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuth2TokenRequest { credentials: [REDACTED] }")
    }
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OAuth2ClientAuth {
    #[default]
    Basic,
    Body,
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OAuth2Grant {
    ClientCredentials,
    RefreshToken,
    AuthorizationCode,
}

/// Sensitive command result, ONLY for an explicitly requested Auth-editor operation.
/// Not a response/public/history DTO. No arbitrary endpoint JSON is returned to the renderer.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2TokenResponse {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    /// Host clock at Token HTTP exchange start, not browser authorization start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obtained_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_context: Option<String>,
}
impl fmt::Debug for OAuth2TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuth2TokenResponse { tokens: [REDACTED] }")
    }
}
/// Shared expiry semantics; no transport, persistence, JWT decoding or background refresh.
pub fn auth_needs_refresh(auth: &crate::request::AuthConfig) -> bool {
    let crate::request::AuthConfig::OAuth2 {
        access_token,
        obtained_at,
        expires_at,
        auto_refresh,
        ..
    } = auth
    else {
        return false;
    };
    if !auto_refresh {
        return false;
    }
    if access_token.trim().is_empty() {
        return true;
    }
    let Some(now) = unix_millis() else {
        return true;
    };
    if obtained_at.is_some_and(|at| now < at) {
        return true;
    }
    expires_at
        .filter(|at| *at <= 8_640_000_000_000_000)
        .is_some_and(|at| now.saturating_add(30_000) >= at)
}
/// Hash only private issuance context; excludes access/refresh values so rotation preserves identity.
pub fn issuance_context(
    auth: &crate::request::AuthConfig,
    environment: Option<&Environment>,
    settings: &crate::request::RequestSettings,
    environment_id: Option<uuid::Uuid>,
) -> Result<String, AppError> {
    let crate::request::AuthConfig::OAuth2 { token_request, .. } = auth else {
        return Ok(String::new());
    };
    let mut config = token_request.clone();
    if let Some(config) = &mut config {
        config.refresh_token.clear();
    }
    let bytes = serde_json::to_vec(&(config, environment, settings, environment_id))
        .map_err(|_| error(ErrorKind::Parse, "OAuth2 context unavailable"))?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    Ok(digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn unix_millis() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
}
pub fn apply_auth_response(
    auth: &crate::request::AuthConfig,
    response: &OAuth2TokenResponse,
) -> Result<crate::request::AuthConfig, AppError> {
    let mut next = auth.clone();
    let crate::request::AuthConfig::OAuth2 {
        access_token,
        token_request,
        obtained_at,
        expires_at,
        token_context,
        ..
    } = &mut next
    else {
        return Err(error(
            ErrorKind::Parse,
            "OAuth2 operation could not complete",
        ));
    };
    *access_token = response.access_token.clone();
    *token_context = response.token_context.clone();
    *obtained_at = response.obtained_at;
    *expires_at = response
        .obtained_at
        .zip(response.expires_in)
        .and_then(|(at, seconds)| seconds.checked_mul(1000).and_then(|ms| at.checked_add(ms)))
        .filter(|at| *at <= 8_640_000_000_000_000);
    if let Some(config) = token_request {
        if let Some(refresh) = &response.refresh_token {
            config.refresh_token.clone_from(refresh);
        }
    }
    Ok(next)
}

fn error(kind: ErrorKind, message: &str) -> AppError {
    AppError::new(kind, message).hint(Hint::new(
        "检查 Token URL、客户端凭据、授权范围和所选环境；详情请在服务端安全日志中排查",
    ))
}
fn resolve(
    input: &OAuth2TokenRequest,
    grant: OAuth2Grant,
    environment: Option<&Environment>,
) -> Result<OAuth2TokenRequest, AppError> {
    let resolve_text = |text: &str, env| {
        crate::vars::resolve_text(text, env)
            .map_err(|_| error(ErrorKind::Var, "OAuth2 配置变量解析失败；检查所选环境"))
    };
    let mut resolved = input.clone();
    resolved.token_url = resolve_text(&input.token_url, environment)?;
    // Endpoint must be explicit: do not accidentally send credentials to an environment host.
    resolved.client_id = resolve_text(&input.client_id, environment)?;
    resolved.client_secret = resolve_text(&input.client_secret, environment)?;
    resolved.scope = resolve_text(&input.scope, environment)?;
    resolved.refresh_token = match grant {
        OAuth2Grant::RefreshToken => resolve_text(&input.refresh_token, environment)?,
        _ => String::new(),
    };
    if matches!(grant, OAuth2Grant::AuthorizationCode) {
        resolved.authorization_url = resolve_text(&input.authorization_url, environment)?;
        resolved.redirect_uri = resolve_text(&input.redirect_uri, environment)?;
    }
    Ok(resolved)
}
fn endpoint(value: &str) -> Result<url::Url, AppError> {
    if value.len() > 8192 {
        return Err(error(ErrorKind::Parse, "OAuth2 Token URL 过长"));
    }
    let url = url::Url::parse(value)
        .map_err(|_| error(ErrorKind::Parse, "OAuth2 Token URL 必须是完整的 HTTPS URL"))?;
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.host().is_none()
        || (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(error(
            ErrorKind::Parse,
            "OAuth2 Token URL 仅允许 HTTPS（本机调试可用 HTTP），不能包含用户名密码或 fragment",
        ));
    }
    Ok(url)
}
#[cfg(test)]
fn build_request(
    client: &reqwest::Client,
    input: &OAuth2TokenRequest,
    grant: OAuth2Grant,
) -> Result<reqwest::Request, AppError> {
    build_request_with_code(client, input, grant, None)
}
struct CodeExchange {
    code: String,
    verifier: String,
    redirect_uri: String,
}
fn build_request_with_code(
    client: &reqwest::Client,
    input: &OAuth2TokenRequest,
    grant: OAuth2Grant,
    exchange: Option<&CodeExchange>,
) -> Result<reqwest::Request, AppError> {
    let url = endpoint(&input.token_url)?;
    if input.client_id.trim().is_empty()
        || (matches!(grant, OAuth2Grant::ClientCredentials) && input.client_secret.is_empty())
    {
        return Err(error(
            ErrorKind::Parse,
            "Client Credentials 需要 Client ID / Secret；其他授权也需要 Client ID",
        ));
    }
    if matches!(grant, OAuth2Grant::RefreshToken) && input.refresh_token.is_empty() {
        return Err(error(ErrorKind::Parse, "刷新 Token 需要 Refresh Token"));
    }
    if [
        &input.client_id,
        &input.client_secret,
        &input.scope,
        &input.refresh_token,
    ]
    .iter()
    .map(|v| v.len())
    .sum::<usize>()
        > MAX_TOKEN_RESPONSE_BYTES
    {
        return Err(error(ErrorKind::Parse, "OAuth2 凭据配置过大"));
    }
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair(
        "grant_type",
        match grant {
            OAuth2Grant::ClientCredentials => "client_credentials",
            OAuth2Grant::RefreshToken => "refresh_token",
            OAuth2Grant::AuthorizationCode => "authorization_code",
        },
    );
    if matches!(grant, OAuth2Grant::RefreshToken) {
        form.append_pair("refresh_token", &input.refresh_token);
    }
    if matches!(grant, OAuth2Grant::AuthorizationCode) {
        let exchange = exchange
            .ok_or_else(|| error(ErrorKind::Parse, "授权码必须通过 PKCE 浏览器授权获取"))?;
        form.append_pair("code", &exchange.code);
        form.append_pair("code_verifier", &exchange.verifier);
        form.append_pair("redirect_uri", &exchange.redirect_uri);
    } else if !input.scope.is_empty() {
        form.append_pair("scope", &input.scope);
    }
    let mut builder = client
        .post(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        );
    let client_auth =
        if input.client_secret.is_empty() && !matches!(grant, OAuth2Grant::ClientCredentials) {
            OAuth2ClientAuth::Body // Public clients identify themselves without a fake Basic credential.
        } else {
            input.client_auth
        };
    match client_auth {
        OAuth2ClientAuth::Basic => {
            // RFC 6749 2.3.1: form-encode each credential BEFORE Basic base64 encoding.
            let encode =
                |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
            builder =
                builder.basic_auth(encode(&input.client_id), Some(encode(&input.client_secret)));
        }
        OAuth2ClientAuth::Body => {
            form.append_pair("client_id", &input.client_id);
            if !input.client_secret.is_empty() {
                form.append_pair("client_secret", &input.client_secret);
            }
        }
    }
    let form = form.finish();
    if form.len() > MAX_TOKEN_RESPONSE_BYTES {
        return Err(error(ErrorKind::Parse, "OAuth2 编码后的凭据配置过大"));
    }
    builder
        .body(form)
        .build()
        .map_err(|_| error(ErrorKind::Parse, "无法构造 OAuth2 Token 请求"))
}
fn parse_response(
    bytes: &[u8],
    status: reqwest::StatusCode,
) -> Result<OAuth2TokenResponse, AppError> {
    if bytes.len() > MAX_TOKEN_RESPONSE_BYTES {
        return Err(error(ErrorKind::Http, "OAuth2 Token 响应超过 64 KiB 限制"));
    }
    let value = serde_json::from_slice::<serde_json::Value>(bytes);
    if !status.is_success() {
        let code = value
            .as_ref()
            .ok()
            .and_then(|v| v.get("error"))
            .and_then(|v| v.as_str())
            .filter(|code| {
                [
                    "invalid_request",
                    "invalid_client",
                    "invalid_grant",
                    "unauthorized_client",
                    "unsupported_grant_type",
                    "invalid_scope",
                ]
                .contains(code)
            });
        let message = code.map_or_else(
            || format!("OAuth2 Token 端点返回 HTTP {}", status.as_u16()),
            |code| format!("OAuth2 Token 端点返回 HTTP {}（{code}）", status.as_u16()),
        );
        return Err(error(ErrorKind::Http, &message)); // Never echo error_description or arbitrary input.
    }
    let value = value.map_err(|_| error(ErrorKind::Parse, "OAuth2 Token 响应不是合法 JSON"))?;
    let token_type = value.get("token_type").and_then(|v| v.as_str());
    if !token_type.is_some_and(|v| v.eq_ignore_ascii_case("bearer")) {
        return Err(error(
            ErrorKind::Parse,
            "OAuth2 Token 响应需要 token_type=Bearer；不支持其他 Token 类型",
        ));
    }
    let read_token = |key: &str| -> Result<Option<String>, AppError> {
        let Some(value) = value.get(key) else {
            return Ok(None);
        };
        let token = value
            .as_str()
            .filter(|v| {
                !v.is_empty()
                    && v.len() <= MAX_TOKEN_BYTES
                    && v.bytes().all(|b| {
                        if key == "access_token" {
                            b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b)
                        } else {
                            (0x20..=0x7e).contains(&b)
                        }
                    })
            })
            .ok_or_else(|| error(ErrorKind::Parse, "OAuth2 Token 响应含无效或过长的凭据字段"))?;
        Ok(Some(token.into()))
    };
    let access_token = read_token("access_token")?
        .ok_or_else(|| error(ErrorKind::Parse, "OAuth2 Token 响应缺少 access_token"))?;
    let refresh_token = read_token("refresh_token")?;
    let expires_in = match value.get("expires_in") {
        None => None,
        Some(v) => Some(
            v.as_u64()
                .or_else(|| v.as_str().and_then(|v| v.parse().ok()))
                .ok_or_else(|| error(ErrorKind::Parse, "OAuth2 expires_in 必须是非负秒数"))?,
        ),
    };
    Ok(OAuth2TokenResponse {
        access_token,
        refresh_token,
        expires_in,
        obtained_at: None,
        token_context: None,
    })
}

pub async fn fetch_token(
    input: &OAuth2TokenRequest,
    grant: OAuth2Grant,
    environment: Option<&Environment>,
    settings: &ResolvedSettings,
    cancel: tokio::sync::oneshot::Receiver<()>,
) -> Result<Option<OAuth2TokenResponse>, AppError> {
    let input = resolve(input, grant, environment)?;
    fetch_resolved(&input, grant, settings, cancel, None).await
}
async fn fetch_resolved(
    input: &OAuth2TokenRequest,
    grant: OAuth2Grant,
    settings: &ResolvedSettings,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
    exchange: Option<&CodeExchange>,
) -> Result<Option<OAuth2TokenResponse>, AppError> {
    let mut settings = settings.clone();
    settings.follow_redirects = false; // NEVER forward credentials to an endpoint-supplied redirect.
    let client = crate::http::client_for(&settings)
        .map_err(|_| error(ErrorKind::Io, "OAuth2 网络设置无效或 HTTP 客户端初始化失败"))?;
    let request = build_request_with_code(&client, input, grant, exchange)?;
    let operation = async {
        let obtained_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());
        let mut response = client.execute(request).await.map_err(|e| {
            let kind = if e.is_timeout() {
                ErrorKind::Timeout
            } else {
                ErrorKind::Connect
            };
            error(kind, "OAuth2 Token 端点连接失败；检查网络、TLS 或代理设置")
        })?;
        let status = response.status();
        if status.is_redirection() {
            return Err(error(
                ErrorKind::Http,
                "OAuth2 Token 端点返回重定向；为保护凭据，不自动跟随",
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_TOKEN_RESPONSE_BYTES as u64)
        {
            return Err(error(ErrorKind::Http, "OAuth2 Token 响应超过 64 KiB 限制"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| error(ErrorKind::Http, "读取 OAuth2 Token 响应失败"))?
        {
            if bytes.len() + chunk.len() > MAX_TOKEN_RESPONSE_BYTES {
                return Err(error(ErrorKind::Http, "OAuth2 Token 响应超过 64 KiB 限制"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let mut result = parse_response(&bytes, status)?;
        result.obtained_at = obtained_at;
        Ok(result)
    };
    let timeout = std::time::Duration::from_millis(if settings.timeout_ms == 0 {
        30_000
    } else {
        settings.timeout_ms.min(120_000)
    });
    tokio::select! { biased;
        _=&mut cancel=>Ok(None),
        result=tokio::time::timeout(timeout,operation)=>match result {
            Ok(result)=>result.map(Some),Err(_)=>Err(error(ErrorKind::Timeout,"OAuth2 Token 请求超时（包含连接及响应读取阶段）")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    #[tokio::test]
    async fn invalid_proxy_details_never_expose_credentials() {
        let mut settings = settings();
        settings.proxy_url = Some("invalid://private-proxy-secret".into());
        let (_tx, cancel) = tokio::sync::oneshot::channel();
        let failure = fetch_token(
            &config(),
            OAuth2Grant::ClientCredentials,
            None,
            &settings,
            cancel,
        )
        .await
        .unwrap_err();
        assert!(!serde_json::to_string(&failure)
            .unwrap()
            .contains("private-proxy-secret"));
    }

    fn config() -> OAuth2TokenRequest {
        OAuth2TokenRequest {
            token_url: "https://auth.example.test/token".into(),
            client_id: "client:id".into(),
            client_secret: "secret +/中".into(),
            ..Default::default()
        }
    }
    fn settings() -> ResolvedSettings {
        ResolvedSettings {
            proxy_url: Some("direct://".into()),
            ..Default::default()
        }
    }
    fn body(request: &reqwest::Request) -> String {
        String::from_utf8(request.body().unwrap().as_bytes().unwrap().to_vec()).unwrap()
    }
    #[test]
    fn basic_credentials_are_form_encoded_before_base64_per_rfc6749() {
        let req = build_request(
            &crate::http::client_for(&settings()).unwrap(),
            &config(),
            OAuth2Grant::ClientCredentials,
        )
        .unwrap();
        let auth = req.headers()[reqwest::header::AUTHORIZATION]
            .to_str()
            .unwrap();
        assert_eq!(
            STANDARD
                .decode(auth.strip_prefix("Basic ").unwrap())
                .unwrap(),
            b"client%3Aid:secret+%2B%2F%E4%B8%AD"
        );
        assert!(req.headers()[reqwest::header::AUTHORIZATION].is_sensitive());
        assert_eq!(body(&req), "grant_type=client_credentials");
        assert_eq!(
            req.headers()[reqwest::header::CONTENT_TYPE],
            "application/x-www-form-urlencoded"
        );
    }
    #[test]
    fn body_credentials_and_refresh_grant_preserve_values_without_dual_auth() {
        let mut input = config();
        input.client_auth = OAuth2ClientAuth::Body;
        input.scope = "read write".into();
        input.refresh_token = "refresh+token/=".into();
        let req = build_request(
            &crate::http::client_for(&settings()).unwrap(),
            &input,
            OAuth2Grant::RefreshToken,
        )
        .unwrap();
        assert!(!req.headers().contains_key(reqwest::header::AUTHORIZATION));
        let pairs = url::form_urlencoded::parse(body(&req).as_bytes())
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(pairs["grant_type"], "refresh_token");
        assert_eq!(pairs["refresh_token"], input.refresh_token);
        assert_eq!(pairs["client_secret"], input.client_secret);
        assert_eq!(pairs["scope"], "read write");
        input.client_secret.clear();
        assert!(!body(
            &build_request(
                &crate::http::client_for(&settings()).unwrap(),
                &input,
                OAuth2Grant::RefreshToken
            )
            .unwrap()
        )
        .contains("client_secret"));
    }
    #[test]
    fn endpoint_rejects_insecure_and_credential_urls_without_echoing_them() {
        for value in [
            "http://example.test/token",
            "ftp://localhost/token",
            "https://user:private@auth.test/token",
            "https://auth.test/token#private",
            "relative/private",
        ] {
            let error = endpoint(value).unwrap_err();
            assert!(!serde_json::to_string(&error).unwrap().contains("private"));
        }
        for value in [
            "https://auth.test/token",
            "http://localhost/token",
            "http://127.0.0.1/token",
            "http://[::1]/token",
        ] {
            assert!(endpoint(value).is_ok());
        }
    }
    #[test]
    fn invalid_or_oversized_inputs_fail_without_echoing_credentials() {
        let client = crate::http::client_for(&settings()).unwrap();
        let mut input = config();
        input.client_id.clear();
        assert!(build_request(&client, &input, OAuth2Grant::ClientCredentials).is_err());
        input = config();
        assert!(build_request(&client, &input, OAuth2Grant::RefreshToken).is_err());
        input.client_secret = "private".repeat(MAX_TOKEN_RESPONSE_BYTES);
        let error = build_request(&client, &input, OAuth2Grant::ClientCredentials).unwrap_err();
        assert!(!serde_json::to_string(&error).unwrap().contains("private"));
    }
    #[test]
    fn successful_response_returns_only_validated_token_fields() {
        let response=parse_response(br#"{"access_token":"access-token","token_type":"bearer","refresh_token":"refresh-token","expires_in":"3600","id_token":"not-returned","private":"not-returned"}"#,reqwest::StatusCode::OK).unwrap();
        assert_eq!(response.access_token, "access-token");
        assert_eq!(response.expires_in, Some(3600));
        assert!(!serde_json::to_string(&response)
            .unwrap()
            .contains("not-returned"));
        assert!(!format!("{response:?}").contains("access-token"));
        assert!(!format!("{:?}", config()).contains("secret"));
    }
    #[test]
    fn malformed_tokens_and_non_bearer_types_are_rejected_without_response_echo() {
        for value in [
            serde_json::json!({"token_type":"Bearer"}),
            serde_json::json!({"access_token":"private\n","token_type":"Bearer"}),
            serde_json::json!({"access_token":"private","token_type":"MAC"}),
            serde_json::json!({"access_token":"private","token_type":"Bearer","expires_in":-1}),
        ] {
            let error = parse_response(
                &serde_json::to_vec(&value).unwrap(),
                reqwest::StatusCode::OK,
            )
            .unwrap_err();
            assert!(!serde_json::to_string(&error).unwrap().contains("private"));
        }
        assert!(parse_response(
            &vec![b'x'; MAX_TOKEN_RESPONSE_BYTES + 1],
            reqwest::StatusCode::OK
        )
        .is_err());
    }
    #[test]
    fn error_description_and_arbitrary_error_codes_are_never_public() {
        for code in ["invalid_client", "private-code"] {
            let error = parse_response(
                &serde_json::to_vec(
                    &serde_json::json!({"error":code,"error_description":"private-secret"}),
                )
                .unwrap(),
                reqwest::StatusCode::UNAUTHORIZED,
            )
            .unwrap_err();
            let text = serde_json::to_string(&error).unwrap();
            assert!(!text.contains("private"));
            if code == "invalid_client" {
                assert!(text.contains("invalid_client"));
            }
        }
    }
    #[test]
    fn resolves_credential_templates_but_does_not_resolve_unused_refresh_token() {
        let env:Environment=serde_json::from_value(serde_json::json!({"id":uuid::Uuid::new_v4(),"name":"Test","host":"", "variables":[
            {"key":"url","value":"https://auth.test/token","enabled":true},{"key":"secret","value":"resolved","enabled":true}]})).unwrap();
        let input = OAuth2TokenRequest {
            token_url: "{{url}}".into(),
            client_id: "client".into(),
            client_secret: "{{secret}}".into(),
            refresh_token: "{{missing}}".into(),
            ..Default::default()
        };
        let result = resolve(&input, OAuth2Grant::ClientCredentials, Some(&env)).unwrap();
        assert_eq!(result.token_url, "https://auth.test/token");
        assert_eq!(result.client_secret, "resolved");
        assert!(result.refresh_token.is_empty());
        assert!(resolve(&input, OAuth2Grant::RefreshToken, Some(&env)).is_err());
        assert_eq!(input.client_secret, "{{secret}}");
    }
    fn server(
        response: String,
        body_delay: std::time::Duration,
    ) -> (
        String,
        tokio::sync::oneshot::Receiver<()>,
        std::thread::JoinHandle<String>,
    ) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (ready, receiver) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            let mut buffer = [0; 8192];
            let mut required = None;
            loop {
                if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&received[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    required = Some(end + 4 + length);
                }
                if required.is_some_and(|n| received.len() >= n) {
                    break;
                }
                let n = stream.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buffer[..n]);
            }
            let (headers, body) = response.split_once("\r\n\r\n").unwrap();
            let _ = stream.write_all(format!("{headers}\r\n\r\n").as_bytes());
            let _ = ready.send(());
            std::thread::sleep(body_delay);
            let _ = stream.write_all(body.as_bytes());
            String::from_utf8(received).unwrap()
        });
        (format!("http://{address}/token"), receiver, worker)
    }
    fn json_response() -> String {
        let body = r#"{"access_token":"live-token","token_type":"Bearer","refresh_token":"live-refresh","expires_in":3600}"#;
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())
    }
    #[tokio::test]
    async fn loopback_fetch_and_refresh_use_form_post_and_validated_result() {
        for grant in [OAuth2Grant::ClientCredentials, OAuth2Grant::RefreshToken] {
            let (url, _ready, worker) = server(json_response(), std::time::Duration::ZERO);
            let mut input = config();
            input.token_url = url;
            input.refresh_token = "old-refresh".into();
            let (_tx, rx) = tokio::sync::oneshot::channel();
            let result = fetch_token(&input, grant, None, &settings(), rx)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.access_token, "live-token");
            assert!(result.obtained_at.is_some_and(|start| start > 0));
            let wire = worker.join().unwrap();
            assert!(wire.starts_with("POST /token HTTP/1.1"));
            assert!(!wire.contains("secret +/中"));
            assert!(!wire.contains("client_secret="));
        }
    }
    #[tokio::test]
    async fn cancellation_interrupts_response_download() {
        let (url, ready, worker) = server(json_response(), std::time::Duration::from_millis(100));
        let mut input = config();
        input.token_url = url;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            fetch_token(
                &input,
                OAuth2Grant::ClientCredentials,
                None,
                &settings(),
                rx,
            )
            .await
        });
        ready.await.unwrap();
        tx.send(()).unwrap();
        assert!(task.await.unwrap().unwrap().is_none());
        worker.join().unwrap();
    }
    #[tokio::test]
    async fn timeout_covers_body_read_and_redirects_never_forward_credentials() {
        let (url, _ready, worker) = server(json_response(), std::time::Duration::from_millis(100));
        let mut input = config();
        input.token_url = url;
        let (_tx, rx) = tokio::sync::oneshot::channel();
        let mut config_settings = settings();
        config_settings.timeout_ms = 20;
        assert!(matches!(
            fetch_token(
                &input,
                OAuth2Grant::ClientCredentials,
                None,
                &config_settings,
                rx
            )
            .await
            .unwrap_err()
            .kind,
            ErrorKind::Timeout
        ));
        worker.join().unwrap();
        let (url,_ready,worker)=server("HTTP/1.1 302 Found\r\nLocation: https://not-contacted.test/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),std::time::Duration::ZERO);
        input.token_url = url;
        let (_tx, rx) = tokio::sync::oneshot::channel();
        let error = fetch_token(
            &input,
            OAuth2Grant::ClientCredentials,
            None,
            &settings(),
            rx,
        )
        .await
        .unwrap_err();
        assert!(!serde_json::to_string(&error).unwrap().contains("private"));
        worker.join().unwrap();
    }
    #[tokio::test]
    async fn response_limits_apply_to_declared_and_chunked_bodies() {
        let oversized = "x".repeat(MAX_TOKEN_RESPONSE_BYTES + 1);
        for response in [format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",oversized.len()),
            format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n{oversized}\r\n0\r\n\r\n",oversized.len())] {
            let (url,_ready,worker)=server(response,std::time::Duration::ZERO);let mut input=config();input.token_url=url;let (_tx,rx)=tokio::sync::oneshot::channel();
            let error=fetch_token(&input,OAuth2Grant::ClientCredentials,None,&settings(),rx).await.unwrap_err();assert!(error.message.contains("64 KiB"));worker.join().unwrap();
        }
    }
}
