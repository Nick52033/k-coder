use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::{Provider, ProviderError, ProviderRequest, ProviderStream};
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
}

impl FallbackProvider {
    pub fn new(
        targets: Vec<FallbackTarget>,
        metrics: RuntimeMetrics,
    ) -> Result<Self, ProviderError> {
        if targets.is_empty() {
            return Err(ProviderError::InvalidResponse(
                "fallback provider requires at least one target".into(),
            ));
        }
        Ok(Self { targets, metrics })
    }
}

#[async_trait]
impl Provider for FallbackProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        let mut last_error = None;
        for (index, target) in self.targets.iter().enumerate() {
            if cancellation.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let mut candidate_request = request.clone();
            candidate_request.model = target.model.clone();
            match target
                .provider
                .stream(candidate_request, cancellation.clone())
                .await
            {
                Ok(stream) => {
                    if index > 0 {
                        self.metrics.fallback();
                    }
                    return Ok(stream);
                }
                Err(error) if retryable(&error) && index + 1 < self.targets.len() => {
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            ProviderError::Request("all configured provider targets failed".into())
        }))
    }
}

fn retryable(error: &ProviderError) -> bool {
    match error {
        ProviderError::Request(_) => true,
        ProviderError::Http { status, .. } => {
            matches!(*status, 408 | 429) || (500..=599).contains(status)
        }
        ProviderError::Cancelled
        | ProviderError::InvalidResponse(_)
        | ProviderError::Interrupted => false,
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
        assert!(!retryable(&ProviderError::Http {
            status: 401,
            message: "auth".into()
        }));
        assert!(!retryable(&ProviderError::Interrupted));
        assert!(!retryable(&ProviderError::InvalidResponse("bad".into())));
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
            messages: vec![],
            tools: vec![],
        };
        assert!(
            provider
                .stream(request, CancellationToken::new())
                .await
                .is_ok()
        );
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.snapshot().unwrap().fallback_count, 1);
    }
}
