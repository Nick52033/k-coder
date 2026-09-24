use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use super::{Provider, ProviderError, ProviderEvent, ProviderRequest, ProviderStream};
use crate::advanced::RuntimeMetrics;

#[derive(Clone)]
pub struct FallbackTarget {
    pub provider: Arc<dyn Provider>,
    pub model: String,
    pub label: String,
}

pub struct FallbackProvider {
    targets: Vec<FallbackTarget>,
    metrics: RuntimeMetrics,
    stream_idle_timeout: Duration,
}

const DEFAULT_ROUTE_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(295);

impl FallbackProvider {
    pub fn new(
        targets: Vec<FallbackTarget>,
        metrics: RuntimeMetrics,
    ) -> Result<Self, ProviderError> {
        Self::new_with_idle_timeout(targets, metrics, DEFAULT_ROUTE_STREAM_IDLE_TIMEOUT)
    }

    pub fn new_with_idle_timeout(
        targets: Vec<FallbackTarget>,
        metrics: RuntimeMetrics,
        stream_idle_timeout: Duration,
    ) -> Result<Self, ProviderError> {
        if targets.is_empty() {
            return Err(ProviderError::InvalidResponse(
                "fallback provider requires at least one target".into(),
            ));
        }
        Ok(Self {
            targets,
            metrics,
            stream_idle_timeout,
        })
    }
}

#[async_trait]
impl Provider for FallbackProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        if cancellation.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let targets = self.targets.clone();
        let metrics = self.metrics.clone();
        let stream_idle_timeout = self.stream_idle_timeout;
        let events = async_stream::stream! {
            'targets: for (index, target) in targets.iter().enumerate() {
                if cancellation.is_cancelled() {
                    yield Err(ProviderError::Cancelled);
                    return;
                }
                let mut candidate_request = request.clone();
                candidate_request.model = target.model.clone();
                let attempt_cancellation = cancellation.child_token();
                let stream_result = tokio::select! {
                    _ = cancellation.cancelled() => {
                        attempt_cancellation.cancel();
                        yield Err(ProviderError::Cancelled);
                        return;
                    }
                    result = tokio::time::timeout(
                        stream_idle_timeout,
                        target.provider.stream(candidate_request, attempt_cancellation.clone()),
                    ) => result,
                };
                let mut stream = match stream_result {
                    Ok(Ok(stream)) => stream,
                    Ok(Err(error)) if retryable(&error) && index + 1 < targets.len() => {
                        attempt_cancellation.cancel();
                        continue 'targets;
                    }
                    Ok(Err(error)) => {
                        attempt_cancellation.cancel();
                        yield Err(error);
                        return;
                    }
                    Err(_) if index + 1 < targets.len() => {
                        attempt_cancellation.cancel();
                        continue 'targets;
                    }
                    Err(_) => {
                        attempt_cancellation.cancel();
                        yield Err(ProviderError::Request(format!(
                            "provider stream startup timeout: no stream after {}s",
                            stream_idle_timeout.as_secs()
                        )));
                        return;
                    }
                };

                if index > 0 {
                    metrics.fallback();
                }
                yield Ok(ProviderEvent::ModelSelected {
                    provider: target.label.clone(),
                    model: target.model.clone(),
                });

                let mut attempt_had_output = false;
                loop {
                    let event = tokio::select! {
                        _ = cancellation.cancelled() => {
                            attempt_cancellation.cancel();
                            yield Err(ProviderError::Cancelled);
                            return;
                        }
                        event = tokio::time::timeout(stream_idle_timeout, stream.next()) => {
                            match event {
                                Ok(event) => event,
                                Err(_) => Some(Err(ProviderError::Request(format!(
                                    "provider stream idle timeout: no events for {}s",
                                    stream_idle_timeout.as_secs()
                                )))),
                            }
                        }
                    };

                    match event {
                        Some(Ok(event)) => {
                            let completed = matches!(&event, ProviderEvent::Completed);
                            attempt_had_output |= event_is_output(&event);
                            yield Ok(event);
                            if completed {
                                return;
                            }
                        }
                        Some(Err(error))
                            if !attempt_had_output
                                && retryable(&error)
                                && index + 1 < targets.len() =>
                        {
                            attempt_cancellation.cancel();
                            continue 'targets;
                        }
                        Some(Err(error)) => {
                            attempt_cancellation.cancel();
                            yield Err(error);
                            return;
                        }
                        None if !attempt_had_output && index + 1 < targets.len() => {
                            attempt_cancellation.cancel();
                            continue 'targets;
                        }
                        None => {
                            attempt_cancellation.cancel();
                            yield Err(ProviderError::Interrupted);
                            return;
                        }
                    }
                }
            }
            yield Err(ProviderError::Request("all configured provider targets failed".into()));
        };
        Ok(Box::pin(events))
    }
}

