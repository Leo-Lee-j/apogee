//! Session-bound credential operations, separate from response/history event pipelines.
pub(crate) mod managed;
use crate::{
    app_settings::{apply_network_defaults, AppSettings},
    state::AppState,
};
use apogee_core::{
    oauth2::{OAuth2Grant, OAuth2TokenRequest, OAuth2TokenResponse},
    public::PublicError,
    request::{AuthConfig, Body, RequestSettings, RequestSpec},
    AppError, ErrorKind, Hint,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenOperationInput {
    pub operation_id: Uuid,
    pub node_id: Option<Uuid>,
    pub environment_id: Option<Uuid>,
    pub request: OAuth2TokenRequest,
    pub grant: OAuth2Grant,
    #[serde(default)]
    pub settings: RequestSettings,
}
fn failure(message: &str) -> PublicError {
    failure_with_code(message, "OAUTH2_OPERATION_INVALID")
}
fn failure_with_code(message: &str, code: &str) -> PublicError {
    PublicError::from(
        &AppError::new(ErrorKind::Io, message)
            .code(code)
            .hint(Hint::new("确认工作区、环境和当前请求仍有效后重试")),
    )
}
struct InFlightGuard {
    state: Arc<AppState>,
    id: Uuid,
    generation: Uuid,
}
impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.state
            .finish_inflight_generation(self.id, self.generation);
    }
}

