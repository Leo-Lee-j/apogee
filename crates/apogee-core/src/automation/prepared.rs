//! Async, cancellable runner primitive. Not a desktop Runner command or a public result DTO.
//! Each request must pass host preparation before transport. Results stream to a host-owned sink;
//! this module never retains complete responses, extracted secrets, or completed credentials.
use super::{
    evaluate_assertions, extract_variables, substitute_variables, ResponseSnapshot, RunnerRequest,
    RunnerResult,
};
use crate::{
    error::{AppError, ErrorKind, Hint},
    http::PreparedRequest,
};
use std::{collections::BTreeMap, future::Future, pin::Pin};
use tokio::sync::oneshot;

pub type RunnerFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AppError>> + Send + 'a>>;
pub trait AsyncRequestExecutor {
    /// Host resolves provenance, environment and variables and applies opt-in Auth preparation.
    /// Returning None cancels the run. Hosts must release token/session leases when dropped.
    fn prepare<'a>(
        &'a mut self,
        item: &'a RunnerRequest,
        variables: &'a BTreeMap<String, String>,
    ) -> RunnerFuture<'a, Option<PreparedRequest>>;
    fn execute(&mut self, request: PreparedRequest) -> RunnerFuture<'_, ResponseSnapshot>;
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub completed: usize,
    pub assertions_passed: usize,
    pub assertions_failed: usize,
    pub cancelled: bool,
}
fn failure(code: &str) -> AppError {
    AppError::new(ErrorKind::Script, "Collection execution stopped")
        .code(code)
        .hint(Hint::new(
            "Check run limits, preparation and extraction configuration",
        ))
}
fn variables_bounded(values: &BTreeMap<String, String>) -> bool {
    values.len() <= 256
        && values
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum::<usize>()
            <= 65_536
}
fn response_bounded(value: &ResponseSnapshot) -> bool {
    value.body.len() <= 65_536
        && value.headers.len() <= 128
        && value
            .headers
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum::<usize>()
            <= 16_384
}

