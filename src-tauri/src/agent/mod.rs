use std::sync::Arc;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::protocol::{
    AgentEvent, AgentEventEnvelope, ChatMessage, ContentBlock, MessageRole, PROTOCOL_VERSION,
    TokenUsage, TurnState,
};
use crate::providers::{Provider, ProviderError, ProviderEvent, ProviderRequest};
use crate::storage::{StorageError, StoredEvent, StoredEventKind, ThreadRepository, now_ms};

const MAX_INPUT_BYTES: usize = 100_000;
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunTurnRequest {
    pub thread_id: String,
    pub input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnOutcome {
    pub schema_version: u32,
    pub thread_id: String,
    pub turn_id: String,
    pub state: TurnState,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentRuntimeError {
    #[error("turn input is invalid: {0}")]
    InvalidInput(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

pub trait EventPublisher: Send + Sync {
    fn publish(&self, event: AgentEventEnvelope);
}

pub struct AgentRuntime {
    repository: Arc<dyn ThreadRepository>,
}

impl AgentRuntime {
    pub fn new(repository: Arc<dyn ThreadRepository>) -> Self {
        Self { repository }
    }

    pub async fn run_turn(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        request: RunTurnRequest,
        cancellation: CancellationToken,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        let input = validate_input(&request.input)?;
        self.run_turn_inner(
            provider,
            model,
            request.thread_id,
            Some(input),
            cancellation,
            publisher,
        )
        .await
    }

    pub async fn retry_turn(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        thread_id: String,
        cancellation: CancellationToken,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        let events = self.repository.load(&thread_id).await?;
        let retryable = events.iter().rev().find_map(|event| match event.kind {
            StoredEventKind::TurnFailed { .. } | StoredEventKind::TurnCancelled => Some(true),
            StoredEventKind::TurnCompleted { .. } => Some(false),
            _ => None,
        });
        if retryable != Some(true) {
            return Err(AgentRuntimeError::InvalidInput(
                "the latest turn is not retryable".to_string(),
            ));
        }
        if !events
            .iter()
            .any(|event| matches!(event.kind, StoredEventKind::UserMessage { .. }))
        {
            return Err(AgentRuntimeError::InvalidInput(
                "the thread has no user message to retry".to_string(),
            ));
        }

        self.run_turn_inner(provider, model, thread_id, None, cancellation, publisher)
            .await
    }

    async fn run_turn_inner(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        thread_id: String,
        new_input: Option<String>,
        cancellation: CancellationToken,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        let existing = self.repository.load(&thread_id).await?;
        if existing
            .iter()
            .any(|event| matches!(event.kind, StoredEventKind::ThreadArchived))
        {
            return Err(AgentRuntimeError::InvalidInput(
                "archived threads cannot accept new turns".to_string(),
            ));
        }

        if let Some(input) = new_input {
            let user_message = ChatMessage {
                schema_version: PROTOCOL_VERSION,
                id: Uuid::new_v4().to_string(),
                role: MessageRole::User,
                content: vec![ContentBlock::Text { text: input }],
                created_at_ms: now_ms(),
            };
            self.repository
                .append(StoredEvent::new(
                    &thread_id,
                    None,
                    StoredEventKind::UserMessage {
                        message: user_message,
                    },
                ))
                .await?;
        }

        let turn_id = Uuid::new_v4().to_string();
        self.repository
            .append(StoredEvent::new(
                &thread_id,
                Some(turn_id.clone()),
                StoredEventKind::TurnStarted,
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnStarted {
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
        }));

        if cancellation.is_cancelled() {
            return self
                .finish_cancelled(&thread_id, &turn_id, &publisher)
                .await;
        }

        let history = self
            .repository
            .load(&thread_id)
            .await?
            .into_iter()
            .filter_map(|event| match event.kind {
                StoredEventKind::UserMessage { message }
                | StoredEventKind::AssistantMessage { message } => Some(message),
                _ => None,
            })
            .collect();
        let request = ProviderRequest {
            schema_version: PROTOCOL_VERSION,
            model,
            messages: history,
        };
        let mut stream = match provider.stream(request, cancellation.clone()).await {
            Ok(stream) => stream,
            Err(ProviderError::Cancelled) => {
                return self
                    .finish_cancelled(&thread_id, &turn_id, &publisher)
                    .await;
            }
            Err(error) => {
                return self
                    .finish_failed(&thread_id, &turn_id, error.to_string(), &publisher)
                    .await;
            }
        };

        let mut response = String::new();
        let mut usage = None;
        loop {
            let event = tokio::select! {
                _ = cancellation.cancelled() => {
                    return self.finish_cancelled(&thread_id, &turn_id, &publisher).await;
                }
                event = stream.next() => event,
            };

            match event {
                Some(Ok(ProviderEvent::TextDelta { delta })) => {
                    if response.len().saturating_add(delta.len()) > MAX_RESPONSE_BYTES {
                        return self
                            .finish_failed(
                                &thread_id,
                                &turn_id,
                                format!(
                                    "provider response exceeds the {MAX_RESPONSE_BYTES} byte limit"
                                ),
                                &publisher,
                            )
                            .await;
                    }
                    response.push_str(&delta);
                    publisher.publish(AgentEventEnvelope::new(AgentEvent::TextDelta {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        delta,
                    }));
                }
                Some(Ok(ProviderEvent::Usage { usage: current })) => {
                    usage = Some(current);
                    publisher.publish(AgentEventEnvelope::new(AgentEvent::UsageUpdated {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        usage: current,
                    }));
                }
                Some(Ok(ProviderEvent::Completed)) => {
                    if response.is_empty() {
                        return self
                            .finish_failed(
                                &thread_id,
                                &turn_id,
                                "provider completed without a text response".to_string(),
                                &publisher,
                            )
                            .await;
                    }
                    return self
                        .finish_completed(&thread_id, &turn_id, response, usage, &publisher)
                        .await;
                }
                Some(Err(ProviderError::Cancelled)) => {
                    return self
                        .finish_cancelled(&thread_id, &turn_id, &publisher)
                        .await;
                }
                Some(Err(error)) => {
                    return self
                        .finish_failed(&thread_id, &turn_id, error.to_string(), &publisher)
                        .await;
                }
                None => {
                    return self
                        .finish_failed(
                            &thread_id,
                            &turn_id,
                            ProviderError::Interrupted.to_string(),
                            &publisher,
                        )
                        .await;
                }
            }
        }
    }

    async fn finish_completed(
        &self,
        thread_id: &str,
        turn_id: &str,
        text: String,
        usage: Option<TokenUsage>,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        let message = ChatMessage {
            schema_version: PROTOCOL_VERSION,
            id: Uuid::new_v4().to_string(),
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Text { text }],
            created_at_ms: now_ms(),
        };
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::AssistantMessage {
                    message: message.clone(),
                },
            ))
            .await?;
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::TurnCompleted { usage },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnCompleted {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            message,
            usage,
        }));
        Ok(outcome(thread_id, turn_id, TurnState::Completed, None))
    }

    async fn finish_failed(
        &self,
        thread_id: &str,
        turn_id: &str,
        message: String,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::TurnFailed {
                    message: message.clone(),
                },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnFailed {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            message: message.clone(),
        }));
        Ok(outcome(
            thread_id,
            turn_id,
            TurnState::Failed,
            Some(message),
        ))
    }

    async fn finish_cancelled(
        &self,
        thread_id: &str,
        turn_id: &str,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::TurnCancelled,
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnCancelled {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
        }));
        Ok(outcome(thread_id, turn_id, TurnState::Cancelled, None))
    }
}

