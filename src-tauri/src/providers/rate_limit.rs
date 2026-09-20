//! Account-scoped admission control, shared across rebuilt providers and child runtimes.
//! No credentials, prompts or model history are stored here.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{Provider, ProviderError, ProviderEvent, ProviderRequest, ProviderStream};

#[derive(Default)]
pub struct RateLimitRegistry(Mutex<HashMap<String, Arc<SharedAdmission>>>);

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
struct SharedAdmission {
    state: Mutex<Admission>,
    changed: Notify,
}

const MIN_SPACING: Duration = Duration::from_secs(4);
const MAX_SPACING: Duration = Duration::from_secs(60);
const IDLE_RESET: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
struct Admission {
    cooldown_until: Option<Instant>,
    last_request: Option<Instant>,
    spacing: Duration,
    limit_generation: u64,
}

impl Admission {
    fn admit(&mut self) -> Result<u64, Duration> {
        let now = Instant::now();
        // Forget adaptive penalties after an idle period, measured from the last
        // admission or the end of server cooldown, whichever is later.
        if self
            .last_request
            .into_iter()
            .chain(self.cooldown_until)
            .max()
            .is_some_and(|last| now.saturating_duration_since(last) >= IDLE_RESET)
        {
            self.spacing = Duration::ZERO;
        }
        let deadline = self
            .cooldown_until
            .into_iter()
            .chain(self.last_request.map(|last| last + self.spacing))
            .max();
        if let Some(deadline) = deadline.filter(|deadline| *deadline > now) {
            return Err(deadline - now);
        }
        self.last_request = Some(now);
        Ok(self.limit_generation)
    }

    fn limited(&mut self, delay: Duration) {
        let deadline = Instant::now() + delay;
        self.cooldown_until = Some(
            self.cooldown_until
                .map_or(deadline, |old| old.max(deadline)),
        );
        self.limit_generation = self.limit_generation.saturating_add(1);
        // Start conservatively after the first 429; repeated limits slow subsequent
        // requests further. This is adaptive spacing, not an assumed provider quota.
        self.spacing = (self.spacing * 2).clamp(MIN_SPACING, MAX_SPACING);
    }

    fn succeeded(&mut self, generation: u64) -> bool {
        // An older in-flight success cannot undo a newer request's 429. Only
        // recovery requests admitted after the latest limit can reduce spacing.
        if generation != self.limit_generation || self.spacing.is_zero() {
            return false;
        }
        self.spacing /= 2;
        if self.spacing < MIN_SPACING {
            self.spacing = Duration::ZERO;
        }
        true
    }
}

