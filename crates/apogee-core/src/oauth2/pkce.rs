//! OAuth2 authorization-code flow: bounded loopback callback and mandatory PKCE S256.
//! Authorization codes, state and verifiers live only here, never in serialized configs.
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::rand::{SecureRandom, SystemRandom};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const MAX_CALLBACK_BYTES: usize = 16 * 1024;
const MAX_CALLBACKS: usize = 32;
const AUTH_TIMEOUT: Duration = Duration::from_secs(180);

fn random_value() -> Result<String, AppError> {
    let mut bytes = [0; 32];
    SystemRandom::new().fill(&mut bytes).map_err(|_| {
        error(ErrorKind::Io, "无法生成安全的 OAuth2 PKCE 随机数").code("OAUTH2_RANDOM_FAILED")
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD
        .encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()).as_ref())
}
fn redirect(value: &str) -> Result<url::Url, AppError> {
    let fail = || {
        error(ErrorKind::Parse, "Redirect URI 必须是带固定端口和路径的 http://127.0.0.1 URL，不能包含凭据、Query 或 fragment").code("OAUTH2_REDIRECT_INVALID")
    };
    let url = url::Url::parse(value).map_err(|_| fail())?;
    if value.len() > 2048 || url.scheme() != "http"
        || url.host() != Some(url::Host::Ipv4(Ipv4Addr::LOCALHOST))
        || url.port().is_none_or(|port| port == 0) || url.path() == "/"
        || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some()
        // Do not silently normalize a registered redirect URI.
        || url.as_str() != value
    {
        return Err(fail());
    }
    Ok(url)
}
fn authorization_url(
    input: &OAuth2TokenRequest,
    redirect: &url::Url,
    state: &str,
    verifier: &str,
) -> Result<url::Url, AppError> {
    let mut url = endpoint(&input.authorization_url).map_err(|_| {
        error(
            ErrorKind::Parse,
            "Authorization URL 仅允许完整 HTTPS（本机调试可用 HTTP），不能包含凭据或 fragment",
        )
        .code("OAUTH2_AUTH_URL_INVALID")
    })?;
    let reserved = [
        "response_type",
        "client_id",
        "redirect_uri",
        "state",
        "scope",
        "code_challenge",
        "code_challenge_method",
        "response_mode",
        "request",
        "request_uri",
        "client_secret",
        "code_verifier",
        "access_token",
        "refresh_token",
        "code",
        "password",
    ];
    if url.query_pairs().any(|(key, _)| {
        reserved
            .iter()
            .any(|reserved| key.eq_ignore_ascii_case(reserved))
    }) {
        return Err(error(
            ErrorKind::Parse,
            "Authorization URL 含冲突或敏感协议参数；请使用 Auth 配置字段",
        )
        .code("OAUTH2_AUTH_PARAMS_INVALID"));
    }
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &input.client_id)
        .append_pair("redirect_uri", redirect.as_str())
        .append_pair("state", state)
        .append_pair("code_challenge", &challenge(verifier))
        .append_pair("code_challenge_method", "S256");
    if !input.scope.is_empty() {
        url.query_pairs_mut().append_pair("scope", &input.scope);
    }
    if url.as_str().len() > 8192 {
        return Err(
            error(ErrorKind::Parse, "OAuth2 授权 URL 编码后过长").code("OAUTH2_CONFIG_TOO_LARGE")
        );
    }
    Ok(url)
}

enum Callback {
    Ignore,
    Code(String),
    Denied,
}
fn same_state(actual: &str, expected: &str) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    actual
        .as_bytes()
        .iter()
        .zip(expected.as_bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}
