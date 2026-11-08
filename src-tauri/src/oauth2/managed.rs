//! Source-scoped credential operations; no token-bearing events/history or settled cache.
use super::{failure_with_code, InFlightGuard, TokenOperationInput};
use crate::{
    app_settings::{apply_network_defaults, AppSettings},
    state::{AppState, OAuth2SourceLock},
};
use apogee_core::{
    oauth2::{apply_auth_response, auth_needs_refresh, OAuth2Grant, OAuth2TokenResponse},
    public::PublicError,
    request::{AuthConfig, Body, RequestSettings, RequestSpec},
    vars::Environment,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub operation_id: Uuid,
    pub node_id: Option<Uuid>,
    /// Only for non-persistent draft/case Auth. Never a path or credential key.
    pub owner_id: String,
    pub environment_id: Option<Uuid>,
    pub auth: AuthConfig,
    /// Editor's loaded source baseline; protects non-token Auth edits too.
    #[serde(default)]
    pub expected_auth: Option<AuthConfig>,
    /// None is pre-send preparation. Some is an explicit editor action.
    pub grant: Option<OAuth2Grant>,
    #[serde(default)]
    pub settings: RequestSettings,
    #[serde(default)]
    pub ephemeral: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub auth: AuthConfig,
    pub source_id: Option<Uuid>,
    // Host-only immutable execution snapshots; never expand sensitive IPC with environment values.
    #[serde(skip)]
    pub environment: Option<Environment>,
    #[serde(skip)]
    pub settings: RequestSettings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<OAuth2TokenResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<PublicError>,
}
fn stale() -> PublicError {
    failure_with_code("OAuth2 operation could not complete", "OAUTH2_SOURCE_STALE")
}
fn busy() -> PublicError {
    failure_with_code("OAuth2 operation could not complete", "OAUTH2_SOURCE_BUSY")
}
struct Snapshot {
    auth: AuthConfig,
    source: Option<Uuid>,
    baseline: Option<AuthConfig>,
    owner_auth: Option<AuthConfig>,
    owner_settings: Option<RequestSettings>,
    settings: RequestSettings,
    environment: Option<Environment>,
}
fn snapshot_in(
    repo: &crate::workspace::WorkspaceRepository,
    input: &Input,
    app: &AppSettings,
) -> anyhow::Result<Snapshot> {
    let (auth, source) = if let Some(node) = input.node_id {
        if input.ephemeral && !matches!(input.auth, AuthConfig::Inherit) {
            (input.auth.clone(), None)
        } else {
            repo.resolve_auth_source(node, input.auth.clone())?
        }
    } else {
        (input.auth.clone(), None)
    };
    let baseline = source.map(|id| repo.read_auth(id)).transpose()?;
    let owner = input
        .node_id
        .map(|id| {
            repo.read_request(id)
                .map(|r| (r.spec.auth, r.spec.settings))
                .or_else(|_| repo.read_container(id).map(|r| (r.auth, r.settings)))
        })
        .transpose()?;
    let (owner_auth, owner_settings) = owner.map_or((None, None), |(auth, settings)| {
        (Some(auth), Some(settings))
    });
    let mut settings = if let Some(node) = input.node_id {
        repo.resolve_token_request_inheritance(
            node,
            RequestSpec {
                method: "POST".into(),
                url: String::new(),
                params: vec![],
                path_params: vec![],
                headers: vec![],
                body: Body::None,
                auth: AuthConfig::None,
                settings: input.settings.clone(),
            },
        )?
        .settings
    } else {
        input.settings.clone()
    };
    let environment = input
        .environment_id
        .map(|id| repo.read_environment(id))
        .transpose()?;
    apply_network_defaults(&mut settings, app);
    Ok(Snapshot {
        auth,
        source,
        baseline,
        owner_auth,
        owner_settings,
        settings,
        environment,
    })
}
fn snapshot(
    state: &AppState,
    workspace: Uuid,
    session: Uuid,
    input: &Input,
    app: &AppSettings,
) -> Result<Snapshot, PublicError> {
    state
        .with_workspace(workspace, session, |repo| snapshot_in(repo, input, app))
        .map_err(|_| stale())
}