pub struct RateLimitedProvider {
    inner: Arc<dyn Provider>,
    admission: Arc<SharedAdmission>,
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
        let recovered_at = Instant::now();
        let _: Vec<_> = b.collect().await;
        assert_eq!(success.requests().len(), 3);
        assert_eq!(
            Instant::now(),
            recovered_at,
            "success must wake the waiting child"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn successful_requests_without_a_limit_are_never_delayed() {
        let registry = RateLimitRegistry::default();
        let inner = Arc::new(FakeProvider::text(&["ok"]));
        let provider = registry.wrap("account", inner.clone());
        let started = Instant::now();
        for _ in 0..3 {
            let events: Vec<_> = provider
                .stream(request(), CancellationToken::new())
                .await
                .unwrap()
                .collect()
                .await;
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ok(ProviderEvent::RetryWaiting { .. })))
            );
        }
        assert_eq!(inner.requests().len(), 3);
        assert_eq!(Instant::now(), started);
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_limits_recover_gradually_instead_of_staying_at_one_request_per_minute() {
        let inner = Arc::new(FakeProvider::text(&["ok"]));
        let admission = Arc::new(SharedAdmission::default());
        for _ in 0..5 {
            admission
                .state
                .lock()
                .unwrap()
                .limited(Duration::from_secs(60));
        }
        let provider = RateLimitedProvider {
            inner: inner.clone(),
            admission,
        };
        let started = Instant::now();
        // Cooldown, then successful completions halve spacing until it is removed.
        for expected_wait in [60_000, 30_000, 15_000, 7_500, 0] {
            let before = Instant::now();
            let _: Vec<_> = provider
                .stream(request(), CancellationToken::new())
                .await
                .unwrap()
                .collect()
                .await;
            assert_eq!(
                Instant::now() - before,
                Duration::from_millis(expected_wait)
            );
        }
        assert_eq!(inner.requests().len(), 5);
        assert_eq!(Instant::now() - started, Duration::from_millis(112_500));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_accounts_forget_spacing_without_shortening_server_cooldown() {
        let mut admission = Admission::default();
        admission.limited(Duration::from_secs(600));
        tokio::time::advance(IDLE_RESET).await;
        assert_eq!(admission.admit(), Err(Duration::from_secs(300)));
        tokio::time::advance(Duration::from_secs(300) + IDLE_RESET).await;
        assert!(admission.admit().is_ok());
        assert!(
            admission.admit().is_ok(),
            "an old limit must not pace a fresh conversation"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_older_in_flight_success_cannot_release_a_newer_server_cooldown() {
        let registry = RateLimitRegistry::default();
        let success = Arc::new(FakeProvider::text(&["ok"]));
        let successful = registry.wrap("account", success.clone());
        let mut older = successful
            .stream(request(), CancellationToken::new())
            .await
            .unwrap();
        assert!(matches!(
            older.next().await,
            Some(Ok(ProviderEvent::RequestReady))
        ));
        assert!(matches!(
            older.next().await,
            Some(Ok(ProviderEvent::TextDelta { .. }))
        ));

        let limited = registry.wrap(
            "account",
            Arc::new(FakeProvider::new(vec![Err(ProviderError::RateLimited {
                message: "busy".into(),
                retry_after: Some(Duration::from_secs(120)),
            })])),
        );
        let _: Vec<_> = limited
            .stream(request(), CancellationToken::new())
            .await
            .unwrap()
            .collect()
            .await;
        let _: Vec<_> = older.collect().await;
        let before = Instant::now();
        let mut fresh = successful
            .stream(request(), CancellationToken::new())
            .await
            .unwrap();
        assert!(matches!(
            fresh.next().await,
            Some(Ok(ProviderEvent::RetryWaiting { .. }))
        ));
        assert_eq!(success.requests().len(), 1);
        let _: Vec<_> = fresh.collect().await;
        assert_eq!(Instant::now() - before, Duration::from_secs(120));
        assert_eq!(success.requests().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn incomplete_failed_and_cancelled_streams_do_not_relax_spacing() {
        for events in [
            vec![Ok(ProviderEvent::TextDelta {
                delta: "partial".into(),
            })],
            vec![
                Err(ProviderError::Unavailable("busy".into())),
                Ok(ProviderEvent::Completed),
            ],
            vec![Err(ProviderError::Cancelled)],
        ] {
            let admission = Arc::new(SharedAdmission::default());
            admission
                .state
                .lock()
                .unwrap()
                .limited(Duration::from_secs(1));
            let provider = RateLimitedProvider {
                inner: Arc::new(FakeProvider::new(events)),
                admission: admission.clone(),
            };
            let _: Vec<_> = provider
                .stream(request(), CancellationToken::new())
                .await
                .unwrap()
                .collect()
                .await;
            assert_eq!(
                admission.state.lock().unwrap().admit(),
                Err(Duration::from_secs(4))
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_wait_does_not_call_provider_or_reserve_a_future_slot() {
        let provider = Arc::new(FakeProvider::text(&["ok"]));
        let admission = Arc::new(SharedAdmission::default());
        admission
            .state
            .lock()
            .unwrap()
            .limited(Duration::from_secs(60));
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
        assert_eq!(admission.admit(), Err(Duration::from_secs(20)));
        tokio::time::advance(Duration::from_secs(20)).await;
        assert!(admission.admit().is_ok());
        assert_eq!(admission.admit(), Err(Duration::from_secs(8)));
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
            let generation = loop {
                if cancellation.is_cancelled() { yield Err(ProviderError::Cancelled); return; }
                let changed = admission.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let decision = admission.state.lock().unwrap_or_else(|e| e.into_inner()).admit();
                let delay = match decision {
                    Ok(generation) => break generation,
                    Err(delay) => delay,
                };
                let deadline = Instant::now() + delay;
                yield Ok(ProviderEvent::RetryWaiting { retry_at_ms: retry_at_ms(delay) });
                tokio::select! {
                    _ = cancellation.cancelled() => { yield Err(ProviderError::Cancelled); return; }
                    _ = tokio::time::sleep_until(deadline) => {}
                    _ = &mut changed => {}
                }
                // Recheck after every wake: another caller may have extended cooldown
                // or recovered. Cancellation never reserves a future slot.
            };
            yield Ok(ProviderEvent::RequestReady);
            let result = tokio::select! {
                _ = cancellation.cancelled() => Err(ProviderError::Cancelled),
                result = inner.stream(request, cancellation.clone()) => result,
            };
            match result {
                Err(error) => {
                    if let Some(delay) = error.rate_limit_delay() {
                        admission.state.lock().unwrap_or_else(|e| e.into_inner()).limited(delay);
                    }
                    yield Err(error);
                }
                Ok(mut stream) => {
                    let mut healthy = true;
                    while let Some(event) = tokio::select! {
                        _ = cancellation.cancelled() => { yield Err(ProviderError::Cancelled); return; }
                        event = stream.next() => event,
                    } {
                        if let Err(error) = &event {
                            healthy = false;
                            if let Some(delay) = error.rate_limit_delay() {
                                admission.state.lock().unwrap_or_else(|e| e.into_inner()).limited(delay);
                            }
                        } else if healthy && matches!(&event, Ok(ProviderEvent::Completed)) {
                            healthy = false;
                            let recovered = admission.state.lock().unwrap_or_else(|e| e.into_inner())
                                .succeeded(generation);
                            if recovered { admission.changed.notify_waiters(); }
                        }
                        yield event;
                    }
                }
            }
        }))
    }
}
