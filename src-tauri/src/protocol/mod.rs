use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub ready: bool,
    pub phase: String,
    pub version: String,
    pub uptime_seconds: u64,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub schema_version: u32,
    pub id: String,
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
    pub created_at_ms: u64,
}

impl ChatMessage {
    pub fn text(&self) -> String {
        self.content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => text.as_str(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Queued,
    Streaming,
    AwaitingApproval,
    RunningTool,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventEnvelope {
    pub schema_version: u32,
    #[serde(flatten)]
    pub event: AgentEvent,
}

impl AgentEventEnvelope {
    pub fn new(event: AgentEvent) -> Self {
        Self {
            schema_version: PROTOCOL_VERSION,
            event,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    TurnStarted {
        thread_id: String,
        turn_id: String,
    },
    TextDelta {
        thread_id: String,
        turn_id: String,
        delta: String,
    },
    UsageUpdated {
        thread_id: String,
        turn_id: String,
        usage: TokenUsage,
    },
    TurnCompleted {
        thread_id: String,
        turn_id: String,
        message: ChatMessage,
        usage: Option<TokenUsage>,
    },
    TurnFailed {
        thread_id: String,
        turn_id: String,
        message: String,
    },
    TurnCancelled {
        thread_id: String,
        turn_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_status_uses_the_frontend_protocol_shape() {
        let status = RuntimeStatus {
            ready: true,
            phase: "foundation".to_string(),
            version: "0.1.0".to_string(),
            uptime_seconds: 3,
            capabilities: vec!["typed-ipc".to_string()],
        };

        let value = serde_json::to_value(status).expect("runtime status should serialize");

        assert_eq!(value["ready"], true);
        assert_eq!(value["uptimeSeconds"], 3);
        assert_eq!(value["capabilities"][0], "typed-ipc");
        assert!(value.get("uptime_seconds").is_none());
    }

    #[test]
    fn agent_event_envelope_is_versioned_and_flattened() {
        let event = AgentEventEnvelope::new(AgentEvent::TurnCancelled {
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
        });

        let value = serde_json::to_value(event).expect("agent event should serialize");

        assert_eq!(value["schemaVersion"], PROTOCOL_VERSION);
        assert_eq!(value["type"], "turn_cancelled");
        assert_eq!(value["threadId"], "thread-1");
    }
}
