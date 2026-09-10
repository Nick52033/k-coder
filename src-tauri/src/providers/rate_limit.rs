//! Account-scoped admission control, shared across rebuilt providers and child runtimes.
//! No credentials, prompts or model history are stored here.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{Provider, ProviderError, ProviderEvent, ProviderRequest, ProviderStream};

#[derive(Default)]
pub struct RateLimitRegistry(Mutex<HashMap<String, Arc<Mutex<Admission>>>>);

impl RateLimitRegistry {
    pub fn wrap(&self, account: &str, provider: Arc<dyn Provider>) -> Arc<dyn Provider> {
        let admission = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(account.to_owned())
            .or_default()
            .clone();
        Arc::new(RateLimitedProvider {
            inner: provider,
            admission,
        })
    }
}

#[derive(Default)]
struct Admission {
    next_request: Option<Instant>,
    spacing: Duration,
}

impl Admission {
    fn admit(&mut self) -> Option<Duration> {
        let now = Instant::now();
        if let Some(deadline) = self.next_request.filter(|deadline| *deadline > now) {
            return Some(deadline - now);
        }
        self.next_request = Some(now + self.spacing);
        None
    }

    fn limited(&mut self, delay: Duration) {
        let deadline = Instant::now() + delay;
        self.next_request = Some(self.next_request.map_or(deadline, |old| old.max(deadline)));
        // Start conservatively after the first 429; repeated limits slow subsequent
        // requests further. This is adaptive spacing, not an assumed provider quota.
        self.spacing = (self.spacing * 2).clamp(Duration::from_secs(4), Duration::from_secs(60));
    }
}

pub struct RateLimitedProvider {
    inner: Arc<dyn Provider>,
    admission: Arc<Mutex<Admission>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::testing::FakeProvider;

    fn request() -> ProviderRequest {
        ProviderRequest {
            schema_version: 1,
            model: "fixture".into(),
            reasoning_effort: Default::default(),
            messages: vec![],
            tools: vec![],
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shared_cooldown_survives_rebuild_spaces_requests_and_isolates_accounts() {
        let registry = RateLimitRegistry::default();
        let failed = Arc::new(FakeProvider::new(vec![Err(ProviderError::RateLimited {
            message: "limited".into(),
            retry_after: Some(Duration::from_secs(60)),
        })]));
        let provider = registry.wrap("account", failed);
        let _: Vec<_> = provider
            .stream(request(), CancellationToken::new())
            .await
            .unwrap()
            .collect()
            .await;
        drop(provider);
        let success = Arc::new(FakeProvider::text(&["ok"]));
        let rebuilt = registry.wrap("account", success.clone());
        let child = registry.wrap("account", success.clone());
        let independent = registry.wrap("other-account", success.clone());
        let mut a = rebuilt
            .stream(request(), CancellationToken::new())
            .await
            .unwrap();
        let mut b = child
            .stream(request(), CancellationToken::new())
            .await
            .unwrap();
        assert!(matches!(
            a.next().await,
            Some(Ok(ProviderEvent::RetryWaiting { .. }))
        ));
        assert!(matches!(
            b.next().await,
            Some(Ok(ProviderEvent::RetryWaiting { .. }))
        ));
        let _: Vec<_> = independent
            .stream(request(), CancellationToken::new())
            .await
            .unwrap()
            .collect()
            .await;
        assert_eq!(success.requests().len(), 1);
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(matches!(
            a.next().await,
            Some(Ok(ProviderEvent::RequestReady))
        ));
        assert!(matches!(
            b.next().await,
            Some(Ok(ProviderEvent::RetryWaiting { .. }))
        ));
        let _: Vec<_> = a.collect().await;
        assert_eq!(success.requests().len(), 2);
        tokio::time::advance(Duration::from_secs(4)).await;
        let _: Vec<_> = b.collect().await;
        assert_eq!(success.requests().len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_wait_does_not_call_provider_or_reserve_a_future_slot() {
        let provider = Arc::new(FakeProvider::text(&["ok"]));
        let admission = Arc::new(Mutex::new(Admission::default()));
        admission.lock().unwrap().limited(Duration::from_secs(60));
        let gated = RateLimitedProvider {
            inner: provider.clone(),
            admission,
        };
        let token = CancellationToken::new();
        let mut stream = gated.stream(request(), token.clone()).await.unwrap();
        assert!(matches!(
            stream.next().await,
            Some(Ok(ProviderEvent::RetryWaiting { .. }))
        ));
        token.cancel();
        assert!(matches!(
            stream.next().await,
            Some(Err(ProviderError::Cancelled))
        ));
        assert!(provider.requests().is_empty());
        tokio::time::advance(Duration::from_secs(60)).await;
        let _: Vec<_> = gated
            .stream(request(), CancellationToken::new())
            .await
            .unwrap()
            .collect()
            .await;
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_later_limit_extends_existing_wait_and_increases_spacing() {
        let mut admission = Admission::default();
        admission.limited(Duration::from_secs(60));
        tokio::time::advance(Duration::from_secs(20)).await;
        admission.limited(Duration::from_secs(60));
        tokio::time::advance(Duration::from_secs(40)).await;
        assert_eq!(admission.admit(), Some(Duration::from_secs(20)));
        tokio::time::advance(Duration::from_secs(20)).await;
        assert_eq!(admission.admit(), None);
        assert_eq!(admission.admit(), Some(Duration::from_secs(8)));
    }
}

pub(crate) fn retry_at_ms(delay: Duration) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .saturating_add(delay)
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[async_trait]
impl Provider for RateLimitedProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        let inner = self.inner.clone();
        let admission = self.admission.clone();
        Ok(Box::pin(async_stream::stream! {
            loop {
                if cancellation.is_cancelled() { yield Err(ProviderError::Cancelled); return; }
                let delay = admission.lock().unwrap_or_else(|e| e.into_inner()).admit();
                let Some(delay) = delay else { break; };
                let deadline = Instant::now() + delay;
                yield Ok(ProviderEvent::RetryWaiting { retry_at_ms: retry_at_ms(delay) });
                tokio::select! {
                    _ = cancellation.cancelled() => { yield Err(ProviderError::Cancelled); return; }
                    _ = tokio::time::sleep_until(deadline) => {}
                }
                // Recheck after every wake: another caller may have extended cooldown
                // or consumed the next slot. Cancellation never reserves a future slot.
            }
            yield Ok(ProviderEvent::RequestReady);
            let result = tokio::select! {
                _ = cancellation.cancelled() => Err(ProviderError::Cancelled),
                result = inner.stream(request, cancellation.clone()) => result,
            };
            match result {
                Err(error) => {
                    if let Some(delay) = error.rate_limit_delay() {
                        admission.lock().unwrap_or_else(|e| e.into_inner()).limited(delay);
                    }
                    yield Err(error);
                }
                Ok(mut stream) => {
                    while let Some(event) = tokio::select! {
                        _ = cancellation.cancelled() => { yield Err(ProviderError::Cancelled); return; }
                        event = stream.next() => event,
                    } {
                        if let Err(error) = &event {
                            if let Some(delay) = error.rate_limit_delay() {
                                admission.lock().unwrap_or_else(|e| e.into_inner()).limited(delay);
                            }
                        }
                        yield event;
                    }
                }
            }
        }))
    }
}