pub async fn fetch(
    state: Arc<AppState>,
    workspace_id: Uuid,
    session_id: Uuid,
    input: TokenOperationInput,
    app_settings: &AppSettings,
) -> Result<Option<OAuth2TokenResponse>, PublicError> {
    fetch_with_browser(
        state,
        workspace_id,
        session_id,
        input,
        app_settings,
        open_browser,
    )
    .await
}
async fn fetch_with_browser<F>(
    state: Arc<AppState>,
    workspace_id: Uuid,
    session_id: Uuid,
    input: TokenOperationInput,
    app_settings: &AppSettings,
    open_browser: F,
) -> Result<Option<OAuth2TokenResponse>, PublicError>
where
    F: FnOnce(&url::Url) -> Result<(), AppError>,
{
    let (mut settings, environment) = state
        .with_workspace(workspace_id, session_id, |repository| {
            let settings = if let Some(id) = input.node_id {
                repository
                    .resolve_token_request_inheritance(
                        id,
                        RequestSpec {
                            method: "POST".into(),
                            url: input.request.token_url.clone(),
                            params: Vec::new(),
                            path_params: Vec::new(),
                            headers: Vec::new(),
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
                .map(|id| repository.read_environment(id))
                .transpose()?;
            Ok((settings, environment))
        })
        .map_err(|_| failure("无法读取 Token 请求所属工作区、环境或网络设置"))?;
    apply_network_defaults(&mut settings, app_settings);
    let lease = match state.begin_token_request(workspace_id, session_id, input.operation_id) {
        Ok(lease) => lease,
        Err(error) if error.to_string() == "TOKEN_OPERATION_CANCELLED" => return Ok(None),
        Err(_) => return Err(failure("无法启动 Token 请求：会话已失效或操作 id 已在使用")),
    };
    let _cleanup = InFlightGuard {
        state: state.clone(),
        id: input.operation_id,
        generation: lease.generation(),
    };
    let (_runtime, cancel, access) = lease.into_execution_parts();
    let result = if matches!(input.grant, OAuth2Grant::AuthorizationCode) {
        apogee_core::oauth2::authorize(
            &input.request,
            environment.as_ref(),
            &settings.resolve(),
            cancel,
            |url| {
                // No detached browser task can open after cancellation or a workspace switch.
                state
                    .with_active_response_access(access, || open_browser(&url).map_err(Into::into))
                    .map_err(|_| {
                        AppError::new(ErrorKind::Io, "无法打开系统浏览器或授权会话已失效")
                            .hint(Hint::new("确认系统默认浏览器可用，并回到目标工作区重试"))
                    })
            },
        )
        .await
    } else {
        apogee_core::oauth2::fetch_token(
            &input.request,
            input.grant,
            environment.as_ref(),
            &settings.resolve(),
            cancel,
        )
        .await
    };
    // A close/reopen may occur while network IO is in progress. Never return credentials
    // belonging to an old session to a newly active workspace, even after HTTP success.
    state
        .with_active_response_access(access, || Ok(()))
        .map_err(|_| {
            failure_with_code(
                "Token 请求所属工作区会话已失效，结果已丢弃",
                "OAUTH2_SESSION_STALE",
            )
        })?;
    result.map_err(|error| PublicError::from(&error))
}

// Only the core-validated OAuth authorization URL reaches this adapter.
// This is not exposed as a general URL-opening IPC or a Tauri capability.
fn open_browser(url: &url::Url) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        };
        // Shell browser handlers may require COM, while IPC runs on a Tokio worker.
        // Balance only our successful initialization, on this same synchronous thread.
        struct ComGuard(bool);
        impl Drop for ComGuard {
            fn drop(&mut self) {
                if self.0 {
                    // SAFETY: this guard balances a successful initialization on this thread.
                    unsafe {
                        CoUninitialize();
                    }
                }
            }
        }
        // SAFETY: no reserved data; this adapter contains no await/thread migration.
        let hr = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        };
        let _com = ComGuard(hr >= 0);
        // RPC_E_CHANGED_MODE means the caller already initialized a different apartment.
        if hr < 0 && hr != 0x80010106_u32 as i32 {
            return Err(AppError::new(ErrorKind::Io, "无法初始化系统浏览器组件"));
        }

        let wide = |value: &str| {
            std::ffi::OsStr::new(value)
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        let verb = wide("open");
        let target = wide(url.as_str());
        // SAFETY: terminated UTF-16 strings remain alive throughout the call.
        let result = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if result as isize > 32 {
            return Ok(());
        }
    }
    #[cfg(target_os = "macos")]
    {
        if std::process::Command::new("/usr/bin/open")
            .arg("--")
            .arg(url.as_str())
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = url;
    Err(AppError::new(ErrorKind::Io, "无法打开系统浏览器"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::WorkspaceRepository;
    #[test]
    fn duplicate_ids_and_stale_cleanup_cannot_replace_or_remove_active_operation() {
        let root = std::env::temp_dir().join(format!("apogee-token-lease-{}", Uuid::new_v4()));
        let state = Arc::new(AppState::new().unwrap());
        let repository =
            WorkspaceRepository::create(root.join("workspace"), "Auth".into()).unwrap();
        let (workspace, session) = state
            .open_workspace(repository, root.join("runtime"))
            .unwrap();
        let id = Uuid::new_v4();
        let lease = state.begin_request(workspace, session, id, None).unwrap();
        let cleanup = InFlightGuard {
            state: state.clone(),
            id,
            generation: lease.generation(),
        };
        assert!(state.begin_request(workspace, session, id, None).is_err());
        assert!(state.cancel_inflight(id));
        let new_lease = state.begin_request(workspace, session, id, None).unwrap();
        drop(cleanup);
        assert!(state.cancel_inflight(id));
        drop(lease);
        drop(new_lease);
        state.close_workspace(workspace, session).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn credential_failures_are_public_and_do_not_register_a_response_or_history() {
        let root = std::env::temp_dir().join(format!("apogee-token-failure-{}", Uuid::new_v4()));
        let state = Arc::new(AppState::new().unwrap());
        let repository =
            WorkspaceRepository::create(root.join("workspace"), "Auth".into()).unwrap();
        let (workspace, session) = state
            .open_workspace(repository, root.join("runtime"))
            .unwrap();
        let id = Uuid::new_v4();
        let input = TokenOperationInput {
            operation_id: id,
            node_id: None,
            environment_id: None,
            request: OAuth2TokenRequest {
                token_url: "https://user:private-secret@auth.test/token".into(),
                client_id: "client".into(),
                client_secret: "private-secret".into(),
                ..Default::default()
            },
            grant: OAuth2Grant::ClientCredentials,
            settings: Default::default(),
        };
        let error = fetch(
            state.clone(),
            workspace,
            session,
            input,
            &serde_json::from_value(
                serde_json::json!({"defaultWorkspaceRoot":root.join("defaults")}),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("private-secret"));
        assert!(!state.cancel_inflight(id));
        assert!(!root
            .join("runtime")
            .join("responses")
            .join(session.to_string())
            .join(format!("{id}.body"))
            .exists());
        state.close_workspace(workspace, session).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn token_results_are_isolated_and_old_sessions_never_receive_credentials() {
        use std::io::{Read, Write};
        for close_session in [false, true] {
            let root =
                std::env::temp_dir().join(format!("apogee-token-success-{}", Uuid::new_v4()));
            let state = Arc::new(AppState::new().unwrap());
            let repository =
                WorkspaceRepository::create(root.join("workspace"), "Auth".into()).unwrap();
            let (workspace, session) = state
                .open_workspace(repository, root.join("runtime"))
                .unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/token", listener.local_addr().unwrap());
            let (accepted, ready) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut buffer = [0; 4096];
                let mut received = Vec::new();
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    received.extend_from_slice(&buffer[..count]);
                    if let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&received[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if received.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let _ = accepted.send(());
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                let body = r#"{"access_token":"private-access","token_type":"Bearer","refresh_token":"private-refresh"}"#;
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                let _ = stream.write_all(response.as_bytes());
            });
            let id = Uuid::new_v4();
            let input = TokenOperationInput {
                operation_id: id,
                node_id: None,
                environment_id: None,
                request: OAuth2TokenRequest {
                    token_url: url,
                    client_id: "id".into(),
                    client_secret: "secret".into(),
                    ..Default::default()
                },
                grant: OAuth2Grant::ClientCredentials,
                settings: RequestSettings {
                    proxy_url: Some("direct://".into()),
                    ..Default::default()
                },
            };
            let app_settings = serde_json::from_value(
                serde_json::json!({"defaultWorkspaceRoot": root.join("defaults")}),
            )
            .unwrap();
            let task_state = state.clone();
            let task = tokio::spawn(async move {
                fetch(task_state, workspace, session, input, &app_settings).await
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), ready)
                .await
                .unwrap()
                .unwrap();
            if close_session {
                state.close_workspace(workspace, session).unwrap();
                let repository = WorkspaceRepository::open(root.join("workspace")).unwrap();
                state
                    .open_workspace(repository, root.join("runtime"))
                    .unwrap();
            }
            release.send(()).unwrap();
            let result = task.await.unwrap();
            server.join().unwrap();
            if close_session {
                let public = serde_json::to_string(&result.unwrap_err()).unwrap();
                assert!(!public.contains("private-access"));
            } else {
                assert_eq!(result.unwrap().unwrap().access_token, "private-access");
            }
            assert!(state.meta.read().unwrap().is_empty());
            assert!(!state.cancel_inflight(id));
            let active = state
                .workspace
                .read()
                .unwrap()
                .as_ref()
                .map(|s| (s.workspace_id, s.id))
                .unwrap();
            state.close_workspace(active.0, active.1).unwrap();
            assert!(!root.join("runtime").join("history.yaml").exists());
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[tokio::test]
    async fn pkce_wait_is_session_bound_cancelled_and_never_publishes_runtime_data() {
        for close_session in [false, true] {
            let root = std::env::temp_dir().join(format!("apogee-pkce-lease-{}", Uuid::new_v4()));
            let state = Arc::new(AppState::new().unwrap());
            let repository =
                WorkspaceRepository::create(root.join("workspace"), "PKCE".into()).unwrap();
            let (workspace, session) = state
                .open_workspace(repository, root.join("runtime"))
                .unwrap();
            let reservation =
                std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
            let port = reservation.local_addr().unwrap().port();
            drop(reservation);
            let id = Uuid::new_v4();
            let input = TokenOperationInput {
                operation_id: id,
                node_id: None,
                environment_id: None,
                request: OAuth2TokenRequest {
                    token_url: "https://provider.test/token".into(),
                    client_id: "client".into(),
                    authorization_url: "https://provider.test/authorize".into(),
                    redirect_uri: format!("http://127.0.0.1:{port}/cb"),
                    ..Default::default()
                },
                grant: OAuth2Grant::AuthorizationCode,
                settings: RequestSettings {
                    proxy_url: Some("direct://".into()),
                    ..Default::default()
                },
            };
            let app_settings: AppSettings = serde_json::from_value(
                serde_json::json!({"defaultWorkspaceRoot": root.join("defaults")}),
            )
            .unwrap();
            let (opened, ready) = tokio::sync::oneshot::channel();
            let task_state = state.clone();
            let task = tokio::spawn(async move {
                fetch_with_browser(
                    task_state,
                    workspace,
                    session,
                    input,
                    &app_settings,
                    |url| {
                        assert!(url
                            .query_pairs()
                            .any(|(k, v)| k == "code_challenge_method" && v == "S256"));
                        opened.send(()).unwrap();
                        Ok(())
                    },
                )
                .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), ready)
                .await
                .unwrap()
                .unwrap();
            if close_session {
                state.close_workspace(workspace, session).unwrap();
                state
                    .open_workspace(
                        WorkspaceRepository::open(root.join("workspace")).unwrap(),
                        root.join("runtime"),
                    )
                    .unwrap();
            } else {
                assert!(state.cancel_inflight(id));
            }
            let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
            if close_session {
                assert_eq!(
                    result.unwrap_err().code.as_deref(),
                    Some("OAUTH2_SESSION_STALE")
                );
            } else {
                assert!(result.unwrap().is_none());
            }
            assert!(std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_ok());
            assert!(!state.cancel_inflight(id));
            assert!(state.meta.read().unwrap().is_empty());
            let active = state
                .workspace
                .read()
                .unwrap()
                .as_ref()
                .map(|s| (s.workspace_id, s.id))
                .unwrap();
            state.close_workspace(active.0, active.1).unwrap();
            assert!(!root.join("runtime").join("history.yaml").exists());
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