fn source_lock(
    state: &AppState,
    workspace: Uuid,
    session: Uuid,
    key: String,
) -> Result<Arc<OAuth2SourceLock>, PublicError> {
    let mut locks = state
        .oauth2_source_locks
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    locks.retain(|_, weak| weak.strong_count() > 0);
    let key = (workspace, session, key);
    if let Some(lock) = locks.get(&key).and_then(std::sync::Weak::upgrade) {
        if Arc::strong_count(&lock) > 32 {
            return Err(busy());
        }
        return Ok(lock);
    }
    if locks.len() >= 8 {
        return Err(busy());
    }
    let lock = Arc::new(OAuth2SourceLock::default());
    locks.insert(key, Arc::downgrade(&lock));
    Ok(lock)
}
fn equal<T: Serialize>(left: &T, right: &T) -> bool {
    serde_json::to_value(left)
        .ok()
        .zip(serde_json::to_value(right).ok())
        .is_some_and(|(a, b)| a == b)
}
fn credentials_match(auth: &AuthConfig, baseline: &Option<AuthConfig>) -> bool {
    match (auth, baseline) {
        (
            AuthConfig::OAuth2 {
                access_token: a,
                token_request: ar,
                ..
            },
            Some(AuthConfig::OAuth2 {
                access_token: b,
                token_request: br,
                ..
            }),
        ) => {
            // Save intentional credential replacement before acquiring. Never reuse a stale rotation.
            (b.is_empty() || a == b)
                && br.as_ref().is_none_or(|cfg| {
                    cfg.refresh_token.is_empty()
                        || ar
                            .as_ref()
                            .is_some_and(|c| c.refresh_token == cfg.refresh_token)
                })
        }
        _ => true,
    }
}
/// Conservatively invalidate waiting draft inputs after any attempted exchange, including
/// cancellation/errors. No credential hash or token is retained after the source lock dies.
struct AdvanceRevision<'a>(&'a std::sync::atomic::AtomicU64);
impl Drop for AdvanceRevision<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
pub async fn run(
    state: Arc<AppState>,
    workspace: Uuid,
    session: Uuid,
    input: Input,
    app: &AppSettings,
) -> Result<Option<Output>, PublicError> {
    if input.owner_id.is_empty()
        || input.owner_id.len() > 128
        || !input
            .owner_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b))
    {
        return Err(stale());
    }
    let initial = snapshot(&state, workspace, session, &input, app)?;
    if !input.ephemeral
        && input.expected_auth.as_ref().is_some_and(|expected| {
            initial
                .baseline
                .as_ref()
                .is_none_or(|actual| !equal(actual, expected))
        })
    {
        return Err(stale());
    }
    let lease = match state.begin_token_request(workspace, session, input.operation_id) {
        Ok(value) => value,
        Err(e) if e.to_string() == "TOKEN_OPERATION_CANCELLED" => return Ok(None),
        Err(_) => return Err(stale()),
    };
    let _cleanup = InFlightGuard {
        state: state.clone(),
        id: input.operation_id,
        generation: lease.generation(),
    };
    let (_, mut cancel, access) = lease.into_execution_parts();
    let key = initial.source.map_or_else(
        || format!("draft:{}", input.owner_id),
        |id| format!("node:{id}"),
    );
    let lock = source_lock(&state, workspace, session, key)?;
    let revision = lock.revision.load(std::sync::atomic::Ordering::SeqCst);
    let _source_guard = tokio::select! { biased;
        _ = &mut cancel => return Ok(None),
        result = tokio::time::timeout(std::time::Duration::from_secs(180), lock.gate.lock()) => result.map_err(|_| busy())?,
    };
    if initial.source.is_none()
        && revision != lock.revision.load(std::sync::atomic::Ordering::SeqCst)
    {
        // Drafts have no persisted source to re-read; never replay a queued old rotation.
        return Err(stale());
    }
    // Re-read after waiting: successful automatic refreshes are deduplicated by persisted expiry.
    let before = snapshot(&state, workspace, session, &input, app)?;
    if initial.source != before.source
        || !equal(&initial.owner_auth, &before.owner_auth)
        || !equal(&initial.owner_settings, &before.owner_settings)
    {
        return Err(stale());
    }
    if input.grant.is_some() && !equal(&initial.baseline, &before.baseline) {
        return Err(stale());
    }
    if !credentials_match(&before.auth, &before.baseline) {
        return Err(stale());
    }
    let context = apogee_core::oauth2::issuance_context(
        &before.auth,
        before.environment.as_ref(),
        &before.settings,
        input.environment_id,
    )
    .map_err(|_| stale())?;
    let context_changed = matches!(&before.auth, AuthConfig::OAuth2 { token_context: Some(value), auto_refresh: true, .. } if value != &context);
    if input.grant.is_none() && !auth_needs_refresh(&before.auth) && !context_changed {
        state
            .with_active_response_access(access, || Ok(()))
            .map_err(|_| stale())?;
        return Ok(Some(Output {
            auth: before.auth,
            source_id: before.source,
            environment: before.environment,
            settings: before.settings,
            token: None,
            error: None,
        }));
    }
    let AuthConfig::OAuth2 {
        token_request: Some(config),
        ..
    } = &before.auth
    else {
        return Err(failure_with_code(
            "OAuth2 operation could not complete",
            "OAUTH2_CONFIG_REQUIRED",
        ));
    };
    let grant = match input.grant {
        Some(grant) => grant,
        None if !config.refresh_token.trim().is_empty() => OAuth2Grant::RefreshToken,
        None if matches!(
            config.grant_type,
            apogee_core::oauth2::OAuth2Flow::ClientCredentials
        ) =>
        {
            OAuth2Grant::ClientCredentials
        }
        None => {
            return Err(failure_with_code(
                "OAuth2 operation could not complete",
                "OAUTH2_REAUTHORIZE",
            ))
        }
    };
    let _advance_revision = AdvanceRevision(&lock.revision);
    let transport_id = Uuid::new_v4();
    let transport = super::fetch(
        state.clone(),
        workspace,
        session,
        TokenOperationInput {
            operation_id: transport_id,
            node_id: input.node_id,
            environment_id: input.environment_id,
            request: (**config).clone(),
            grant,
            settings: input.settings.clone(),
        },
        app,
    );
    let response = tokio::select! { biased;
        _ = &mut cancel => { state.cancel_token_operation(transport_id); return Ok(None); },
        result = transport => result?,
    };
    let Some(mut response) = response else {
        return Ok(None);
    };
    if !matches!(
        cancel.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ) {
        return Ok(None);
    }
    response.token_context = Some(context);
    let next = apply_auth_response(&before.auth, &response).map_err(|e| PublicError::from(&e))?;
    // Short mutation window only; the source lock is not a workspace filesystem lock.
    let committed = state.with_workspace_mutation(workspace, session, |repo| {
        let after =
            snapshot_in(repo, &input, app).map_err(|_| anyhow::anyhow!("OAUTH2_SOURCE_STALE"))?;
        if after.source != before.source
            || !equal(&after.owner_auth, &before.owner_auth)
            || !equal(&after.owner_settings, &before.owner_settings)
            || !equal(&after.baseline, &before.baseline)
            || !equal(&after.environment, &before.environment)
            || !equal(&after.settings, &before.settings)
        {
            anyhow::bail!("OAUTH2_SOURCE_STALE");
        }
        state.with_active_token(input.operation_id, _cleanup.generation, || {
            if let (Some(id), Some(expected)) = (before.source, before.baseline.as_ref()) {
                repo.compare_and_set_auth(id, expected, next.clone())?;
            }
            Ok(())
        })
    });
    if let Err(error) = committed {
        let code = error.to_string();
        if code == "TOKEN_OPERATION_CANCELLED" {
            return Ok(None);
        }
        if code == "OAUTH2_SOURCE_STALE" || code.contains("SESSION_EXPIRED") {
            return Err(stale());
        }
        return Err(failure_with_code(
            "Could not persist Auth source",
            "OAUTH2_SOURCE_WRITE_FAILED",
        ));
    }
    state
        .with_active_response_access(access, || Ok(()))
        .map_err(|_| stale())?;
    let error = (input.grant.is_none() && auth_needs_refresh(&next))
        .then(|| failure_with_code("Token lifetime too short", "OAUTH2_TOKEN_TOO_SHORT"));

    Ok(Some(Output {
        auth: next,
        source_id: before.source,
        environment: before.environment,
        settings: before.settings,
        token: Some(response),
        error,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::WorkspaceRepository;
    use apogee_core::{
        oauth2::{OAuth2ClientAuth, OAuth2TokenRequest},
        workspace::{WorkspaceContainer, WorkspaceNodeKind},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct Fixture {
        root: std::path::PathBuf,
        state: Arc<AppState>,
        workspace: Uuid,
        session: Uuid,
        folder: Uuid,
        requests: Vec<Uuid>,
    }
    impl Fixture {
        fn new(url: String) -> Self {
            let root = std::env::temp_dir().join(format!("apogee-source-auth-{}", Uuid::new_v4()));
            let repo = WorkspaceRepository::create(root.join("workspace"), "Test".into()).unwrap();
            let collection = repo
                .create_collection("Collection".into())
                .unwrap()
                .nodes
                .into_iter()
                .find(|n| matches!(n.kind, WorkspaceNodeKind::Collection))
                .unwrap()
                .id;
            let folder = repo
                .create_folder(collection, "Folder".into())
                .unwrap()
                .nodes
                .into_iter()
                .find(|n| matches!(n.kind, WorkspaceNodeKind::Folder))
                .unwrap()
                .id;
            let a = repo.create_request(folder, "A".into()).unwrap();
            let b = repo.create_request(folder, "B".into()).unwrap();
            let auth = AuthConfig::OAuth2 {
                access_token: "old".into(),
                header_prefix: "Bearer".into(),
                obtained_at: Some(0),
                expires_at: Some(0),
                auto_refresh: true,
                token_context: None,
                token_request: Some(Box::new(OAuth2TokenRequest {
                    token_url: url,
                    client_id: "id".into(),
                    client_secret: "secret".into(),
                    refresh_token: "r0".into(),
                    client_auth: OAuth2ClientAuth::Body,
                    ..Default::default()
                })),
            };
            repo.save_container(WorkspaceContainer {
                id: folder,
                name: "Folder".into(),
                auth,
                settings: RequestSettings::default(),
            })
            .unwrap();
            let state = Arc::new(AppState::new().unwrap());
            let (workspace, session) = state.open_workspace(repo, root.join("runtime")).unwrap();
            Self {
                root,
                state,
                workspace,
                session,
                folder,
                requests: vec![a.id, b.id],
            }
        }
        fn input(&self, index: usize) -> Input {
            Input {
                operation_id: Uuid::new_v4(),
                node_id: Some(self.requests[index]),
                owner_id: self.requests[index].to_string(),
                environment_id: None,
                auth: AuthConfig::Inherit,
                expected_auth: None,
                grant: None,
                settings: RequestSettings::default(),
                ephemeral: false,
            }
        }
        fn auth(&self) -> AuthConfig {
            self.state
                .with_workspace(self.workspace, self.session, |repo| {
                    repo.read_auth(self.folder)
                })
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.state.close_workspace(self.workspace, self.session);
            assert!(self.root.starts_with(std::env::temp_dir()));
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    async fn server() -> (
        String,
        tokio::sync::oneshot::Receiver<()>,
        Arc<tokio::sync::Notify>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        server_with_body(r#"{"access_token":"new","refresh_token":"r1","token_type":"Bearer","expires_in":3600}"#).await
    }
    async fn server_with_body(
        body: &'static str,
    ) -> (
        String,
        tokio::sync::oneshot::Receiver<()>,
        Arc<tokio::sync::Notify>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        let (ready_tx, ready) = tokio::sync::oneshot::channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let notify = release.clone();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            seen.fetch_add(1, Ordering::SeqCst);
            let mut wire = vec![0; 8192];
            let n = socket.read(&mut wire).await.unwrap();
            assert!(n > 0);
            let _ = ready_tx.send(());
            notify.notified().await;
            let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
            let _ = socket.write_all(reply.as_bytes()).await;
            if tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                .await
                .is_ok()
            {
                seen.fetch_add(1, Ordering::SeqCst);
            }
        });
        (url, ready, release, count, task)
    }

    fn app() -> AppSettings {
        serde_json::from_value(serde_json::json!({"defaultWorkspaceRoot":"unused"})).unwrap()
    }
    fn start(
        f: &Fixture,
        input: Input,
    ) -> tokio::task::JoinHandle<Result<Option<Output>, PublicError>> {
        let state = f.state.clone();
        let workspace = f.workspace;
        let session = f.session;
        tokio::spawn(async move { run(state, workspace, session, input, &app()).await })
    }
    #[tokio::test]
    async fn inherited_siblings_refresh_once_and_only_real_source_is_written() {
        let (url, ready, release, count, server) = server().await;
        let f = Fixture::new(url);
        let first = start(&f, f.input(0));
        ready.await.unwrap();
        let second = start(&f, f.input(1));
        release.notify_one();
        let a = first.await.unwrap().unwrap().unwrap();
        let b = second.await.unwrap().unwrap().unwrap();
        assert_eq!(a.source_id, Some(f.folder));
        assert_eq!(b.source_id, Some(f.folder));
        assert!(a.token.is_some());
        assert!(b.token.is_none());
        assert!(!auth_needs_refresh(&f.auth()));
        f.state
            .with_workspace(f.workspace, f.session, |repo| {
                for id in &f.requests {
                    assert!(matches!(
                        repo.read_request(*id)?.spec.auth,
                        AuthConfig::Inherit
                    ));
                }

                Ok(())
            })
            .unwrap();
        assert!(f.state.meta.read().unwrap().is_empty());
        assert!(!f.root.join("runtime").join("history").exists());
        assert!(!f.root.join("workspace").join("history").exists());
        server.await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn manual_and_automatic_share_source_serialization_and_waiter_cancels_independently() {
        let (url, ready, release, count, server) = server().await;
        let f = Fixture::new(url);
        let mut manual = f.input(0);
        manual.node_id = Some(f.folder);
        manual.auth = f.auth();
        manual.grant = Some(OAuth2Grant::RefreshToken);
        let first = start(&f, manual);
        ready.await.unwrap();
        let cancelled = f.input(1);
        let id = cancelled.operation_id;
        let second = start(&f, cancelled);
        f.state.cancel_token_operation(id);
        assert!(second.await.unwrap().unwrap().is_none());
        let third = start(&f, f.input(1));
        release.notify_one();
        assert!(first.await.unwrap().unwrap().unwrap().token.is_some());
        assert!(third.await.unwrap().unwrap().unwrap().token.is_none());
        server.await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn stale_manual_rotation_queued_behind_auto_is_not_replayed() {
        let (url, ready, release, count, server) = server().await;
        let f = Fixture::new(url);
        let first = start(&f, f.input(0));
        ready.await.unwrap();
        let mut manual = f.input(0);
        manual.node_id = Some(f.folder);
        manual.auth = f.auth();
        manual.grant = Some(OAuth2Grant::RefreshToken);
        let second = start(&f, manual);
        tokio::task::yield_now().await;
        release.notify_one();
        first.await.unwrap().unwrap();
        let error = second.await.unwrap().err().unwrap();
        assert_eq!(error.code.as_deref(), Some("OAUTH2_SOURCE_STALE"));
        server.await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn edits_during_http_prevent_old_credentials_from_committing() {
        let (url, ready, release, _, server) = server().await;
        let f = Fixture::new(url);
        let first = start(&f, f.input(0));
        ready.await.unwrap();
        f.state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                let mut value = repo.read_container(f.folder)?;
                value.auth = AuthConfig::Bearer {
                    token: "user-edit".into(),
                };
                value.name = "User name".into();
                repo.save_container(value)?;
                Ok(())
            })
            .unwrap();
        release.notify_one();
        assert_eq!(
            first.await.unwrap().err().unwrap().code.as_deref(),
            Some("OAUTH2_SOURCE_STALE")
        );
        assert!(matches!(f.auth(),AuthConfig::Bearer{token} if token=="user-edit"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn settings_change_and_session_close_prevent_commit() {
        for close in [false, true] {
            let (url, ready, release, _, server) = server().await;
            let f = Fixture::new(url);
            let old = f.auth();
            let first = start(&f, f.input(0));
            ready.await.unwrap();
            if close {
                f.state.close_workspace(f.workspace, f.session).unwrap();
            } else {
                f.state
                    .with_workspace_mutation(f.workspace, f.session, |repo| {
                        let mut c = repo.read_container(f.folder)?;
                        c.settings.timeout_ms = Some(5000);
                        repo.save_container(c)?;
                        Ok(())
                    })
                    .unwrap();
            }
            release.notify_one();
            let result = first.await.unwrap();
            assert!(result.is_err() || result.unwrap().is_none());
            if !close {
                assert!(equal(&f.auth(), &old));
            }
            server.await.unwrap();
        }
    }
    #[tokio::test]
    async fn short_lived_rotation_is_persisted_and_returned_before_stopping_business_http() {
        let (url, ready, release, _, server) = server_with_body(
            r#"{"access_token":"short","refresh_token":"r1","token_type":"Bearer","expires_in":5,"token_context":"provider-must-not-control-context"}"#,
        ).await;
        let f = Fixture::new(url);
        let first = start(&f, f.input(0));
        ready.await.unwrap();
        release.notify_one();
        let output = first.await.unwrap().unwrap().unwrap();
        assert_eq!(
            output.error.as_ref().and_then(|e| e.code.as_deref()),
            Some("OAUTH2_TOKEN_TOO_SHORT")
        );
        assert_eq!(output.token.as_ref().unwrap().access_token, "short");
        assert!(equal(&f.auth(), &output.auth));
        assert!(
            matches!(&output.auth, AuthConfig::OAuth2 { token_request: Some(config), token_context: Some(context), .. }
            if config.refresh_token == "r1" && context.len() == 64 && context != "provider-must-not-control-context")
        );
        let spec = RequestSpec {
            method: "GET".into(),
            url: "https://api.test".into(),
            params: vec![],
            path_params: vec![],
            headers: vec![],
            body: Body::None,
            auth: output.auth.clone(),
            settings: RequestSettings::default(),
        };
        let public =
            serde_json::to_string(&apogee_core::public::PublicRequestPreview::from(&spec)).unwrap();
        assert!(
            !public.contains("tokenContext") && !public.contains("r1") && !public.contains("short")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stale_editor_baseline_rejects_non_token_auth_changes_before_exchange() {
        let f = Fixture::new("https://auth.test/token".into());
        let old = f.auth();
        let mut edited = old.clone();
        if let AuthConfig::OAuth2 {
            token_request: Some(config),
            ..
        } = &mut edited
        {
            config.client_secret = "new-config-secret".into();
        }
        f.state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                repo.compare_and_set_auth(f.folder, &old, edited.clone())
            })
            .unwrap();
        let mut manual = f.input(0);
        manual.node_id = Some(f.folder);
        manual.auth = old.clone();
        manual.expected_auth = Some(old);
        manual.grant = Some(OAuth2Grant::RefreshToken);
        assert_eq!(
            start(&f, manual)
                .await
                .unwrap()
                .err()
                .unwrap()
                .code
                .as_deref(),
            Some("OAUTH2_SOURCE_STALE")
        );
        assert!(equal(&f.auth(), &edited));
        assert!(f.state.oauth2_source_locks.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn queued_draft_refresh_cannot_replay_a_previous_rotation_without_a_persisted_source() {
        let (url, ready, release, count, server) = server().await;
        let f = Fixture::new(url);
        let old = f.auth();
        let mut manual = f.input(0);
        manual.ephemeral = true;
        manual.auth = old.clone();
        manual.grant = Some(OAuth2Grant::RefreshToken);
        let first = start(&f, manual);
        ready.await.unwrap();
        let mut automatic = f.input(0);
        automatic.ephemeral = true;
        automatic.auth = old.clone();
        let second = start(&f, automatic);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let queued = f
                    .state
                    .oauth2_source_locks
                    .lock()
                    .unwrap()
                    .values()
                    .any(|lock| lock.strong_count() >= 2);
                if queued {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        release.notify_one();
        assert!(first.await.unwrap().unwrap().unwrap().token.is_some());
        assert_eq!(
            second.await.unwrap().err().unwrap().code.as_deref(),
            Some("OAUTH2_SOURCE_STALE")
        );
        assert!(equal(&old, &f.auth())); // ephemeral Auth never writes the source request/folder.
        server.await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn environment_edit_during_exchange_prevents_source_commit() {
        let (url, ready, release, _, server) = server().await;
        let f = Fixture::new(url);
        let environment = f
            .state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                repo.create_environment("Test".into())
            })
            .unwrap();
        let old = f.auth();
        let mut input = f.input(0);
        input.environment_id = Some(environment.id);
        let first = start(&f, input);
        ready.await.unwrap();
        f.state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                let mut updated = environment.clone();
                updated
                    .variables
                    .push(apogee_core::vars::EnvironmentVariable {
                        key: "client".into(),
                        value: "changed".into(),
                        enabled: true,
                    });
                repo.save_environment(updated)?;
                Ok(())
            })
            .unwrap();
        release.notify_one();
        assert_eq!(
            first.await.unwrap().err().unwrap().code.as_deref(),
            Some("OAUTH2_SOURCE_STALE")
        );
        assert!(equal(&f.auth(), &old));
        server.await.unwrap();
    }

    #[test]
    fn source_bounds_and_optimistic_save_reject_stale_auth_without_changing_other_fields() {
        let f = Fixture::new("https://auth.test/token".into());
        let old = f.auth();
        let mut updated = old.clone();
        if let AuthConfig::OAuth2 { access_token, .. } = &mut updated {
            *access_token = "rotated".into();
        }
        f.state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                repo.compare_and_set_auth(f.folder, &old, updated.clone())?;
                assert!(repo.ensure_auth_unchanged(f.folder, &old).is_err());
                assert!(repo
                    .compare_and_set_auth(f.folder, &old, old.clone())
                    .is_err());
                let container = repo.read_container(f.folder)?;
                assert_eq!(container.name, "Folder");
                assert!(equal(&container.auth, &updated));
                let request = repo.read_request(f.requests[0])?;
                repo.compare_and_set_auth(request.id, &AuthConfig::Inherit, updated.clone())?;
                let after = repo.read_request(request.id)?;
                assert_eq!(after.name, request.name);
                assert!(equal(&after.spec.settings, &request.spec.settings));
                Ok(())
            })
            .unwrap();
        let held: Vec<_> = (0..8)
            .map(|n| source_lock(&f.state, f.workspace, f.session, n.to_string()).unwrap())
            .collect();
        assert_eq!(
            source_lock(&f.state, f.workspace, f.session, "ninth".into())
                .err()
                .unwrap()
                .code
                .as_deref(),
            Some("OAUTH2_SOURCE_BUSY")
        );
        drop(held);
        assert!(source_lock(&f.state, f.workspace, f.session, "ninth".into()).is_ok());
    }
    #[test]
    fn contexts_isolate_environment_and_config_but_not_refresh_rotation() {
        let f = Fixture::new("https://auth.test/token".into());
        let old = f.auth();
        let mut rotated = old.clone();
        if let AuthConfig::OAuth2 {
            token_request: Some(c),
            ..
        } = &mut rotated
        {
            c.refresh_token = "r1".into();
        }
        let context =
            apogee_core::oauth2::issuance_context(&old, None, &RequestSettings::default(), None)
                .unwrap();
        assert_eq!(
            context,
            apogee_core::oauth2::issuance_context(
                &rotated,
                None,
                &RequestSettings::default(),
                None
            )
            .unwrap()
        );
        assert_ne!(
            context,
            apogee_core::oauth2::issuance_context(
                &old,
                None,
                &RequestSettings::default(),
                Some(Uuid::new_v4())
            )
            .unwrap()
        );
        assert_ne!(
            context,
            apogee_core::oauth2::issuance_context(
                &old,
                None,
                &RequestSettings {
                    timeout_ms: Some(1000),
                    ..Default::default()
                },
                None
            )
            .unwrap()
        );
    }
    #[tokio::test]
    async fn nearer_request_override_during_exchange_blocks_parent_commit() {
        let (url, ready, release, _, server) = server().await;
        let f = Fixture::new(url);
        let old = f.auth();
        let first = start(&f, f.input(0));
        ready.await.unwrap();
        f.state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                let r = repo.read_request(f.requests[0])?;
                let mut spec = r.spec;
                spec.auth = AuthConfig::None;
                repo.save_request(r.id, r.name, spec)
            })
            .unwrap();
        release.notify_one();
        assert_eq!(
            first.await.unwrap().err().unwrap().code.as_deref(),
            Some("OAUTH2_SOURCE_STALE")
        );
        assert!(equal(&old, &f.auth()));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn changed_issuance_context_refreshes_even_if_access_token_has_not_expired() {
        let (url, ready, release, _, server) = server().await;
        let f = Fixture::new(url);
        f.state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                let mut c = repo.read_container(f.folder)?;
                if let AuthConfig::OAuth2 {
                    expires_at,
                    token_context,
                    ..
                } = &mut c.auth
                {
                    *expires_at = Some(8_640_000_000_000_000);
                    *token_context = Some("different-environment".into());
                }
                repo.save_container(c)?;
                Ok(())
            })
            .unwrap();
        assert!(!auth_needs_refresh(&f.auth()));
        let first = start(&f, f.input(0));
        ready.await.unwrap();
        release.notify_one();
        let result = first.await.unwrap().unwrap().unwrap();
        assert!(result.token.is_some());
        assert!(
            matches!(result.auth, AuthConfig::OAuth2{token_context:Some(context),..} if context.len()==64 && context!="different-environment")
        );
        server.await.unwrap();
    }
    #[test]
    fn cancelled_generation_cannot_enter_commit_window() {
        let f = Fixture::new("https://auth.test/token".into());
        let id = Uuid::new_v4();
        let lease = f
            .state
            .begin_token_request(f.workspace, f.session, id)
            .unwrap();
        f.state.cancel_token_operation(id);
        let mut committed = false;
        assert!(f
            .state
            .with_active_token(id, lease.generation(), || {
                committed = true;
                Ok(())
            })
            .is_err());
        assert!(!committed);
    }
    // Real backend preparation -> business HTTP/async-runner integration, with no Tauri mocks.
    async fn business_server(
        expected: &'static str,
        total: usize,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/business", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let task = tokio::spawn(async move {
            for _ in 0..total {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut wire = Vec::new();
                let mut buffer = [0; 8192];
                while !wire.windows(4).any(|window| window == b"\r\n\r\n") {
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    wire.extend_from_slice(&buffer[..n]);
                }
                seen.fetch_add(1, Ordering::SeqCst);
                let wire = String::from_utf8(wire).unwrap().to_ascii_lowercase();
                assert!(wire.contains(&format!(
                    "authorization: {}\r\n",
                    expected.to_ascii_lowercase()
                )));
                let body = r#"{"id":7}"#;
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        (url, count, task)
    }
    fn business_spec(url: String) -> RequestSpec {
        RequestSpec {
            method: "GET".into(),
            url,
            params: vec![],
            path_params: vec![],
            headers: vec![],
            body: Body::None,
            auth: AuthConfig::Inherit,
            settings: RequestSettings::default(),
        }
    }
    fn context(f: &Fixture, index: usize) -> apogee_core::request::ExecContext {
        apogee_core::request::ExecContext {
            request_node_id: Some(f.requests[index].to_string()),
            ephemeral: Some(false),
            ..Default::default()
        }
    }
    struct FinishRequest(Arc<AppState>, Uuid);
    impl Drop for FinishRequest {
        fn drop(&mut self) {
            self.0.finish_inflight(self.1);
        }
    }
    fn pipeline(
        f: &Fixture,
        spec: RequestSpec,
        ctx: apogee_core::request::ExecContext,
    ) -> (Uuid, tokio::task::JoinHandle<apogee_core::http::Outcome>) {
        let id = Uuid::new_v4();
        let state = f.state.clone();
        let workspace = f.workspace;
        let session = f.session;
        let lease = state.begin_request(workspace, session, id, None).unwrap();
        let (runtime, cancel, _) = lease.into_execution_parts();
        let task = tokio::spawn(async move {
            let _finish = FinishRequest(state.clone(), id);
            apogee_core::http::execute_prepared(
                id,
                crate::request_preparation::prepare(
                    state,
                    workspace,
                    session,
                    spec,
                    Some(ctx),
                    &app(),
                ),
                runtime.spool(),
                cancel,
                |_| {},
            )
            .await
        });
        (id, task)
    }
    #[tokio::test]
    async fn backend_send_preparation_refreshes_inherited_auth_without_frontend_ipc() {
        let (token_url, ready, release, token_count, token_server) = server().await;
        let f = Fixture::new(token_url);
        let (url, business_count, business) = business_server("Bearer new", 2).await;
        let (_, first) = pipeline(&f, business_spec(url.clone()), context(&f, 0));
        ready.await.unwrap();
        release.notify_one();
        assert!(matches!(
            first.await.unwrap(),
            apogee_core::http::Outcome::Done(_)
        ));
        let (_, second) = pipeline(&f, business_spec(url), context(&f, 1));
        assert!(matches!(
            second.await.unwrap(),
            apogee_core::http::Outcome::Done(_)
        ));
        token_server.await.unwrap();
        business.await.unwrap();
        assert_eq!(token_count.load(Ordering::SeqCst), 1);
        assert_eq!(business_count.load(Ordering::SeqCst), 2);
        assert!(!auth_needs_refresh(&f.auth()));
        assert!(matches!(
            f.state
                .with_workspace(f.workspace, f.session, |repo| Ok(repo
                    .read_request(f.requests[0])?
                    .spec
                    .auth))
                .unwrap(),
            AuthConfig::Inherit
        ));
    }
    #[tokio::test]
    async fn cancelled_backend_preparation_does_not_send_business_or_commit_tokens() {
        let (token_url, ready, release, _, token_server) = server().await;
        let f = Fixture::new(token_url);
        let old = f.auth();
        let (url, business_count, business) = business_server("unused", 1).await;
        let (id, task) = pipeline(&f, business_spec(url), context(&f, 0));
        ready.await.unwrap();
        assert!(f.state.cancel_inflight(id));
        release.notify_one();
        assert!(matches!(
            task.await.unwrap(),
            apogee_core::http::Outcome::Cancelled
        ));
        token_server.await.unwrap();
        business.abort();
        assert_eq!(business_count.load(Ordering::SeqCst), 0);
        assert!(equal(&f.auth(), &old));
        assert!(!f.state.cancel_inflight(id));
        assert!(f
            .state
            .oauth2_source_locks
            .lock()
            .unwrap()
            .values()
            .all(|lock| lock.strong_count() == 0));
    }
    #[tokio::test]
    async fn short_lifetime_backend_failure_keeps_rotation_but_never_sends_business_http() {
        let (token_url, ready, release, _, token_server) = server_with_body(
            r#"{"access_token":"short","refresh_token":"r1","token_type":"Bearer","expires_in":5}"#,
        )
        .await;
        let f = Fixture::new(token_url);
        let (url, business_count, business) = business_server("unused", 1).await;
        let (_, task) = pipeline(&f, business_spec(url), context(&f, 0));
        ready.await.unwrap();
        release.notify_one();
        assert!(
            matches!(task.await.unwrap(), apogee_core::http::Outcome::Failed { error, actual_request }
            if error.code.as_deref() == Some("OAUTH2_TOKEN_TOO_SHORT") && actual_request.is_none())
        );
        assert!(
            matches!(f.auth(), AuthConfig::OAuth2 { token_request: Some(config), .. } if config.refresh_token == "r1")
        );
        token_server.await.unwrap();
        business.abort();
        assert_eq!(business_count.load(Ordering::SeqCst), 0);
        assert!(f.state.meta.read().unwrap().is_empty());
    }
    #[tokio::test]
    async fn backend_pkce_reauthorization_requirement_stops_without_opening_browser_or_http() {
        let f = Fixture::new("https://not-contacted.test/token".into());
        f.state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                let mut value = repo.read_container(f.folder)?;
                if let AuthConfig::OAuth2 {
                    token_request: Some(config),
                    ..
                } = &mut value.auth
                {
                    config.refresh_token.clear();
                    config.grant_type = apogee_core::oauth2::OAuth2Flow::AuthorizationCode;
                }
                repo.save_container(value)?;
                Ok(())
            })
            .unwrap();
        let (url, count, business) = business_server("unused", 1).await;
        let (_, task) = pipeline(&f, business_spec(url), context(&f, 0));
        assert!(
            matches!(task.await.unwrap(), apogee_core::http::Outcome::Failed { error, actual_request }
            if error.code.as_deref() == Some("OAUTH2_REAUTHORIZE") && actual_request.is_none())
        );
        business.abort();
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn legacy_or_explicit_ephemeral_auth_requires_sensitive_preflight_before_rotation() {
        for ephemeral in [None, Some(true), Some(false)] {
            let f = Fixture::new("https://not-contacted.test/token".into());
            let old = f.auth();
            let (url, count, business) = business_server("unused", 1).await;
            let mut spec = business_spec(url);
            spec.auth = old.clone();
            let mut ctx = context(&f, 0);
            ctx.ephemeral = ephemeral;
            if ephemeral == Some(false) {
                ctx.request_node_id = None;
            }
            let (_, task) = pipeline(&f, spec, ctx);
            assert!(
                matches!(task.await.unwrap(), apogee_core::http::Outcome::Failed { error, actual_request }
                if error.code.as_deref() == Some("OAUTH2_EPHEMERAL_PREPARATION_REQUIRED") && actual_request.is_none())
            );
            business.abort();
            assert_eq!(count.load(Ordering::SeqCst), 0);
            assert!(equal(&old, &f.auth()));
            assert!(f.state.oauth2_source_locks.lock().unwrap().is_empty());
        }
    }
    #[tokio::test]
    async fn privately_prepared_ephemeral_credentials_can_execute_without_second_rotation() {
        let (token_url, ready, release, count, token_server) = server().await;
        let f = Fixture::new(token_url);
        let old = f.auth();
        let mut input = f.input(0);
        input.auth = old.clone();
        input.ephemeral = true;
        let acquired = start(&f, input);
        ready.await.unwrap();
        release.notify_one();
        let output = acquired.await.unwrap().unwrap().unwrap();
        let (url, _, business) = business_server("Bearer new", 1).await;
        let mut spec = business_spec(url);
        spec.auth = output.auth;
        let mut ctx = context(&f, 0);
        ctx.ephemeral = Some(true);
        let (_, task) = pipeline(&f, spec, ctx);
        assert!(matches!(
            task.await.unwrap(),
            apogee_core::http::Outcome::Done(_)
        ));
        token_server.await.unwrap();
        business.await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(equal(&old, &f.auth()));
    }
    #[tokio::test]
    async fn manual_authorization_header_suppresses_backend_token_acquisition() {
        let f = Fixture::new("https://not-contacted.test/token".into());
        let old = f.auth();
        let (url, _, business) = business_server("Bearer manual", 1).await;
        let mut spec = business_spec(url);
        spec.headers.push(apogee_core::model::KeyValue {
            enabled: true,
            key: "Authorization".into(),
            value: "Bearer manual".into(),
            description: None,
            kind: apogee_core::model::ValueKind::Text,
        });
        let (_, task) = pipeline(&f, spec, context(&f, 0));
        assert!(matches!(
            task.await.unwrap(),
            apogee_core::http::Outcome::Done(_)
        ));
        business.await.unwrap();
        assert!(equal(&old, &f.auth()));
        assert!(f.state.oauth2_source_locks.lock().unwrap().is_empty());
    }
    #[test]
    fn host_execution_snapshots_do_not_expand_sensitive_ipc() {
        let f = Fixture::new("https://auth.test/token".into());
        let settings = RequestSettings {
            proxy_url: Some("http://private-proxy-secret.test".into()),
            ..Default::default()
        };
        let output = Output {
            auth: f.auth(),
            source_id: Some(f.folder),
            token: None,
            error: None,
            environment: Some(Environment {
                id: Uuid::new_v4(),
                name: "private-environment-secret".into(),
                host: String::new(),
                variables: vec![],
            }),
            settings,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(
            !json.contains("private-proxy-secret") && !json.contains("private-environment-secret")
        );
        assert!(!json.contains("\"environment\"") && !json.contains("\"settings\""));
    }
    struct RunnerAdapter<'a>(&'a Fixture);
    impl apogee_core::automation::prepared::AsyncRequestExecutor for RunnerAdapter<'_> {
        fn prepare<'a>(
            &'a mut self,
            item: &'a apogee_core::automation::RunnerRequest,
            _: &'a std::collections::BTreeMap<String, String>,
        ) -> apogee_core::automation::prepared::RunnerFuture<
            'a,
            Option<apogee_core::http::PreparedRequest>,
        > {
            Box::pin(async move {
                crate::request_preparation::prepare(
                    self.0.state.clone(),
                    self.0.workspace,
                    self.0.session,
                    item.request.clone(),
                    Some(item.context.clone()),
                    &app(),
                )
                .await
            })
        }
        fn execute(
            &mut self,
            prepared: apogee_core::http::PreparedRequest,
        ) -> apogee_core::automation::prepared::RunnerFuture<
            '_,
            apogee_core::automation::ResponseSnapshot,
        > {
            Box::pin(async move {
                let id = Uuid::new_v4();
                let lease = self
                    .0
                    .state
                    .begin_request(self.0.workspace, self.0.session, id, None)
                    .unwrap();
                let (runtime, cancel, _) = lease.into_execution_parts();
                let _finish = FinishRequest(self.0.state.clone(), id);
                let result = apogee_core::http::execute(
                    id,
                    &prepared.spec,
                    prepared.environment.as_ref(),
                    runtime.spool(),
                    cancel,
                    |_| {},
                )
                .await;
                let apogee_core::http::Outcome::Done(meta) = result else {
                    panic!("mock business HTTP must succeed")
                };
                let body = runtime
                    .spool()
                    .read_range(id, 0, meta.size_bytes.min(65_536))
                    .unwrap();
                Ok(apogee_core::automation::ResponseSnapshot {
                    status: meta.status,
                    body: String::from_utf8(body).unwrap(),
                    ..Default::default()
                })
            })
        }
    }
    #[tokio::test]
    async fn async_collection_preparation_refreshes_shared_source_once_and_streams_results() {
        let (token_url, ready, release, token_count, token_server) = server().await;
        let f = Fixture::new(token_url);
        let (url, count, business) = business_server("Bearer new", 2).await;
        let requests: Vec<_> = (0..2)
            .map(|index| apogee_core::automation::RunnerRequest {
                name: "Request".into(),
                request: business_spec(url.clone()),
                context: context(&f, index),
                assertions: vec![apogee_core::automation::Assertion::Status { expected: 200 }],
                extractors: vec![],
            })
            .collect();
        let mut executor = RunnerAdapter(&f);
        let (_sender, cancel) = tokio::sync::oneshot::channel();
        let mut delivered = 0;
        let (result, ()) = tokio::join!(
            apogee_core::automation::prepared::run_collection_async(
                &requests,
                1,
                std::collections::BTreeMap::new(),
                &mut executor,
                cancel,
                |_| {
                    delivered += 1;
                    Ok(())
                }
            ),
            async {
                ready.await.unwrap();
                release.notify_one();
            }
        );
        let result = result.unwrap();
        assert_eq!(result.completed, 2);
        assert_eq!(result.assertions_passed, 2);
        assert_eq!(delivered, 2);
        token_server.await.unwrap();
        business.await.unwrap();
        assert_eq!(token_count.load(Ordering::SeqCst), 1);
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn backend_rejected_token_response_is_safe_and_never_sends_business() {
        let (token_url, ready, release, _, token_server) = server_with_body(
            r#"{"error":"invalid_grant","error_description":"private-refresh-secret"}"#,
        )
        .await;
        let f = Fixture::new(token_url);
        let old = f.auth();
        let (url, count, business) = business_server("unused", 1).await;
        let (_, task) = pipeline(&f, business_spec(url), context(&f, 0));
        ready.await.unwrap();
        release.notify_one();
        let apogee_core::http::Outcome::Failed {
            error,
            actual_request,
        } = task.await.unwrap()
        else {
            panic!("must stop")
        };
        assert_eq!(error.code.as_deref(), Some("REQUEST_PREPARATION_FAILED"));
        assert!(actual_request.is_none());
        assert!(!serde_json::to_string(&PublicError::from(&error))
            .unwrap()
            .contains("private-refresh-secret"));
        token_server.await.unwrap();
        business.abort();
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(equal(&old, &f.auth()));
    }
    #[tokio::test]
    async fn explicitly_persistent_request_auth_updates_only_its_own_source() {
        let (token_url, ready, release, _, token_server) = server().await;
        let f = Fixture::new(token_url);
        let old = f.auth();
        f.state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                repo.compare_and_set_auth(f.requests[0], &AuthConfig::Inherit, old.clone())
            })
            .unwrap();
        let (url, _, business) = business_server("Bearer new", 1).await;
        let mut spec = business_spec(url);
        spec.auth = old.clone();
        let (_, task) = pipeline(&f, spec, context(&f, 0));
        ready.await.unwrap();
        release.notify_one();
        assert!(matches!(
            task.await.unwrap(),
            apogee_core::http::Outcome::Done(_)
        ));
        token_server.await.unwrap();
        business.await.unwrap();
        assert!(equal(&old, &f.auth()));
        let saved = f
            .state
            .with_workspace(f.workspace, f.session, |repo| repo.read_auth(f.requests[0]))
            .unwrap();
        assert!(matches!(saved, AuthConfig::OAuth2 { access_token, .. } if access_token == "new"));
    }
    #[tokio::test]
    async fn backend_persistent_source_rejects_stale_non_token_auth_configuration() {
        let f = Fixture::new("https://not-contacted.test/token".into());
        let old = f.auth();
        let mut updated = old.clone();
        if let AuthConfig::OAuth2 {
            token_request: Some(config),
            ..
        } = &mut updated
        {
            config.client_secret = "new-client-config".into();
        }
        f.state
            .with_workspace_mutation(f.workspace, f.session, |repo| {
                repo.compare_and_set_auth(f.requests[0], &AuthConfig::Inherit, updated.clone())
            })
            .unwrap();
        let (url, count, business) = business_server("unused", 1).await;
        let mut spec = business_spec(url);
        spec.auth = old;
        let (_, task) = pipeline(&f, spec, context(&f, 0));
        assert!(
            matches!(task.await.unwrap(), apogee_core::http::Outcome::Failed { error, actual_request }
            if error.code.as_deref() == Some("OAUTH2_SOURCE_STALE") && actual_request.is_none())
        );
        business.abort();
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(equal(
            &updated,
            &f.state
                .with_workspace(f.workspace, f.session, |repo| repo.read_auth(f.requests[0]))
                .unwrap()
        ));
    }
}
