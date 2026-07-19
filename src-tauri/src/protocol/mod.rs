use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileOperation {
    Add,
    Modify,
    Delete,
    Move,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PatchFilePreview {
    pub path: String,
    pub destination_path: Option<String>,
    pub operation: FileOperation,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub before_content: Option<String>,
    pub after_content: Option<String>,
    pub unified_diff: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PatchPreview {
    pub patch: String,
    pub files: Vec<PatchFilePreview>,
    pub total_snapshot_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedFileHash {
    pub path: String,
    pub before_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    Read,
    Write,
    Delete,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub reason: String,
    pub risk: ToolRisk,
    pub arguments: serde_json::Value,
    pub preview: Option<PatchPreview>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAction {
    Approved,
    Rejected,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalResolution {
    pub action: ApprovalAction,
    pub patch: Option<String>,
    pub selected_paths: Vec<String>,
    pub expected_hashes: Vec<ExpectedFileHash>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSnapshot {
    pub request: ApprovalRequest,
    pub resolution: Option<ApprovalResolution>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeFileSnapshot {
    pub path: String,
    pub destination_path: Option<String>,
    pub operation: FileOperation,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub before_content: Option<String>,
    pub after_content: Option<String>,
    pub unified_diff: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSet {
    pub id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub tool_call_id: String,
    pub created_at_ms: u64,
    pub files: Vec<ChangeFileSnapshot>,
    pub undone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub metadata: serde_json::Value,
}

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
    ToolStarted {
        thread_id: String,
        turn_id: String,
        call: ToolCall,
    },
    ToolCompleted {
        thread_id: String,
        turn_id: String,
        call_id: String,
        name: String,
        result: ToolResult,
    },
    ApprovalRequested {
        thread_id: String,
        turn_id: String,
        request: ApprovalRequest,
    },
    ApprovalResolved {
        thread_id: String,
        turn_id: String,
        request_id: String,
        resolution: ApprovalResolution,
    },
    ChangeApplied {
        thread_id: String,
        turn_id: String,
        change_set: ChangeSet,
    },
    ChangeUndone {
        thread_id: String,
        turn_id: String,
        change_id: String,
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