fn event_is_output(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::TextDelta { .. }
            | ProviderEvent::Image { .. }
            | ProviderEvent::ReasoningSummaryDelta { .. }
            | ProviderEvent::ReasoningSummaryCompleted { .. }
            | ProviderEvent::ToolCall { .. }
            | ProviderEvent::ProviderContext { .. }
    )
}

fn retryable(error: &ProviderError) -> bool {
    match error {
        ProviderError::Request(_)
        | ProviderError::Unavailable(_)
        | ProviderError::RateLimited { .. } => true,
        ProviderError::Http { status, .. } => {
            matches!(*status, 408 | 429) || (500..=599).contains(status)
        }
        ProviderError::Interrupted => true,
        ProviderError::Cancelled
        | ProviderError::InvalidResponse(_)
        | ProviderError::InvalidToolArguments(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FixedProvider {
        calls: Arc<AtomicUsize>,
        error: Option<ProviderError>,
    }

    #[async_trait]
    impl Provider for FixedProvider {
        async fn stream(
            &self,
            _request: ProviderRequest,
            _cancellation: CancellationToken,
        ) -> Result<ProviderStream, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            Ok(Box::pin(stream::iter(vec![Ok(
                super::super::ProviderEvent::Completed,
            )])))
        }
    }

    #[test]
    fn failure_semantics_only_retry_transient_pre_stream_errors() {
        assert!(retryable(&ProviderError::Request("network".into())));
        assert!(retryable(&ProviderError::Http {
            status: 429,
            message: "busy".into()
        }));
        assert!(retryable(&ProviderError::Http {
            status: 503,
            message: "down".into()
        }));
        assert!(retryable(&ProviderError::Unavailable("overloaded".into())));
        assert!(!retryable(&ProviderError::Http {
            status: 401,
            message: "auth".into()
        }));
        assert!(retryable(&ProviderError::Interrupted));
        assert!(!retryable(&ProviderError::InvalidResponse("bad".into())));
        assert!(!retryable(&ProviderError::InvalidToolArguments(
            "bad".into()
        )));
    }

    #[tokio::test]
    async fn transient_pre_stream_failure_uses_the_next_target_and_records_it() {
        let directory = tempfile::tempdir().unwrap();
        let metrics = RuntimeMetrics::new(directory.path()).unwrap();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let provider = FallbackProvider::new(
            vec![
                FallbackTarget {
                    provider: Arc::new(FixedProvider {
                        calls: first_calls.clone(),
                        error: Some(ProviderError::Http {
                            status: 503,
                            message: "down".into(),
                        }),
                    }),
                    model: "primary".into(),
                    label: "primary".into(),
                },
                FallbackTarget {
                    provider: Arc::new(FixedProvider {
                        calls: second_calls.clone(),
                        error: None,
                    }),
                    model: "fallback".into(),
                    label: "fallback".into(),
                },
            ],
            metrics.clone(),
        )
        .unwrap();
        let request = ProviderRequest {
            schema_version: 1,
            model: "primary".into(),
            reasoning_effort: crate::protocol::ReasoningEffort::default(),
            messages: vec![],
            tools: vec![],
        };
        let mut stream = provider
            .stream(request, CancellationToken::new())
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await,
            Some(Ok(ProviderEvent::ModelSelected { provider, model }))
                if provider == "fallback" && model == "fallback"
        ));
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.snapshot().unwrap().fallback_count, 1);
    }

    struct StreamProvider {
        calls: Arc<AtomicUsize>,
        events: Vec<Result<ProviderEvent, ProviderError>>,
        pending: bool,
        pending_start: bool,
    }

    #[async_trait]
    impl Provider for StreamProvider {
        async fn stream(
            &self,
            _request: ProviderRequest,
            _cancellation: CancellationToken,
        ) -> Result<ProviderStream, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.pending_start {
                return std::future::pending().await;
            }
            let events = stream::iter(self.events.clone());
            if self.pending {
                Ok(Box::pin(events.chain(stream::pending())))
            } else {
                Ok(Box::pin(events))
            }
        }
    }

    fn provider_request() -> ProviderRequest {
        ProviderRequest {
            schema_version: 1,
            model: "primary".into(),
            reasoning_effort: crate::protocol::ReasoningEffort::default(),
            messages: vec![],
            tools: vec![],
        }
    }

    fn stream_target(
        calls: Arc<AtomicUsize>,
        model: &str,
        label: &str,
        events: Vec<Result<ProviderEvent, ProviderError>>,
        pending: bool,
    ) -> FallbackTarget {
        FallbackTarget {
            provider: Arc::new(StreamProvider {
                calls,
                events,
                pending,
                pending_start: false,
            }),
            model: model.into(),
            label: label.into(),
        }
    }

    fn test_metrics() -> (tempfile::TempDir, RuntimeMetrics) {
        let directory = tempfile::tempdir().unwrap();
        let metrics = RuntimeMetrics::new(directory.path()).unwrap();
        (directory, metrics)
    }

    #[tokio::test]
    async fn switches_after_a_zero_output_stream_disconnect() {
        let (_directory, metrics) = test_metrics();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let provider = FallbackProvider::new(
            vec![
                stream_target(first_calls.clone(), "primary", "primary", vec![], false),
                stream_target(
                    second_calls.clone(),
                    "backup-model",
                    "backup",
                    vec![Ok(ProviderEvent::Completed)],
                    false,
                ),
            ],
            metrics.clone(),
        )
        .unwrap();
        let mut events = provider
            .stream(provider_request(), CancellationToken::new())
            .await
            .unwrap();

        assert!(
            matches!(events.next().await, Some(Ok(ProviderEvent::ModelSelected { provider, .. })) if provider == "primary")
        );
        assert!(
            matches!(events.next().await, Some(Ok(ProviderEvent::ModelSelected { provider, model })) if provider == "backup" && model == "backup-model")
        );
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.snapshot().unwrap().fallback_count, 1);
    }

    #[tokio::test]
    async fn switches_after_a_zero_output_stream_error() {
        let (_directory, metrics) = test_metrics();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let provider = FallbackProvider::new(
            vec![
                stream_target(
                    first_calls.clone(),
                    "primary",
                    "primary",
                    vec![
                        Ok(ProviderEvent::RequestReady),
                        Err(ProviderError::Http {
                            status: 503,
                            message: "channel unavailable".into(),
                        }),
                    ],
                    false,
                ),
                stream_target(
                    second_calls.clone(),
                    "backup-model",
                    "backup",
                    vec![Ok(ProviderEvent::Completed)],
                    false,
                ),
            ],
            metrics,
        )
        .unwrap();
        let mut events = provider
            .stream(provider_request(), CancellationToken::new())
            .await
            .unwrap();
        assert!(
            matches!(events.next().await, Some(Ok(ProviderEvent::ModelSelected { provider, .. })) if provider == "primary")
        );
        assert!(matches!(
            events.next().await,
            Some(Ok(ProviderEvent::RequestReady))
        ));
        assert!(
            matches!(events.next().await, Some(Ok(ProviderEvent::ModelSelected { provider, .. })) if provider == "backup")
        );
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn does_not_switch_after_output_has_started() {
        let (_directory, metrics) = test_metrics();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let provider = FallbackProvider::new(
            vec![
                stream_target(
                    first_calls.clone(),
                    "primary",
                    "primary",
                    vec![
                        Ok(ProviderEvent::TextDelta {
                            delta: "partial".into(),
                        }),
                        Err(ProviderError::Request("connection reset".into())),
                    ],
                    false,
                ),
                stream_target(
                    second_calls.clone(),
                    "backup-model",
                    "backup",
                    vec![Ok(ProviderEvent::Completed)],
                    false,
                ),
            ],
            metrics,
        )
        .unwrap();
        let mut events = provider
            .stream(provider_request(), CancellationToken::new())
            .await
            .unwrap();
        assert!(
            matches!(events.next().await, Some(Ok(ProviderEvent::ModelSelected { provider, .. })) if provider == "primary")
        );
        assert!(
            matches!(events.next().await, Some(Ok(ProviderEvent::TextDelta { delta })) if delta == "partial")
        );
        assert!(matches!(
            events.next().await,
            Some(Err(ProviderError::Request(_)))
        ));
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn switches_when_the_zero_output_stream_stays_idle() {
        let (_directory, metrics) = test_metrics();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let provider = FallbackProvider::new(
            vec![
                stream_target(first_calls.clone(), "primary", "primary", vec![], true),
                stream_target(
                    second_calls.clone(),
                    "backup-model",
                    "backup",
                    vec![Ok(ProviderEvent::Completed)],
                    false,
                ),
            ],
            metrics,
        )
        .unwrap();
        let mut events = provider
            .stream(provider_request(), CancellationToken::new())
            .await
            .unwrap();
        assert!(
            matches!(events.next().await, Some(Ok(ProviderEvent::ModelSelected { provider, .. })) if provider == "primary")
        );
        let waiting = tokio::spawn(async move { events.next().await });
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(301)).await;
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("idle failover should complete before the test deadline")
            .unwrap();
        assert!(
            matches!(event, Some(Ok(ProviderEvent::ModelSelected { provider, .. })) if provider == "backup")
        );
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn idle_after_visible_output_fails_without_switching() {
        let (_directory, metrics) = test_metrics();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let provider = FallbackProvider::new(
            vec![
                stream_target(
                    first_calls.clone(),
                    "primary",
                    "primary",
                    vec![Ok(ProviderEvent::TextDelta {
                        delta: "partial".into(),
                    })],
                    true,
                ),
                stream_target(
                    second_calls.clone(),
                    "backup-model",
                    "backup",
                    vec![Ok(ProviderEvent::Completed)],
                    false,
                ),
            ],
            metrics,
        )
        .unwrap();
        let mut events = provider
            .stream(provider_request(), CancellationToken::new())
            .await
            .unwrap();
        assert!(
            matches!(events.next().await, Some(Ok(ProviderEvent::ModelSelected { provider, .. })) if provider == "primary")
        );
        assert!(
            matches!(events.next().await, Some(Ok(ProviderEvent::TextDelta { delta })) if delta == "partial")
        );
        let waiting = tokio::spawn(async move { events.next().await });
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(301)).await;
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("idle timeout should terminate the stream")
            .unwrap();
        assert!(
            matches!(event, Some(Err(ProviderError::Request(message))) if message.contains("idle timeout"))
        );
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn switches_when_provider_stream_start_never_resolves() {
        let (_directory, metrics) = test_metrics();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let provider = FallbackProvider::new(
            vec![
                FallbackTarget {
                    provider: Arc::new(StreamProvider {
                        calls: first_calls.clone(),
                        events: vec![],
                        pending: false,
                        pending_start: true,
                    }),
                    model: "primary".into(),
                    label: "primary".into(),
                },
                stream_target(
                    second_calls.clone(),
                    "backup-model",
                    "backup",
                    vec![Ok(ProviderEvent::Completed)],
                    false,
                ),
            ],
            metrics,
        )
        .unwrap();
        let mut events = provider
            .stream(provider_request(), CancellationToken::new())
            .await
            .unwrap();
        let waiting = tokio::spawn(async move { events.next().await });
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(301)).await;
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("provider startup timeout should start the backup route")
            .unwrap();
        assert!(
            matches!(event, Some(Ok(ProviderEvent::ModelSelected { provider, .. })) if provider == "backup")
        );
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
    }
}
