use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::{Provider, ProviderError, ProviderEvent, ProviderRequest, ProviderStream};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct FakeProvider {
    events: Vec<Result<ProviderEvent, ProviderError>>,
    delay: Duration,
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

impl FakeProvider {
    pub fn new(events: Vec<Result<ProviderEvent, ProviderError>>) -> Self {
        Self {
            events,
            delay: Duration::ZERO,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn text(chunks: &[&str]) -> Self {
        let mut events = chunks
            .iter()
            .map(|chunk| {
                Ok(ProviderEvent::TextDelta {
                    delta: (*chunk).to_string(),
                })
            })
            .collect::<Vec<_>>();
        events.push(Ok(ProviderEvent::Completed));
        Self::new(events)
    }

    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn requests(&self) -> Vec<ProviderRequest> {
        self.requests
            .lock()
            .expect("fake provider request lock should not be poisoned")
            .clone()
    }
}

#[async_trait]
impl Provider for FakeProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        self.requests
            .lock()
            .expect("fake provider request lock should not be poisoned")
            .push(request);
        let events = self.events.clone();
        let delay = self.delay;

        Ok(Box::pin(async_stream::stream! {
            for event in events {
                if delay.is_zero() {
                    if cancellation.is_cancelled() {
                        yield Err(ProviderError::Cancelled);
                        return;
                    }
                } else {
                    tokio::select! {
                        _ = cancellation.cancelled() => {
                            yield Err(ProviderError::Cancelled);
                            return;
                        }
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
                yield event;
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;
    use crate::protocol::PROTOCOL_VERSION;

    fn request() -> ProviderRequest {
        ProviderRequest {
            schema_version: PROTOCOL_VERSION,
            model: "test".to_string(),
            messages: Vec::new(),
        }
    }

    #[tokio::test]
    async fn emits_a_deterministic_stream_without_network_access() {
        let provider = FakeProvider::text(&["hello", " world"]);
        let stream = provider
            .stream(request(), CancellationToken::new())
            .await
            .expect("stream should start");
        let events = stream.collect::<Vec<_>>().await;

        assert_eq!(events.len(), 3);
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn observes_cancellation_while_waiting_for_a_chunk() {
        let provider = FakeProvider::text(&["never"]).with_delay(Duration::from_secs(10));
        let cancellation = CancellationToken::new();
        let mut stream = provider
            .stream(request(), cancellation.clone())
            .await
            .expect("stream should start");

        cancellation.cancel();

        assert_eq!(stream.next().await, Some(Err(ProviderError::Cancelled)));
    }
}