fn validate_input(input: &str) -> Result<String, AgentRuntimeError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AgentRuntimeError::InvalidInput(
            "input must not be empty".to_string(),
        ));
    }
    if input.len() > MAX_INPUT_BYTES {
        return Err(AgentRuntimeError::InvalidInput(format!(
            "input exceeds the {MAX_INPUT_BYTES} byte limit"
        )));
    }
    Ok(input.to_string())
}

fn outcome(thread_id: &str, turn_id: &str, state: TurnState, error: Option<String>) -> TurnOutcome {
    TurnOutcome {
        schema_version: PROTOCOL_VERSION,
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
        state,
        error,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use crate::providers::testing::FakeProvider;
    use crate::storage::JsonlThreadRepository;

    #[derive(Default)]
    struct RecordingPublisher {
        events: Mutex<Vec<AgentEventEnvelope>>,
    }

    impl EventPublisher for RecordingPublisher {
        fn publish(&self, event: AgentEventEnvelope) {
            self.events.lock().unwrap().push(event);
        }
    }

    async fn runtime_fixture() -> (
        tempfile::TempDir,
        Arc<JsonlThreadRepository>,
        AgentRuntime,
        String,
    ) {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let repository = Arc::new(
            JsonlThreadRepository::new(directory.path()).expect("repository should be created"),
        );
        let thread = repository
            .create_thread()
            .await
            .expect("thread should be created");
        let runtime = AgentRuntime::new(repository.clone());
        (directory, repository, runtime, thread.id)
    }

    #[tokio::test]
    async fn persists_and_publishes_a_streamed_text_turn() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::text(&["hello", " world"]));
        let publisher = Arc::new(RecordingPublisher::default());

        let result = runtime
            .run_turn(
                provider.clone(),
                "fake-model".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "say hello".to_string(),
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .expect("turn should run");

        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert_eq!(result.state, TurnState::Completed);
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[1].text(), "hello world");
        assert_eq!(provider.requests()[0].messages.len(), 1);
        let recorded_events = publisher.events.lock().unwrap();
        let event_types = recorded_events
            .iter()
            .map(|envelope| &envelope.event)
            .collect::<Vec<_>>();
        assert!(matches!(event_types[0], AgentEvent::TurnStarted { .. }));
        assert!(matches!(
            event_types.last(),
            Some(AgentEvent::TurnCompleted { .. })
        ));
    }

    #[tokio::test]
    async fn cancellation_is_persisted_and_published() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::text(&["late"]).with_delay(Duration::from_secs(10)));
        let publisher = Arc::new(RecordingPublisher::default());
        let cancellation = CancellationToken::new();
        let cancel_from_test = cancellation.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel_from_test.cancel();
        });
        let result = runtime
            .run_turn(
                provider,
                "fake-model".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "wait".to_string(),
                },
                cancellation,
                publisher,
            )
            .await
            .expect("cancelled turn should finish cleanly");

        assert_eq!(result.state, TurnState::Cancelled);
        let events = repository.load(&thread_id).await.unwrap();
        assert!(matches!(
            events.last().map(|event| &event.kind),
            Some(StoredEventKind::TurnCancelled)
        ));
    }

    #[tokio::test]
    async fn retry_reuses_the_existing_user_message() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let failure = Arc::new(FakeProvider::new(vec![Err(ProviderError::Request(
            "offline".to_string(),
        ))]));
        let publisher = Arc::new(RecordingPublisher::default());
        runtime
            .run_turn(
                failure,
                "fake-model".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "try once".to_string(),
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        let success = Arc::new(FakeProvider::text(&["done"]));
        runtime
            .retry_turn(
                success.clone(),
                "fake-model".to_string(),
                thread_id.clone(),
                CancellationToken::new(),
                publisher,
            )
            .await
            .expect("retry should complete");

        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(success.requests()[0].messages.len(), 1);
    }
}