/// Sequential async foundation (no UI, scripts, concurrent scheduler or CSV support).
/// Preparation/transport errors stop the chain without replay. Each iteration starts with the
/// initial variables; extraction affects later items only. Sink values are PRIVATE and may carry
/// business response data/secrets: a host must project them before IPC/history/logging.
pub async fn run_collection_async<E: AsyncRequestExecutor>(
    requests: &[RunnerRequest],
    iterations: usize,
    initial: BTreeMap<String, String>,
    executor: &mut E,
    mut cancel: oneshot::Receiver<()>,
    mut sink: impl FnMut(RunnerResult) -> Result<(), AppError>,
) -> Result<RunSummary, AppError> {
    let iterations = iterations.max(1);
    if requests.len() > 1000
        || iterations > 100
        || requests
            .len()
            .checked_mul(iterations)
            .is_none_or(|count| count > 10_000)
        || !variables_bounded(&initial)
        || requests
            .iter()
            .any(|item| item.assertions.len() > 128 || item.extractors.len() > 128)
    {
        return Err(failure("RUN_LIMIT_EXCEEDED"));
    }
    let mut summary = RunSummary::default();
    for _ in 0..iterations {
        let mut variables = initial.clone();
        for item in requests {
            let mut item = item.clone();
            item.request.url = substitute_variables(&item.request.url, &variables);
            for header in &mut item.request.headers {
                header.key = substitute_variables(&header.key, &variables);
                header.value = substitute_variables(&header.value, &variables);
            }
            let prepared = tokio::select! { biased;
                _ = &mut cancel => { summary.cancelled = true; return Ok(summary); },
                result = executor.prepare(&item, &variables) => match result? {
                    Some(value) => value,
                    None => { summary.cancelled = true; return Ok(summary); },
                },
            };
            if !matches!(cancel.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
                summary.cancelled = true;
                return Ok(summary);
            }
            let response = tokio::select! { biased;
                _ = &mut cancel => { summary.cancelled = true; return Ok(summary); },
                result = executor.execute(prepared) => result?,
            };
            if !response_bounded(&response) {
                return Err(failure("RUN_RESPONSE_LIMIT_EXCEEDED"));
            }
            let assertions = evaluate_assertions(&item.assertions, &response);
            let extracted = extract_variables(&item.extractors, &response)
                .map_err(|_| failure("RUN_EXTRACTION_FAILED"))?;
            let mut next = variables.clone();
            next.extend(extracted.clone());
            if !variables_bounded(&next) {
                return Err(failure("RUN_VARIABLE_LIMIT_EXCEEDED"));
            }
            summary.assertions_passed += assertions.iter().filter(|result| result.passed).count();
            summary.assertions_failed += assertions.iter().filter(|result| !result.passed).count();
            sink(RunnerResult {
                name: item.name,
                response,
                assertions,
                variables: extracted,
            })?;
            variables = next;
            summary.completed += 1;
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        automation::{Assertion, Extractor},
        request::{ExecContext, RequestSpec},
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Executor {
        prepare_count: Arc<AtomicUsize>,
        execute_count: Arc<AtomicUsize>,
        fail: bool,
        prepare_pending: bool,
        execute_pending: bool,
        oversized: bool,
        urls: Vec<String>,
    }
    impl Executor {
        fn new() -> Self {
            Self {
                prepare_count: Arc::new(AtomicUsize::new(0)),
                execute_count: Arc::new(AtomicUsize::new(0)),
                fail: false,
                prepare_pending: false,
                execute_pending: false,
                oversized: false,
                urls: vec![],
            }
        }
    }
    impl AsyncRequestExecutor for Executor {
        fn prepare<'a>(
            &'a mut self,
            item: &'a RunnerRequest,
            _: &'a BTreeMap<String, String>,
        ) -> RunnerFuture<'a, Option<PreparedRequest>> {
            Box::pin(async move {
                self.prepare_count.fetch_add(1, Ordering::SeqCst);
                if self.prepare_pending {
                    std::future::pending::<()>().await;
                }
                if self.fail {
                    return Err(failure("OAUTH2_REAUTHORIZE"));
                }
                Ok(Some(PreparedRequest {
                    spec: item.request.clone(),
                    environment: None,
                }))
            })
        }
        fn execute(&mut self, request: PreparedRequest) -> RunnerFuture<'_, ResponseSnapshot> {
            Box::pin(async move {
                self.execute_count.fetch_add(1, Ordering::SeqCst);
                if self.execute_pending {
                    std::future::pending::<()>().await;
                }
                self.urls.push(request.spec.url);
                Ok(ResponseSnapshot {
                    status: 200,
                    body: if self.oversized {
                        "x".repeat(65_537)
                    } else {
                        r#"{"id":7}"#.into()
                    },
                    ..Default::default()
                })
            })
        }
    }
    fn item() -> RunnerRequest {
        RunnerRequest {
            name: "Fixture".into(),
            request: serde_json::from_value(
                serde_json::json!({"method":"GET","url":"https://fixture.test/{{id}}"}),
            )
            .unwrap(),
            context: ExecContext::default(),
            assertions: vec![Assertion::Status { expected: 200 }],
            extractors: vec![],
        }
    }
    #[tokio::test]
    async fn prep_failure_stops_transport_and_remaining_chain() {
        let mut executor = Executor::new();
        executor.fail = true;
        let (_sender, cancel) = oneshot::channel();
        let error = run_collection_async(
            &[item(), item()],
            1,
            BTreeMap::new(),
            &mut executor,
            cancel,
            |_| Ok(()),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("OAUTH2_REAUTHORIZE"));
        assert_eq!(executor.prepare_count.load(Ordering::SeqCst), 1);
        assert_eq!(executor.execute_count.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn cancellation_during_either_phase_stops_remaining_items() {
        for prepare in [true, false] {
            let mut executor = Executor::new();
            executor.prepare_pending = prepare;
            executor.execute_pending = !prepare;
            let count = if prepare {
                executor.prepare_count.clone()
            } else {
                executor.execute_count.clone()
            };
            let (sender, cancel) = oneshot::channel();
            let cancel_task = tokio::spawn(async move {
                while count.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
                sender.send(()).unwrap();
            });
            let result = run_collection_async(
                &[item(), item()],
                1,
                BTreeMap::new(),
                &mut executor,
                cancel,
                |_| Ok(()),
            )
            .await
            .unwrap();
            cancel_task.await.unwrap();
            assert!(result.cancelled);
            assert_eq!(result.completed, 0);
            assert_eq!(executor.prepare_count.load(Ordering::SeqCst), 1);
            assert_eq!(
                executor.execute_count.load(Ordering::SeqCst),
                usize::from(!prepare)
            );
        }
    }
    #[tokio::test]
    async fn order_extraction_iteration_reset_and_streaming_results() {
        let mut first = item();
        first.extractors.push(Extractor::JsonPath {
            name: "id".into(),
            path: "$.id".into(),
        });
        let mut executor = Executor::new();
        let mut initial = BTreeMap::new();
        initial.insert("id".into(), "start".into());
        let (_sender, cancel) = oneshot::channel();
        let mut delivered = 0;
        let result =
            run_collection_async(&[first, item()], 2, initial, &mut executor, cancel, |_| {
                delivered += 1;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            executor.urls,
            [
                "https://fixture.test/start",
                "https://fixture.test/7",
                "https://fixture.test/start",
                "https://fixture.test/7"
            ]
        );
        assert_eq!(result.completed, 4);
        assert_eq!(result.assertions_passed, 4);
        assert_eq!(delivered, 4);
    }
    #[tokio::test]
    async fn run_limits_reject_before_preparation_or_transport() {
        let mut executor = Executor::new();
        let (_sender, cancel) = oneshot::channel();
        let result = run_collection_async(
            &[item()],
            101,
            BTreeMap::new(),
            &mut executor,
            cancel,
            |_| Ok(()),
        )
        .await;
        assert_eq!(
            result.unwrap_err().code.as_deref(),
            Some("RUN_LIMIT_EXCEEDED")
        );
        assert_eq!(executor.prepare_count.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn snapshot_and_variable_limits_are_bounded() {
        let response = ResponseSnapshot {
            body: "x".repeat(65_537),
            ..Default::default()
        };
        assert!(!response_bounded(&response));
        let variables = BTreeMap::from([("credential".into(), "x".repeat(65_536))]);
        assert!(!variables_bounded(&variables));
        let _: RequestSpec = item().request;
    }
    #[tokio::test]
    async fn extraction_failure_stops_next_item_and_does_not_echo_private_pattern() {
        let mut first = item();
        first.extractors.push(Extractor::JsonPath {
            name: "credential".into(),
            path: "$.private-refresh-secret".into(),
        });
        let mut executor = Executor::new();
        let (_sender, cancel) = oneshot::channel();
        let error = run_collection_async(
            &[first, item()],
            1,
            BTreeMap::new(),
            &mut executor,
            cancel,
            |_| panic!("failed item cannot be delivered"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("RUN_EXTRACTION_FAILED"));
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("private-refresh-secret"));
        assert_eq!(executor.execute_count.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn oversized_response_is_not_delivered_to_result_sink_or_next_item() {
        let mut executor = Executor::new();
        executor.oversized = true;
        let (_sender, cancel) = oneshot::channel();
        let error = run_collection_async(
            &[item(), item()],
            1,
            BTreeMap::new(),
            &mut executor,
            cancel,
            |_| panic!("oversized body must not be delivered"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("RUN_RESPONSE_LIMIT_EXCEEDED"));
        assert_eq!(executor.execute_count.load(Ordering::SeqCst), 1);
    }
}
