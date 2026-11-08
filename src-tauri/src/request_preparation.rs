//! Backend pre-send boundary shared by ordinary sends and future automation adapters.
//! No Token values, provider responses, or environment values enter ordinary events/history.
use crate::{
    app_settings::{apply_network_defaults, AppSettings},
    oauth2::managed,
    state::AppState,
};
use apogee_core::{
    error::{AppError, ErrorKind, Hint},
    http::PreparedRequest,
    public::PublicError,
    request::{AuthConfig, ExecContext, RequestSpec},
};
use std::sync::Arc;
use uuid::Uuid;

fn failure(code: &str) -> AppError {
    AppError::new(ErrorKind::Io, "请求认证准备失败，未发送业务请求")
        .code(code)
        .hint(Hint::new("检查请求来源、环境和认证配置后重试"))
}
fn identifier(value: Option<&str>) -> Result<Option<Uuid>, AppError> {
    value
        .map(|value| {
            Uuid::parse_str(value).map_err(|_| failure("REQUEST_PREPARATION_CONTEXT_INVALID"))
        })
        .transpose()
}
pub(crate) fn validate_context(ctx: Option<&ExecContext>) -> Result<(), AppError> {
    identifier(ctx.and_then(|ctx| ctx.request_node_id.as_deref()))?;
    identifier(ctx.and_then(|ctx| ctx.environment_id.as_deref()))?;
    Ok(())
}
fn public_failure(value: PublicError) -> AppError {
    let mut error = failure(
        value
            .code
            .as_deref()
            .unwrap_or("REQUEST_PREPARATION_FAILED"),
    );
    error.hints = value
        .hints
        .into_iter()
        .map(|hint| Hint {
            label: hint.label,
            action: hint.action,
        })
        .collect();
    error
}
struct TokenCancellation {
    state: Arc<AppState>,
    id: Uuid,
    armed: bool,
}
impl Drop for TokenCancellation {
    fn drop(&mut self) {
        if self.armed {
            self.state.cancel_token_operation(self.id);
        }
    }
}

/// Freeze inherited settings and environment with the effective Auth. Explicit ephemeral Auth
/// never writes its request node. Legacy callers are ephemeral by default, not implicitly saved.
pub(crate) async fn prepare(
    state: Arc<AppState>,
    workspace: Uuid,
    session: Uuid,
    spec: RequestSpec,
    ctx: Option<ExecContext>,
    app: &AppSettings,
) -> Result<Option<PreparedRequest>, AppError> {
    let node = identifier(ctx.as_ref().and_then(|ctx| ctx.request_node_id.as_deref()))?;
    let environment_id = identifier(ctx.as_ref().and_then(|ctx| ctx.environment_id.as_deref()))?;
    let ephemeral = node.is_none() || ctx.as_ref().and_then(|ctx| ctx.ephemeral).unwrap_or(true);
    let (mut frozen, persisted_auth) = state
        .with_workspace(workspace, session, |repo| {
            let effective = match node {
                Some(node) => repo.resolve_request_inheritance(node, spec.clone())?,
                None => spec.clone(),
            };
            let persisted_auth = if !ephemeral && !matches!(spec.auth, AuthConfig::Inherit) {
                node.map(|id| repo.read_auth(id)).transpose()?
            } else {
                None
            };
            Ok((
                PreparedRequest {
                    spec: effective,
                    environment: environment_id
                        .map(|id| repo.read_environment(id))
                        .transpose()?,
                },
                persisted_auth,
            ))
        })
        .map_err(|_| failure("REQUEST_PREPARATION_SOURCE_STALE"))?;
    apply_network_defaults(&mut frozen.spec.settings, app);
    let explicit_authorization = spec
        .headers
        .iter()
        .any(|header| header.enabled && header.key.trim().eq_ignore_ascii_case("authorization"));
    let refresh_enabled = matches!(
        &frozen.spec.auth,
        AuthConfig::OAuth2 {
            auto_refresh: true,
            ..
        }
    );
    if explicit_authorization || !refresh_enabled {
        return Ok(Some(frozen));
    }

    if !ephemeral
        && !matches!(spec.auth, AuthConfig::Inherit)
        && persisted_auth.as_ref().is_none_or(|saved| {
            serde_json::to_value(saved).ok() != serde_json::to_value(&spec.auth).ok()
        })
    {
        // Persistent ownership is not permission to overwrite an old loaded Auth configuration.
        return Err(failure("OAUTH2_SOURCE_STALE"));
    }
    if ephemeral && !matches!(spec.auth, AuthConfig::Inherit) {
        // Ordinary send events cannot return credentials. A draft's rotation would otherwise be
        // lost after execution, so only its dedicated sensitive preflight may acquire/refresh.
        let context = apogee_core::oauth2::issuance_context(
            &frozen.spec.auth,
            frozen.environment.as_ref(),
            &frozen.spec.settings,
            environment_id,
        )
        .map_err(|_| failure("REQUEST_PREPARATION_FAILED"))?;
        let changed = matches!(&frozen.spec.auth, AuthConfig::OAuth2 { token_context: Some(value), .. } if value != &context);
        if apogee_core::oauth2::auth_needs_refresh(&frozen.spec.auth) || changed {
            return Err(failure("OAUTH2_EPHEMERAL_PREPARATION_REQUIRED"));
        }
        return Ok(Some(frozen));
    }

    let operation = Uuid::new_v4();
    let owner_id = ctx
        .as_ref()
        .and_then(|ctx| ctx.tab_id.clone())
        .unwrap_or_else(|| node.unwrap_or(operation).to_string());
    let mut cancellation = TokenCancellation {
        state: state.clone(),
        id: operation,
        armed: true,
    };
    let result = managed::run(
        state,
        workspace,
        session,
        managed::Input {
            operation_id: operation,
            node_id: node,
            owner_id,
            environment_id,
            auth: spec.auth,
            expected_auth: persisted_auth,
            grant: None,
            settings: spec.settings,
            ephemeral,
        },
        app,
    )
    .await;
    cancellation.armed = false;
    let Some(output) = result.map_err(public_failure)? else {
        return Ok(None);
    };
    // managed already kept any rotated credentials, including short-lived results.
    if let Some(error) = output.error {
        return Err(public_failure(error));
    }
    frozen.spec.auth = output.auth;
    frozen.spec.settings = output.settings;
    frozen.environment = output.environment;
    Ok(Some(frozen))
}