fn parse_callback(bytes: &[u8], redirect: &url::Url, state: &str) -> Callback {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Callback::Ignore;
    };
    let Some((headers, _)) = text.split_once("\r\n\r\n") else {
        return Callback::Ignore;
    };
    let mut lines = headers.split("\r\n");
    let mut request = lines.next().unwrap_or("").split(' ');
    if request.next() != Some("GET") {
        return Callback::Ignore;
    }
    let Some(target) = request
        .next()
        .filter(|target| target.starts_with('/') && !target.starts_with("//"))
    else {
        return Callback::Ignore;
    };
    if request.next() != Some("HTTP/1.1") || request.next().is_some() {
        return Callback::Ignore;
    }
    let expected_host = format!("127.0.0.1:{}", redirect.port().unwrap());
    let mut host_seen = false;
    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            return Callback::Ignore;
        };
        if key.eq_ignore_ascii_case("host") {
            if host_seen || value.trim() != expected_host {
                return Callback::Ignore;
            }
            host_seen = true;
        }
        if key.eq_ignore_ascii_case("transfer-encoding")
            || key.eq_ignore_ascii_case("content-length") && value.trim() != "0"
        {
            return Callback::Ignore;
        }
    }
    if !host_seen {
        return Callback::Ignore;
    }
    let Ok(url) = redirect.join(target) else {
        return Callback::Ignore;
    };
    if url.origin() != redirect.origin()
        || url.path() != redirect.path()
        || url.fragment().is_some()
    {
        return Callback::Ignore;
    }
    let mut callback_state = None;
    let mut code = None;
    let mut denial = None;
    for (key, value) in url.query_pairs() {
        let slot = match key.as_ref() {
            "state" => &mut callback_state,
            "code" => &mut code,
            "error" => &mut denial,
            _ => continue,
        };
        if slot.is_some() {
            return Callback::Ignore;
        }
        *slot = Some(value.into_owned());
    }
    if !callback_state
        .as_deref()
        .is_some_and(|actual| same_state(actual, state))
    {
        return Callback::Ignore;
    }
    match (code, denial) {
        (Some(code), None)
            if !code.is_empty()
                && code.len() <= MAX_TOKEN_BYTES
                && !code.chars().any(char::is_control) =>
        {
            Callback::Code(code)
        }
        (None, Some(denial)) if !denial.is_empty() => Callback::Denied,
        _ => Callback::Ignore,
    }
}
async fn receive(stream: &mut TcpStream, redirect: &url::Url, state: &str) -> Callback {
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    while bytes.len() < MAX_CALLBACK_BYTES {
        let Ok(count) = stream.read(&mut buffer).await else {
            return Callback::Ignore;
        };
        if count == 0 {
            return Callback::Ignore;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > MAX_CALLBACK_BYTES {
            return Callback::Ignore;
        }
        if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            return parse_callback(&bytes, redirect, state);
        }
    }
    Callback::Ignore
}
async fn reply(stream: &mut TcpStream, accepted: bool) {
    // Never reflect the URL, code, state or provider error, even in the browser.
    let (status, body) = if accepted {
        (
            "200 OK",
            "Authorization received. Return to Apogee. You may close this window.",
        )
    } else {
        (
            "400 Bad Request",
            "Invalid callback. Return to Apogee and retry.",
        )
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = stream.write_all(response.as_bytes()).await;
}

/// Opens only a validated authorization URL, after successfully binding the registered callback.
/// The browser adapter is synchronous: cancellation must not leave a deferred browser-open task.
pub async fn authorize<F>(
    input: &OAuth2TokenRequest,
    environment: Option<&Environment>,
    settings: &ResolvedSettings,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
    open_browser: F,
) -> Result<Option<OAuth2TokenResponse>, AppError>
where
    F: FnOnce(url::Url) -> Result<(), AppError>,
{
    if !matches!(
        cancel.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ) {
        return Ok(None);
    }
    let input = resolve(input, OAuth2Grant::AuthorizationCode, environment)?;
    let redirect = redirect(&input.redirect_uri)?;
    let verifier = random_value()?;
    let state = random_value()?;
    let url = authorization_url(&input, &redirect, &state, &verifier)?;
    // Validate token endpoint, credentials and networking BEFORE opening the browser.
    let mut network = settings.clone();
    network.follow_redirects = false;
    let client = crate::http::client_for(&network).map_err(|_| {
        error(ErrorKind::Io, "OAuth2 网络设置无效或 HTTP 客户端初始化失败")
            .code("OAUTH2_NETWORK_INVALID")
    })?;
    build_request_with_code(
        &client,
        &input,
        OAuth2Grant::AuthorizationCode,
        Some(&CodeExchange {
            code: String::new(),
            verifier: verifier.clone(),
            redirect_uri: redirect.to_string(),
        }),
    )?;
    let listener = TcpListener::bind(SocketAddrV4::new(
        Ipv4Addr::LOCALHOST,
        redirect.port().unwrap(),
    ))
    .await
    .map_err(|_| {
        error(
            ErrorKind::Io,
            "OAuth2 回调端口无法绑定；检查端口占用或修改并重新注册 Redirect URI",
        )
        .code("OAUTH2_CALLBACK_BIND_FAILED")
    })?;
    let wait = async {
        open_browser(url).map_err(|_| {
            error(ErrorKind::Io, "无法打开 OAuth2 系统浏览器；授权已停止")
                .code("OAUTH2_BROWSER_OPEN_FAILED")
        })?;
        for _ in 0..MAX_CALLBACKS {
            let (mut stream, peer) = listener.accept().await.map_err(|_| {
                error(ErrorKind::Io, "OAuth2 本机回调监听失败").code("OAUTH2_CALLBACK_IO")
            })?;
            if !peer.ip().is_loopback() {
                continue;
            }
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                receive(&mut stream, &redirect, &state),
            )
            .await
            .unwrap_or(Callback::Ignore);
            let accepted = !matches!(result, Callback::Ignore);
            let _ = tokio::time::timeout(Duration::from_millis(500), reply(&mut stream, accepted))
                .await;
            match result {
                Callback::Code(code) => return Ok(code),
                Callback::Denied => {
                    return Err(error(
                        ErrorKind::Http,
                        "OAuth2 浏览器授权被拒绝或失败；未交换 Token",
                    )
                    .code("OAUTH2_AUTH_DENIED"))
                }
                Callback::Ignore => {}
            }
        }
        Err(
            error(ErrorKind::Http, "OAuth2 无效回调过多，授权已停止，请重试")
                .code("OAUTH2_CALLBACK_LIMIT"),
        )
    };
    let code = tokio::select! { biased;
        _ = &mut cancel => return Ok(None),
        result = tokio::time::timeout(AUTH_TIMEOUT, wait) => result
            .map_err(|_| error(ErrorKind::Timeout, "OAuth2 浏览器授权超过 3 分钟，已停止等待").code("OAUTH2_AUTH_TIMEOUT"))??,
    };
    drop(listener); // Close before exchange; never leave a callback server in the background.
    let exchange = CodeExchange {
        code,
        verifier,
        redirect_uri: redirect.to_string(),
    };
    fetch_resolved(
        &input,
        OAuth2Grant::AuthorizationCode,
        &network,
        cancel,
        Some(&exchange),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config(redirect_uri: String) -> OAuth2TokenRequest {
        OAuth2TokenRequest {
            token_url: "https://provider.test/token".into(),
            client_id: "public-client".into(),
            authorization_url: "https://provider.test/authorize?audience=api".into(),
            redirect_uri,
            grant_type: OAuth2Flow::AuthorizationCode,
            ..Default::default()
        }
    }
    fn free_redirect() -> String {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        format!(
            "http://127.0.0.1:{}/oauth/callback",
            listener.local_addr().unwrap().port()
        )
    }
    fn settings() -> ResolvedSettings {
        ResolvedSettings {
            proxy_url: Some("direct://".into()),
            ..Default::default()
        }
    }
    fn raw(target: &str, uri: &url::Url) -> String {
        format!(
            "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            uri.port().unwrap()
        )
    }
    async fn callback(mut uri: url::Url, state: &str, code: Option<&str>) -> String {
        uri.query_pairs_mut().append_pair("state", state);
        if let Some(code) = code {
            uri.query_pairs_mut().append_pair("code", code);
        } else {
            uri.query_pairs_mut()
                .append_pair("error", "access_denied")
                .append_pair("error_description", "fixture-private-description");
        }
        let target = format!("{}?{}", uri.path(), uri.query().unwrap());
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, uri.port().unwrap()))
            .await
            .unwrap();
        stream
            .write_all(raw(&target, &uri).as_bytes())
            .await
            .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.unwrap();
        reply
    }
    fn browser_values(url: &url::Url) -> HashMap<String, String> {
        url.query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }
    async fn request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buf = [0; 1024];
        loop {
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = std::str::from_utf8(&bytes[..end]).unwrap();
                let len: usize = header
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .unwrap()
                    .1
                    .trim()
                    .parse()
                    .unwrap();
                if bytes.len() >= end + 4 + len {
                    return String::from_utf8(bytes).unwrap();
                }
            }
            assert!(bytes.len() < MAX_TOKEN_RESPONSE_BYTES + MAX_CALLBACK_BYTES);
        }
    }
    async fn token_reply(stream: &mut TcpStream) {
        let body = r#"{"access_token":"fixture-access","token_type":"Bearer","refresh_token":"fixture-refresh"}"#;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }

    #[test]
    fn rfc7636_s256_vector_and_fresh_randomness() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let first = random_value().unwrap();
        assert_eq!(first.len(), 43);
        assert!(first
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        assert_ne!(first, random_value().unwrap());
    }
    #[test]
    fn redirect_is_explicit_canonical_ipv4_loopback_only() {
        for uri in [
            "https://127.0.0.1:8000/cb",
            "http://localhost:8000/cb",
            "http://[::1]:8000/cb",
            "http://0.0.0.0:8000/cb",
            "http://127.0.0.2:8000/cb",
            "http://127.0.0.1/cb",
            "http://127.0.0.1:0/cb",
            "http://127.0.0.1:8000/",
            "http://127.0.0.1:8000/cb?q=private",
            "http://private@127.0.0.1:8000/cb",
            "http://127.0.0.1:8000/cb#private",
            "http://127.0.0.1:8000/x/../cb",
        ] {
            let failure = redirect(uri).unwrap_err();
            assert!(!format!("{failure:?}").contains("private"));
        }
        assert!(redirect("http://127.0.0.1:8000/oauth/callback").is_ok());
    }
    #[test]
    fn auth_url_preserves_provider_parameters_but_cannot_override_protocol_or_leak_secrets() {
        let uri = redirect("http://127.0.0.1:8000/cb").unwrap();
        let mut input = config(uri.to_string());
        input.client_secret = "fixture-private-secret".into();
        input.refresh_token = "fixture-private-refresh".into();
        input.scope = "read write".into();
        let url = authorization_url(&input, &uri, "state", "verifier").unwrap();
        let params = browser_values(&url);
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["scope"], "read write");
        assert_eq!(params["audience"], "api");
        assert!(!url.as_str().contains("private"));
        for suffix in [
            "state=bad",
            "%73tate=bad",
            "code_challenge_method=plain",
            "response_mode=form_post",
            "CLIENT_SECRET=private",
            "request_uri=https://private",
        ] {
            input.authorization_url = format!("https://provider.test/authorize?{suffix}");
            assert!(authorization_url(&input, &uri, "state", "verifier").is_err());
        }
    }
    #[test]
    fn callback_requires_method_host_path_state_and_unique_outcome() {
        let uri = redirect("http://127.0.0.1:8000/cb").unwrap();
        assert!(
            matches!(parse_callback(raw("/cb?state=expected&code=a%2Bb",&uri).as_bytes(),&uri,"expected"),Callback::Code(code) if code=="a+b")
        );
        for target in [
            "/other?state=expected&code=a",
            "/cb?state=wrong&code=a",
            "/cb?code=a",
            "/cb?state=expected&state=expected&code=a",
            "/cb?state=expected&code=a&code=b",
            "/cb?state=expected&code=a&error=denied",
            "/cb?state=expected&code=",
            "/cb?state=expected&code=a%0D%0Ab",
            "//evil.test/cb?state=expected&code=a",
            "/\\evil.test/cb?state=expected&code=a",
            "/cb?state=expected&code=a#bad",
        ] {
            assert!(matches!(
                parse_callback(raw(target, &uri).as_bytes(), &uri, "expected"),
                Callback::Ignore
            ));
        }
        for request in ["POST /cb?state=expected&code=a HTTP/1.1\r\nHost: 127.0.0.1:8000\r\n\r\n", "GET /cb?state=expected&code=a HTTP/1.1\r\nHost: evil.test\r\n\r\n", "GET /cb?state=expected&code=a HTTP/1.1\r\nHost: 127.0.0.1:8000\r\nHost: 127.0.0.1:8000\r\n\r\n", "GET /cb?state=expected&code=a HTTP/1.1\r\n\r\n", "GET /cb?state=expected&code=a HTTP/1.1\r\nHost: 127.0.0.1:8000\r\nTransfer-Encoding: chunked\r\n\r\n"] {
            assert!(matches!(parse_callback(request.as_bytes(),&uri,"expected"),Callback::Ignore));
        }
        assert!(matches!(
            parse_callback(
                raw("/cb?state=expected&error=access_denied", &uri).as_bytes(),
                &uri,
                "expected"
            ),
            Callback::Denied
        ));
    }
    #[test]
    fn public_code_exchange_uses_body_id_without_secret_or_scope() {
        let client = crate::http::client_for(&settings()).unwrap();
        let uri = "http://127.0.0.1:8000/cb";
        let mut input = config(uri.into());
        input.scope = "read".into();
        let exchange = CodeExchange {
            code: "fixture-code +/".into(),
            verifier: "verifier".into(),
            redirect_uri: uri.into(),
        };
        let request = build_request_with_code(
            &client,
            &input,
            OAuth2Grant::AuthorizationCode,
            Some(&exchange),
        )
        .unwrap();
        assert!(request.headers().get("authorization").is_none());
        let pairs: HashMap<_, _> =
            url::form_urlencoded::parse(request.body().unwrap().as_bytes().unwrap())
                .into_owned()
                .collect();
        assert_eq!(pairs["grant_type"], "authorization_code");
        assert_eq!(pairs["code"], exchange.code);
        assert_eq!(pairs["redirect_uri"], uri);
        assert_eq!(pairs["client_id"], "public-client");
        assert!(!pairs.contains_key("client_secret"));
        assert!(!pairs.contains_key("scope"));
        assert!(
            build_request_with_code(&client, &input, OAuth2Grant::AuthorizationCode, None).is_err()
        );
        input.client_secret = "fixture-private-secret".into();
        assert!(build_request_with_code(
            &client,
            &input,
            OAuth2Grant::AuthorizationCode,
            Some(&exchange)
        )
        .unwrap()
        .headers()
        .contains_key("authorization"));
    }
    #[tokio::test]
    async fn loopback_code_exchange_checks_s256_rejects_wrong_state_and_closes_listener() {
        let token = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let uri = free_redirect();
        let callback_uri = redirect(&uri).unwrap();
        let port = callback_uri.port().unwrap();
        let mut input = config(uri.clone());
        input.token_url = format!(
            "http://127.0.0.1:{}/token",
            token.local_addr().unwrap().port()
        );
        let server = tokio::spawn(async move {
            let (mut stream, _) = token.accept().await.unwrap();
            let request = request(&mut stream).await;
            // Callback listener must already be closed before any credential exchange.
            let rebound = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap();
            drop(rebound);
            token_reply(&mut stream).await;
            request
        });
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (browser_tx, browser_rx) = tokio::sync::oneshot::channel();
        let result = authorize(&input, None, &settings(), rx, |url| {
            let values = browser_values(&url);
            let expected = values["code_challenge"].clone();
            tokio::spawn(async move {
                let wrong =
                    callback(callback_uri.clone(), "wrong", Some("fixture-private-code")).await;
                assert!(wrong.starts_with("HTTP/1.1 400"));
                assert!(!wrong.contains("private"));
                let reply =
                    callback(callback_uri, &values["state"], Some("fixture-code +/中文")).await;
                assert!(reply.starts_with("HTTP/1.1 200"));
                assert!(!reply.contains(&values["state"]));
                assert!(!reply.contains("fixture-code"));
                browser_tx.send(expected).unwrap();
            });
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.access_token, "fixture-access");
        drop(tx);
        let request = server.await.unwrap();
        let pairs: HashMap<_, _> =
            url::form_urlencoded::parse(request.split_once("\r\n\r\n").unwrap().1.as_bytes())
                .into_owned()
                .collect();
        assert_eq!(pairs["code"], "fixture-code +/中文");
        assert_eq!(pairs["redirect_uri"], uri);
        assert_eq!(
            challenge(&pairs["code_verifier"]),
            browser_rx.await.unwrap()
        );
    }
    #[tokio::test]
    async fn cancelled_before_start_or_after_browser_open_never_exchanges_and_releases_port() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        tx.send(()).unwrap();
        assert!(authorize(
            &OAuth2TokenRequest::default(),
            None,
            &settings(),
            rx,
            |_| panic!("must not open")
        )
        .await
        .unwrap()
        .is_none());
        let uri = free_redirect();
        let port = redirect(&uri).unwrap().port().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        assert!(authorize(&config(uri), None, &settings(), rx, |_| {
            tx.send(()).unwrap();
            Ok(())
        })
        .await
        .unwrap()
        .is_none());
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await.is_ok());
    }
    #[tokio::test]
    async fn occupied_port_invalid_endpoint_and_bad_variables_do_not_launch_browser() {
        let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let uri = format!("http://127.0.0.1:{}/cb", busy.local_addr().unwrap().port());
        for input in [
            config(uri),
            OAuth2TokenRequest {
                token_url: "http://private.test/token".into(),
                ..config(free_redirect())
            },
            OAuth2TokenRequest {
                authorization_url: "{{fixture-private-variable}}".into(),
                ..config(free_redirect())
            },
        ] {
            let (_tx, rx) = tokio::sync::oneshot::channel();
            let failure = authorize(&input, None, &settings(), rx, |_| panic!("must not open"))
                .await
                .unwrap_err();
            assert!(!format!("{failure:?}").contains("private"));
        }
    }
    #[tokio::test]
    async fn provider_denial_is_source_safe_and_never_connects_to_token_endpoint() {
        let uri = free_redirect();
        let callback_uri = redirect(&uri).unwrap();
        let (_tx, rx) = tokio::sync::oneshot::channel();
        let (done, finished) = tokio::sync::oneshot::channel();
        let failure = authorize(&config(uri), None, &settings(), rx, |url| {
            let values = browser_values(&url);
            tokio::spawn(async move {
                let reply = callback(callback_uri, &values["state"], None).await;
                assert!(!reply.contains("private"));
                done.send(()).unwrap();
            });
            Ok(())
        })
        .await
        .unwrap_err();
        finished.await.unwrap();
        assert!(failure.message.contains("拒绝"));
        assert!(!format!("{failure:?}").contains("private"));
    }
    #[tokio::test]
    async fn browser_failure_closes_the_callback_listener() {
        let uri = free_redirect();
        let port = redirect(&uri).unwrap().port().unwrap();
        let (_tx, rx) = tokio::sync::oneshot::channel();
        assert!(
            authorize(&config(uri), None, &settings(), rx, |_| Err(error(
                ErrorKind::Io,
                "fixture-private-launch-url"
            )))
            .await
            .is_err()
        );
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await.is_ok());
    }
    #[tokio::test]
    async fn cancellation_during_code_exchange_interrupts_response_read() {
        let token = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let mut input = config(free_redirect());
        input.token_url = format!(
            "http://127.0.0.1:{}/token",
            token.local_addr().unwrap().port()
        );
        let callback_uri = redirect(&input.redirect_uri).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = token.accept().await.unwrap();
            request(&mut stream).await;
            tx.send(()).unwrap();
        });
        let result = authorize(&input, None, &settings(), rx, |url| {
            let values = browser_values(&url);
            tokio::spawn(async move {
                callback(callback_uri, &values["state"], Some("fixture-code")).await;
            });
            Ok(())
        })
        .await
        .unwrap();
        assert!(result.is_none());
        server.await.unwrap();
    }
    #[tokio::test]
    async fn too_many_invalid_callbacks_stop_the_flow_and_release_listener() {
        let uri = free_redirect();
        let callback_uri = redirect(&uri).unwrap();
        let port = callback_uri.port().unwrap();
        let (_tx, rx) = tokio::sync::oneshot::channel();
        let mut worker = None;
        let failure = authorize(&config(uri), None, &settings(), rx, |_| {
            worker = Some(tokio::spawn(async move {
                for _ in 0..MAX_CALLBACKS {
                    callback(callback_uri.clone(), "wrong-state", Some("private-code")).await;
                }
            }));
            Ok(())
        })
        .await
        .unwrap_err();
        worker.unwrap().await.unwrap();
        assert!(failure.message.contains("过多"));
        assert!(!format!("{failure:?}").contains("private"));
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await.is_ok());
    }
    #[tokio::test]
    async fn a_slow_partial_request_is_bounded_and_does_not_block_valid_authorization() {
        let token = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let mut input = config(free_redirect());
        input.token_url = format!(
            "http://127.0.0.1:{}/token",
            token.local_addr().unwrap().port()
        );
        let callback_uri = redirect(&input.redirect_uri).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = token.accept().await.unwrap();
            request(&mut stream).await;
            token_reply(&mut stream).await;
        });
        let mut browser = None;
        let result = authorize(&input, None, &settings(), rx, |url| {
            let values = browser_values(&url);
            browser = Some(tokio::spawn(async move {
                let mut slow =
                    TcpStream::connect((Ipv4Addr::LOCALHOST, callback_uri.port().unwrap()))
                        .await
                        .unwrap();
                slow.write_all(b"GET /cb").await.unwrap();
                callback(callback_uri, &values["state"], Some("fixture-code")).await;
            }));
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.access_token, "fixture-access");
        browser.unwrap().await.unwrap();
        server.await.unwrap();
        drop(tx);
    }

    #[tokio::test]
    async fn callback_headers_are_bounded_even_when_not_terminated() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap();
            stream
                .write_all(&vec![b'x'; MAX_CALLBACK_BYTES + 1024])
                .await
                .unwrap();
        });
        let (mut stream, _) = listener.accept().await.unwrap();
        let uri = redirect(&format!("http://127.0.0.1:{port}/cb")).unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), receive(&mut stream, &uri, "state"))
                .await
                .unwrap(),
            Callback::Ignore
        ));
        client.await.unwrap();
    }
}
