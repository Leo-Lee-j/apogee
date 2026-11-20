use super::*;
use apogee_core::vars::{EnvironmentVariable, VariableScopes};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn variable(key: &str, value: &str) -> EnvironmentVariable {
    serde_json::from_value(serde_json::json!({"key":key,"value":value,"enabled":true})).unwrap()
}

async fn capturing_server() -> (
    String,
    tokio::sync::oneshot::Receiver<String>,
    Arc<tokio::sync::Notify>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let notify = release.clone();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 2048];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0 && bytes.len() < 16_384);
            bytes.extend_from_slice(&chunk[..n]);
            let wire = String::from_utf8_lossy(&bytes);
            if let Some((headers, body)) = wire.split_once("\r\n\r\n") {
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if body.len() >= length {
                    break;
                }
            }
        }
        tx.send(String::from_utf8(bytes).unwrap()).unwrap();
        notify.notified().await;
        let body = r#"{"access_token":"scoped-token","refresh_token":"scoped-refresh","token_type":"Bearer","expires_in":3600}"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(reply.as_bytes()).await.unwrap();
    });
    (url, rx, release, task)
}

#[tokio::test]
async fn manual_acquisition_uses_global_collection_folder_environment_and_runtime_precedence() {
    for level in 0..5 {
        let (url, ready, release, server) = capturing_server().await;
        let f = Fixture::new("{{endpoint}}".into());
        let environment_id = f
            .state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                repo.save_global_variables(vec![
                    variable("endpoint", &url),
                    variable("client", "global"),
                    variable("secret", "scoped-secret"),
                ])?;
                let snapshot = repo.snapshot()?;
                let collection = snapshot
                    .nodes
                    .iter()
                    .find(|n| n.id == f.folder)
                    .unwrap()
                    .parent_id
                    .unwrap();
                if level >= 1 {
                    let mut container = repo.read_container(collection)?;
                    container.variables = vec![variable("client", "collection")];
                    repo.save_container(container)?;
                }
                let mut folder = repo.read_container(f.folder)?;
                if level >= 2 {
                    folder.variables = vec![variable("client", "folder")];
                }
                if let AuthConfig::OAuth2 {
                    token_request: Some(config),
                    ..
                } = &mut folder.auth
                {
                    config.client_id = "{{client}}".into();
                    config.client_secret = "{{secret}}".into();
                }
                repo.save_container(folder)?;
                if level >= 3 {
                    let mut environment = repo.create_environment("Scopes".into())?;
                    environment.variables = vec![variable("client", "environment")];
                    Ok(Some(repo.save_environment(environment)?.id))
                } else {
                    Ok(None)
                }
            })
            .unwrap();
        let mut input = f.input(0);
        input.environment_id = environment_id;
        input.grant = Some(OAuth2Grant::ClientCredentials);
        if level == 4 {
            input
                .runtime_variables
                .insert("client".into(), "runtime".into());
        }
        let operation = start(&f, input);
        let wire = tokio::time::timeout(std::time::Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        let body: std::collections::HashMap<_, _> =
            url::form_urlencoded::parse(wire.split_once("\r\n\r\n").unwrap().1.as_bytes())
                .into_owned()
                .collect();
        assert_eq!(
            body["client_id"],
            ["global", "collection", "folder", "environment", "runtime"][level]
        );
        assert_eq!(body["client_secret"], "scoped-secret");
        release.notify_one();
        let output = operation.await.unwrap().unwrap().unwrap();
        assert!(
            matches!(output.auth, AuthConfig::OAuth2 {access_token,..} if access_token == "scoped-token")
        );
        assert!(output.variables.global.iter().any(|v| v.key == "endpoint"));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn scope_changes_during_refresh_discard_token_without_overwriting_source() {
    for scope in ["global", "collection", "folder"] {
        let (url, ready, release, server) = capturing_server().await;
        let f = Fixture::new(url);
        let original = f.auth();
        let operation = start(&f, f.input(0));
        tokio::time::timeout(std::time::Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        f.state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                let variables = vec![variable("changed", "during-exchange")];
                if scope == "global" {
                    repo.save_global_variables(variables)?;
                } else {
                    let id = if scope == "folder" {
                        f.folder
                    } else {
                        repo.snapshot()?
                            .nodes
                            .into_iter()
                            .find(|n| n.id == f.folder)
                            .unwrap()
                            .parent_id
                            .unwrap()
                    };
                    let mut container = repo.read_container(id)?;
                    container.variables = variables;
                    repo.save_container(container)?;
                }
                Ok(())
            })
            .unwrap();
        release.notify_one();
        let error = match operation.await.unwrap() {
            Err(error) => error,
            _ => panic!("stale context accepted"),
        };
        assert_eq!(error.code.as_deref(), Some("OAUTH2_SOURCE_STALE"));
        assert_eq!(
            serde_json::to_value(f.auth()).unwrap(),
            serde_json::to_value(original).unwrap()
        );
        server.await.unwrap();
    }
}

#[test]
fn scope_fingerprint_ignores_editor_ids_but_tracks_runtime_values() {
    let f = Fixture::new("https://auth.test/token".into());
    let mut scopes = VariableScopes {
        global: vec![variable("client", "before")],
        ..Default::default()
    };
    let context = |scopes: &VariableScopes| {
        apogee_core::oauth2::issuance_context_with_scopes(
            &f.auth(),
            None,
            &RequestSettings::default(),
            None,
            scopes,
        )
        .unwrap()
    };
    let baseline = context(&scopes);
    scopes.global[0].id = Uuid::new_v4();
    scopes.global[0].has_value = true;
    assert_eq!(baseline, context(&scopes));
    scopes.runtime.insert("client".into(), "runtime".into());
    assert_ne!(baseline, context(&scopes));
}
