//! Shared cancellable preparation boundary for desktop and future runner/CLI adapters.
use super::{execute, Outcome, Progress};
use crate::{error::AppError, request::RequestSpec, response::Spool, vars::Environment};
use std::future::Future;
use tokio::sync::oneshot;
use uuid::Uuid;

/// Private execution snapshot. Deliberately neither Serialize nor Debug: it carries credentials.
pub struct PreparedRequest {
    pub spec: RequestSpec,
    pub environment: Option<Environment>,
}

/// One cancellation receiver spans preparation and HTTP. A failed preparation never builds or
/// sends business HTTP and has no actual-request snapshot. Dropped preparation futures must
/// release their host leases (the host OAuth2 adapter supplies its own cancellation guard).
pub async fn execute_prepared(
    id: Uuid,
    preparation: impl Future<Output = Result<Option<PreparedRequest>, AppError>>,
    spool: &Spool,
    mut cancel: oneshot::Receiver<()>,
    on_progress: impl FnMut(Progress),
) -> Outcome {
    let prepared = tokio::select! { biased;
        _ = &mut cancel => return Outcome::Cancelled,
        result = preparation => match result {
            Ok(Some(value)) => value,
            Ok(None) => return Outcome::Cancelled,
            Err(error) => return Outcome::Failed { error, actual_request: Box::new(None) },
        },
    };
    // Check cancellation again at the phase boundary, before request/body construction.
    if !matches!(cancel.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
        return Outcome::Cancelled;
    }
    execute(
        id,
        &prepared.spec,
        prepared.environment.as_ref(),
        spool,
        cancel,
        on_progress,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ErrorKind, Hint};
    #[tokio::test]
    async fn preparation_failure_has_no_business_snapshot_or_spool() {
        let root = std::env::temp_dir().join(format!("apogee-prepare-{}", Uuid::new_v4()));
        let spool = Spool::new(&root).unwrap();
        let (_sender, cancel) = oneshot::channel();
        let result = execute_prepared(
            Uuid::new_v4(),
            async {
                Err(AppError::new(ErrorKind::Io, "Preparation failed")
                    .code("OAUTH2_SOURCE_STALE")
                    .hint(Hint::new("Reload Auth")))
            },
            &spool,
            cancel,
            |_| {},
        )
        .await;
        assert!(matches!(result, Outcome::Failed { actual_request, error }
            if actual_request.is_none() && error.code.as_deref() == Some("OAUTH2_SOURCE_STALE")));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        drop(spool);
        assert!(root.starts_with(std::env::temp_dir()));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn cancellation_is_biased_before_preparation_is_polled() {
        let root = std::env::temp_dir().join(format!("apogee-prepare-{}", Uuid::new_v4()));
        let spool = Spool::new(&root).unwrap();
        let (sender, cancel) = oneshot::channel();
        sender.send(()).unwrap();
        let result = execute_prepared(
            Uuid::new_v4(),
            async { panic!("must not prepare") },
            &spool,
            cancel,
            |_| {},
        )
        .await;
        assert!(matches!(result, Outcome::Cancelled));
        drop(spool);
        assert!(root.starts_with(std::env::temp_dir()));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn cancellation_at_ready_preparation_boundary_prevents_http_construction() {
        let root = std::env::temp_dir().join(format!("apogee-prepare-{}", Uuid::new_v4()));
        let spool = Spool::new(&root).unwrap();
        let (sender, cancel) = oneshot::channel();
        let result = execute_prepared(
            Uuid::new_v4(),
            async move {
                sender.send(()).unwrap();
                Ok(Some(PreparedRequest {
                    spec: serde_json::from_value(
                        serde_json::json!({"method":"GET","url":"invalid-url"}),
                    )
                    .unwrap(),
                    environment: None,
                }))
            },
            &spool,
            cancel,
            |_| {},
        )
        .await;
        assert!(matches!(result, Outcome::Cancelled));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        drop(spool);
        assert!(root.starts_with(std::env::temp_dir()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
