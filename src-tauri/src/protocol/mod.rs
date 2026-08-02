use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// 应用层的推理强度，Provider 适配器负责映射到兼容字段。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
}

impl ReasoningEffort {
    pub fn openai_value(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Minimal => Some("minimal"),
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::XHigh => Some("xhigh"),
        }
    }

    pub fn anthropic_budget_tokens(self) -> Option<u32> {
        match self {
            Self::Off => None,
            Self::Minimal => Some(1_024),
            Self::Low => Some(2_048),
            Self::Medium => Some(4_096),
            Self::High => Some(8_192),
            Self::XHigh => Some(16_384),
        }
    }

    pub fn gemini_budget_tokens(self) -> Option<i32> {
        self.anthropic_budget_tokens().map(|value| value as i32)
    }
}

/// 任务清单状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// 任务清单项
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    pub active_form: String,
}

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

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    #[default]
    Ask,
    FullAccess,
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
    #[serde(default)]
    pub auto_approved: bool,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    /// 系统指令消息，用于注入 runtime_instructions 等分层 prompt。
    /// 在 provider 层会被转换为各 API 的 system 消息格式。
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Image { name: String, data_url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachment {
    pub name: String,
    pub data_url: String,
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
                ContentBlock::Image { .. } => "",
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivityStatus {
    Thinking,
    Responding,
    RunningTool,
    AwaitingApproval,
    Finalizing,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputStream {
    Stdout,
    Stderr,
}

/// Turn 的语义阶段（借鉴 Codex 的 TurnPhase 概念）。
/// 与 `TurnState` 不同，`TurnPhase` 描述的是 turn 在逻辑上处于哪个阶段，
/// 而非当前在做什么操作。前端可据此显示"探索中…"/"规划中…"/"执行中…"等提示。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    /// 空闲，未开始
    #[default]
    Idle,
    /// 探索阶段：读取文件、搜索代码、理解上下文
    Exploring,
    /// 规划阶段：制定方案、向用户提问（Plan 模式）
    Planning,
    /// 执行阶段：修改文件、运行命令
    Executing,
    /// 等待用户输入（审批/提问）
    AwaitingInput,
    /// 已完成
    Complete,
    /// 已失败
    Failed,
    /// 已取消
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventEnvelope {
    pub schema_version: u32,
    /// 当前 turn 的语义阶段，前端据此渲染不同 UI。
    pub phase: TurnPhase,
    #[serde(flatten)]
    pub event: AgentEvent,
}

impl AgentEventEnvelope {
    pub fn new(event: AgentEvent) -> Self {
        let phase = event.default_phase();
        Self {
            schema_version: PROTOCOL_VERSION,
            phase,
            event,
        }
    }

    /// 用指定 phase 构造 envelope（用于覆盖默认推断）。
    pub fn with_phase(event: AgentEvent, phase: TurnPhase) -> Self {
        Self {
            schema_version: PROTOCOL_VERSION,
            phase,
            event,
        }
    }
}

impl AgentEvent {
    /// 根据事件类型推断默认的 turn phase。
    fn default_phase(&self) -> TurnPhase {
        match self {
            Self::TurnStarted { .. } => TurnPhase::Exploring,
            Self::ActivityStatusChanged { status, .. } => match status {
                AgentActivityStatus::Thinking => TurnPhase::Exploring,
                AgentActivityStatus::Responding | AgentActivityStatus::Finalizing => {
                    TurnPhase::Planning
                }
                AgentActivityStatus::RunningTool => TurnPhase::Executing,
                AgentActivityStatus::AwaitingApproval => TurnPhase::AwaitingInput,
            },
            Self::TextDelta { .. } => TurnPhase::Planning,
            Self::ReasoningSummaryDelta { .. } | Self::ReasoningSummaryCompleted { .. } => {
                TurnPhase::Planning
            }
            Self::ToolStarted { .. } => TurnPhase::Executing,
            Self::ToolOutputDelta { .. } => TurnPhase::Executing,
            Self::ToolCompleted { .. } => TurnPhase::Executing,
            Self::ApprovalRequested { .. } => TurnPhase::AwaitingInput,
            Self::ApprovalResolved { .. } => TurnPhase::Executing,
            Self::UserInputRequested { .. } => TurnPhase::AwaitingInput,
            Self::UserInputResolved { .. } => TurnPhase::Planning,
            Self::ChangeApplied { .. } => TurnPhase::Executing,
            Self::ChangeUndone { .. } => TurnPhase::Executing,
            Self::TurnCompleted { .. } => TurnPhase::Complete,
            Self::TurnFailed { .. } => TurnPhase::Failed,
            Self::TurnCancelled { .. } => TurnPhase::Cancelled,
            Self::UsageUpdated { .. } => TurnPhase::Executing,
            Self::TodoUpdated { .. } => TurnPhase::Planning,
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
    ActivityStatusChanged {
        thread_id: String,
        turn_id: String,
        status: AgentActivityStatus,
    },
    TextDelta {
        thread_id: String,
        turn_id: String,
        delta: String,
    },
    ReasoningSummaryDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    ReasoningSummaryCompleted {
        thread_id: String,
        turn_id: String,
        item_id: String,
        summary: String,
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
    ToolOutputDelta {
        thread_id: String,
        turn_id: String,
        call_id: String,
        stream: ToolOutputStream,
        cursor: u64,
        delta: String,
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
    /// Plan 模式：模型通过 `request_user_input` 工具向用户提问
    UserInputRequested {
        thread_id: String,
        turn_id: String,
        request: UserInputRequest,
    },
    /// Plan 模式：用户回答了提问
    UserInputResolved {
        thread_id: String,
        turn_id: String,
        request_id: String,
        resolution: UserInputResolution,
    },
    /// 任务清单更新
    TodoUpdated {
        thread_id: String,
        turn_id: String,
        todos: Vec<TodoItem>,
    },
}

/// 智能体协作模式
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// 默认模式：直接执行，可修改代码
    #[default]
    Craft,
    /// 只回答问题，不修改代码（只读工具子集）
    Ask,
    /// 规划模式：只探索和制定计划，不执行变更，可向用户提问
    Plan,
}

impl AgentMode {
    pub fn from_str(value: &str) -> Self {
        match value {
            "ask" => Self::Ask,
            "plan" => Self::Plan,
            _ => Self::Craft,
        }
    }

    /// 该模式下允许的工具名（其余工具会被 restricted_to 过滤掉）
    pub fn allowed_tools(&self) -> &[&str] {
        const READ_ONLY: &[&str] = &[
            "list_directory",
            "read_file",
            "search_repository",
            "recall_memory",
            "request_user_input",
            "update_plan",
        ];
        const PLAN_ONLY: &[&str] = &[
            "list_directory",
            "read_file",
            "search_repository",
            "recall_memory",
            "request_user_input",
        ];
        match self {
            Self::Craft => &[],
            Self::Ask => READ_ONLY,
            Self::Plan => PLAN_ONLY,
        }
    }

    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::Ask | Self::Plan)
    }
}

/// 模型向用户提出的单个问题
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserInputQuestion {
    pub question: String,
    pub options: Vec<String>,
}

/// `request_user_input` 工具发起的提问请求
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserInputRequest {
    pub id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub tool_call_id: String,
    pub questions: Vec<UserInputQuestion>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

/// 用户对单个问题的回答
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserInputAnswer {
    pub question: String,
    /// 用户选择的选项；如果用户自由输入则不在原 options 中
    pub answer: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserInputAction {
    Answered,
    Skipped,
    Cancelled,
}

/// 用户对 `request_user_input` 的整体回复
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserInputResolution {
    pub action: UserInputAction,
    pub answers: Vec<UserInputAnswer>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_mode_uses_the_public_snake_case_contract() {
        assert_eq!(
            serde_json::to_value(ApprovalMode::FullAccess).unwrap(),
            serde_json::json!("full_access")
        );
        assert_eq!(
            serde_json::from_value::<ApprovalMode>(serde_json::json!("ask")).unwrap(),
            ApprovalMode::Ask
        );
    }

    #[test]
    fn reasoning_effort_uses_stable_values_and_provider_mappings() {
        assert_eq!(
            serde_json::to_value(ReasoningEffort::XHigh).unwrap(),
            serde_json::json!("x_high")
        );
        assert_eq!(ReasoningEffort::Off.openai_value(), None);
        assert_eq!(ReasoningEffort::High.openai_value(), Some("high"));
        assert_eq!(
            ReasoningEffort::Minimal.anthropic_budget_tokens(),
            Some(1_024)
        );
        assert_eq!(ReasoningEffort::XHigh.gemini_budget_tokens(), Some(16_384));
    }

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

    #[test]
    fn live_activity_events_use_the_public_camel_case_contract() {
        let event = AgentEventEnvelope::new(AgentEvent::ToolOutputDelta {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            call_id: "call-1".into(),
            stream: ToolOutputStream::Stderr,
            cursor: 7,
            delta: "failed\n".into(),
        });
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "tool_output_delta");
        assert_eq!(value["phase"], "executing");
        assert_eq!(value["callId"], "call-1");
        assert_eq!(value["stream"], "stderr");
        assert_eq!(value["cursor"], 7);
    }
}
