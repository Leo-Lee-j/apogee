use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn cookies_follow_redirects_obey_disable_and_manual_headers_and_remain_session_local() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let seen = captured.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut buffer = [0; 2048];
                let n = socket.read(&mut buffer).await.unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..n]);
                if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8(bytes).unwrap();
            let path = text.split_whitespace().nth(1).unwrap().to_string();
            let cookie = text
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("cookie"))
                        .map(|(_, value)| value.trim().to_string())
                })
                .unwrap_or_default();
            seen.lock().unwrap().push((path.clone(), cookie));
            let extra = match path.as_str() {
                "/start" => {
                    "Set-Cookie: hop=fixture-session; Path=/; HttpOnly\r\nLocation: /end\r\n"
                        .to_string()
                }
                "/cross" => format!("Location: http://localhost:{}/end\r\n", address.port()),
                "/disabled" => "Set-Cookie: disabled=should-not-store; Path=/\r\n".to_string(),
                "/clear" => "Set-Cookie: hop=; Path=/; Max-Age=0\r\n".to_string(),
                _ => String::new(),
            };
            let status = if matches!(path.as_str(), "/start" | "/cross") {
                "302 Found"
            } else {
                "200 OK"
            };
            socket.write_all(format!("HTTP/1.1 {status}\r\n{extra}Content-Length: 2\r\nConnection: close\r\n\r\n{{}}").as_bytes()).await.unwrap();
        }
    });
    let root = std::env::temp_dir().join(format!("apogee-cookie-http-{}", Uuid::new_v4()));
    let spool = Spool::new(&root).unwrap();
    let session = HttpSession::default();
    async fn send(
        address: std::net::SocketAddr,
        path: &str,
        spool: &Spool,
        session: &HttpSession,
        enabled: bool,
        manual: Option<&str>,
    ) -> Box<ResponseMeta> {
        let mut spec: RequestSpec = serde_json::from_value(serde_json::json!({"method":"GET","url":format!("http://{address}{path}"),"settings":{"proxyUrl":"direct://","sendCookies":enabled}})).unwrap();
        if let Some(value) = manual {
            spec.headers.push(KeyValue {
                key: "Cookie".into(),
                value: value.into(),
                enabled: true,
                description: None,
                kind: ValueKind::Text,
            });
        }
        let (_sender, cancel) = tokio::sync::oneshot::channel();
        let outcome = execute_with_session(
            Uuid::new_v4(),
            &spec,
            None,
            spool,
            cancel,
            |_| {},
            Some(session),
        )
        .await;
        match outcome {
            Outcome::Done(meta) => meta,
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
    send(address, "/start", &spool, &session, true, None).await;
    assert_eq!(
        captured.lock().unwrap().last().unwrap().1,
        "hop=fixture-session"
    );
    assert_eq!(session.cookies.list().len(), 1);
    let meta = send(
        address,
        "/end",
        &spool,
        &session,
        true,
        Some("manual=fixture-manual"),
    )
    .await;
    assert_eq!(
        captured.lock().unwrap().last().unwrap().1,
        "manual=fixture-manual"
    );
    let public = serde_json::to_string(&crate::public::PublicActualRequest::from(
        &meta.actual_request,
    ))
    .unwrap();
    assert!(!public.contains("fixture-manual"));
    send(address, "/disabled", &spool, &session, false, None).await;
    assert_eq!(captured.lock().unwrap().last().unwrap().1, "");
    assert_eq!(session.cookies.list().len(), 1);
    send(address, "/end", &spool, &HttpSession::default(), true, None).await;
    assert_eq!(captured.lock().unwrap().last().unwrap().1, "");
    send(address, "/cross", &spool, &session, true, None).await;
    assert_eq!(captured.lock().unwrap().last().unwrap().1, "");
    send(address, "/clear", &spool, &session, true, None).await;
    assert!(session.cookies.list().is_empty());
    server.abort();
    drop(spool);
    std::fs::remove_dir_all(root).unwrap();
}
