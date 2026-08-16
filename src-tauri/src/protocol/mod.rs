use serde::{Deserialize, Serialize};

use crate::storage::{
    ThreadSummary, ToolActivitySnapshot, TurnSnapshot, TurnTimelineItem, UserInputSnapshot,
};

pub const PROTOCOL_VERSION: u32 = 1;
pub const AGENT_EVENT_SCHEMA_VERSION: u32 = 5;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistorySortDirection {
    Asc,
    #[default]
    Desc,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnItemsView {
    NotLoaded,
    Summary,
    #[default]
    Full,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMessagePhase {
    Commentary,
    FinalAnswer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ThreadItemPayload {
    UserMessage {
        message: ChatMessage,
    },
    AgentMessage {
        message: ChatMessage,
        phase: AgentMessagePhase,
    },
    Reasoning {
        summary: String,
    },
    Tool {
        activity: ToolActivitySnapshot,
    },
    Approval {
        approval: ApprovalSnapshot,
    },
    UserInput {
        user_input: UserInputSnapshot,
    },
    Change {
        change_set: ChangeSet,
    },
    ContextCompaction {
        automatic: bool,
        compacted_message_count: usize,
        user_constraint_count: usize,
        recent_tool_result_count: usize,
        #[serde(default)]
        recent_user_message_count: usize,
    },
    Event,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItem {
    pub schema_version: u32,
    pub id: String,
    pub turn_id: Option<String>,
    pub status: Option<AgentItemStatus>,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
    pub timeline_items: Vec<TurnTimelineItem>,
    #[serde(flatten)]
    pub payload: ThreadItemPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTurn {
    pub schema_version: u32,
    pub id: String,
    pub user_message_id: Option<String>,
    pub state: TurnState,
    pub error: Option<TurnError>,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub items_view: TurnItemsView,
    pub items: Vec<ThreadItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnHandle {
    pub schema_version: u32,
    pub thread_id: String,
    pub turn_id: String,
    pub state: TurnState,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueuedTurnKind {
    #[default]
    Message,
    Retry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueuedTurn {
    pub schema_version: u32,
    pub turn_id: String,
    pub thread_id: String,
    #[serde(default)]
    pub kind: QueuedTurnKind,
    pub input: String,
    pub agent_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    pub attachments: Vec<ImageAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMailboxSnapshot {
    pub schema_version: u32,
    pub thread_id: String,
    pub revision: u64,
    pub active_turn_id: Option<String>,
    pub pending: Vec<QueuedTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMailboxChanged {
    pub schema_version: u32,
    pub thread_id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerRequest {
    pub thread_id: String,
    pub expected_turn_id: String,
    pub input: String,
    #[serde(default)]
    pub attachments: Vec<ImageAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueuedTurnSteerRequest {
    pub thread_id: String,
    pub expected_turn_id: String,
    pub queued_turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerResponse {
    pub schema_version: u32,
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkRequest {
    pub thread_id: String,
    pub last_turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRollbackRequest {
    pub thread_id: String,
    pub num_turns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTurnsPage {
    pub data: Vec<ThreadTurn>,
    pub next_cursor: Option<String>,
    pub backwards_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItemEntry {
    pub turn_id: Option<String>,
    pub item: ThreadItem,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItemsPage {
    pub data: Vec<ThreadItemEntry>,
    pub next_cursor: Option<String>,
    pub backwards_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadHistorySnapshot {
    pub schema_version: u32,
    pub summary: ThreadSummary,
    pub last_turn: Option<TurnSnapshot>,
    pub todos: Vec<TodoItem>,
    pub last_usage: Option<TokenUsage>,
    #[serde(default)]
    pub context_usage: Option<TokenUsage>,
    pub turns: ThreadTurnsPage,
    pub unscoped_items: Vec<ThreadItem>,
}

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    Disabled,
    Loaded,
    Degraded,
    Blocked,
    Invalid,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginComponentSummary {
    pub skill_count: usize,
    pub mcp_server_count: usize,
    pub mcp_tool_count: usize,
    pub unsupported_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginDiagnostic {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub path: String,
    pub enabled: bool,
    pub state: PluginState,
    pub deletable: bool,
    pub components: PluginComponentSummary,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginOverview {
    pub schema_version: u32,
    pub root_path: String,
    pub plugins: Vec<PluginDiagnostic>,
    pub error: Option<String>,
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
    Text {
        text: String,
    },
    Context {
        text: String,
    },
    Image {
        name: String,
        #[serde(rename = "dataUrl", alias = "data_url")]
        data_url: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachment {
    pub name: String,
    pub data_url: String,
    #[serde(default)]
    pub ocr_text: Option<String>,
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
                ContentBlock::Context { text } => text.as_str(),
                ContentBlock::Image { .. } => "",
            })
            .collect()
    }

    pub fn visible_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                ContentBlock::Context { .. } | ContentBlock::Image { .. } => None,
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
pub enum TurnErrorCategory {
    Provider,
    Authentication,
    Policy,
    Tool,
    Storage,
    Protocol,
    Runtime,
    Legacy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub category: TurnErrorCategory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl TurnError {
    pub fn legacy(message: String) -> Self {
        Self {
            code: "legacy_failure".to_string(),
            message,
            retryable: true,
            category: TurnErrorCategory::Legacy,
            details: None,
        }
    }

    pub fn classify(message: String) -> Self {
        let normalized = message.to_ascii_lowercase();
        let (code, retryable, category) = if normalized.contains("token_budget_exceeded")
            || normalized.contains("response_limit")
        {
            ("limit_exceeded", false, TurnErrorCategory::Runtime)
        } else if normalized.contains("api key") || normalized.contains("authentication") {
            (
                "authentication_failed",
                false,
                TurnErrorCategory::Authentication,
            )
        } else if normalized.contains("rate limit") || normalized.contains("status 429") {
            ("rate_limited", true, TurnErrorCategory::Provider)
        } else if normalized.contains("approval") || normalized.contains("permission") {
            ("policy_denied", false, TurnErrorCategory::Policy)
        } else if normalized.contains("storage") || normalized.contains("jsonl") {
            ("storage_failure", true, TurnErrorCategory::Storage)
        } else if normalized.contains("invalid input") || normalized.contains("invalid json") {
            ("invalid_input", false, TurnErrorCategory::Protocol)
        } else if normalized.contains("provider") {
            ("provider_failure", true, TurnErrorCategory::Provider)
        } else {
            ("runtime_failure", true, TurnErrorCategory::Runtime)
        };
        Self {
            code: code.to_string(),
            message,
            retryable,
            category,
            details: None,
        }
    }
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

/// Codex 式 Thread Item 的公开类别。事件先统一生命周期，再逐步扩展每类 Item 的增量载荷。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentItemType {
    AgentMessage,
    Reasoning,
    Tool,
    Approval,
    Change,
    ContextCompaction,
    UserInput,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentItemStatus {
    Completed,
    Failed,
    Cancelled,
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
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            phase,
            event,
        }
    }

    /// 用指定 phase 构造 envelope（用于覆盖默认推断）。
    pub fn with_phase(event: AgentEvent, phase: TurnPhase) -> Self {
        Self {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
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
            Self::TurnSteered { .. } => TurnPhase::Exploring,
            Self::TurnRejected { .. } => TurnPhase::Failed,
            Self::ActivityStatusChanged { status, .. } => match status {
                AgentActivityStatus::Thinking => TurnPhase::Exploring,
                AgentActivityStatus::Responding | AgentActivityStatus::Finalizing => {
                    TurnPhase::Planning
                }
                AgentActivityStatus::RunningTool => TurnPhase::Executing,
                AgentActivityStatus::AwaitingApproval => TurnPhase::AwaitingInput,
            },
            Self::ItemStarted { item_type, .. } | Self::ItemCompleted { item_type, .. } => {
                match item_type {
                    AgentItemType::Tool | AgentItemType::Approval | AgentItemType::Change => {
                        TurnPhase::Executing
                    }
                    AgentItemType::ContextCompaction => TurnPhase::Executing,
                    AgentItemType::UserInput => TurnPhase::AwaitingInput,
                    AgentItemType::AgentMessage | AgentItemType::Reasoning => TurnPhase::Planning,
                }
            }
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
            Self::UsageUpdated { .. } | Self::ContextCompacted { .. } => TurnPhase::Executing,
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
        user_message: Option<ChatMessage>,
    },
    TurnSteered {
        thread_id: String,
        turn_id: String,
        message: ChatMessage,
    },
    TurnRejected {
        thread_id: String,
        turn_id: String,
        message: String,
    },
    ItemStarted {
        thread_id: String,
        turn_id: String,
        item_id: String,
        item_type: AgentItemType,
    },
    ItemCompleted {
        thread_id: String,
        turn_id: String,
        item_id: String,
        item_type: AgentItemType,
        status: AgentItemStatus,
    },
    ActivityStatusChanged {
        thread_id: String,
        turn_id: String,
        status: AgentActivityStatus,
    },
    TextDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
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
        context_usage: TokenUsage,
    },
    ContextCompacted {
        thread_id: String,
        turn_id: String,
        item_id: String,
        automatic: bool,
        compacted_message_count: usize,
        user_constraint_count: usize,
        recent_tool_result_count: usize,
        #[serde(default)]
        recent_user_message_count: usize,
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
        started_at_ms: u64,
        completed_at_ms: u64,
        duration_ms: u64,
    },
    TurnFailed {
        thread_id: String,
        turn_id: String,
        message: String,
        started_at_ms: u64,
        completed_at_ms: u64,
        duration_ms: u64,
    },
    TurnCancelled {
        thread_id: String,
        turn_id: String,
        started_at_ms: u64,
        completed_at_ms: u64,
        duration_ms: u64,
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
            "plugin_skill_read",
            "plugin_resource_read",
            "search_repository",
            "recall_memory",
            "request_user_input",
            "update_plan",
        ];
        const PLAN_ONLY: &[&str] = &[
            "list_directory",
            "read_file",
            "plugin_skill_read",
            "plugin_resource_read",
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

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserInputRequestKind {
    #[default]
    ModelQuestion,
    TurnContinuation,
}

/// `request_user_input` 工具发起的提问请求
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserInputRequest {
    pub id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub tool_call_id: String,
    #[serde(default)]
    pub kind: UserInputRequestKind,
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
    fn image_content_blocks_use_camel_case_and_read_legacy_jsonl() {
        let block = ContentBlock::Image {
            name: "screen.png".into(),
            data_url: "data:image/png;base64,iVBORw0KGgo=".into(),
        };
        let value = serde_json::to_value(&block).expect("image block should serialize");

        assert_eq!(value["type"], "image");
        assert_eq!(value["name"], "screen.png");
        assert_eq!(value["dataUrl"], "data:image/png;base64,iVBORw0KGgo=");
        assert!(value.get("data_url").is_none());

        let legacy: ContentBlock = serde_json::from_value(serde_json::json!({
            "type": "image",
            "name": "legacy.png",
            "data_url": "data:image/png;base64,bGVnYWN5"
        }))
        .expect("legacy image block should remain readable");
        assert!(matches!(
            legacy,
            ContentBlock::Image { name, data_url }
                if name == "legacy.png" && data_url == "data:image/png;base64,bGVnYWN5"
        ));
    }

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
    fn user_input_kind_is_typed_and_legacy_requests_default_to_model_questions() {
        let legacy = serde_json::json!({
            "id": "input-1",
            "threadId": "thread-1",
            "turnId": "turn-1",
            "toolCallId": "call-1",
            "questions": [],
            "createdAtMs": 1,
            "expiresAtMs": 2
        });
        let request: UserInputRequest = serde_json::from_value(legacy).unwrap();
        assert_eq!(request.kind, UserInputRequestKind::ModelQuestion);

        let mut continuation = request;
        continuation.kind = UserInputRequestKind::TurnContinuation;
        assert_eq!(
            serde_json::to_value(continuation).unwrap()["kind"],
            "turn_continuation"
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
    fn plugin_overview_uses_the_versioned_public_contract() {
        let overview = PluginOverview {
            schema_version: 1,
            root_path: r"D:\data\runtime-data\plugins".into(),
            plugins: vec![PluginDiagnostic {
                id: "review-tools@local".into(),
                name: "review-tools".into(),
                version: "1.2.3".into(),
                description: "Review helpers".into(),
                path: r"D:\data\runtime-data\plugins\review-tools".into(),
                enabled: true,
                state: PluginState::Degraded,
                deletable: true,
                components: PluginComponentSummary {
                    skill_count: 1,
                    mcp_server_count: 1,
                    mcp_tool_count: 2,
                    unsupported_count: 1,
                },
                warnings: vec!["Apps are not supported".into()],
                error: None,
            }],
            error: None,
        };

        let value = serde_json::to_value(overview).expect("plugin overview should serialize");

        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["plugins"][0]["state"], "degraded");
        assert_eq!(value["plugins"][0]["components"]["skillCount"], 1);
        assert_eq!(value["plugins"][0]["components"]["mcpToolCount"], 2);
        assert!(value["plugins"][0].get("schema_version").is_none());
    }

    #[test]
    fn turn_handle_uses_the_public_async_start_contract() {
        let handle = TurnHandle {
            schema_version: PROTOCOL_VERSION,
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            state: TurnState::Streaming,
        };

        let value = serde_json::to_value(handle).expect("turn handle should serialize");

        assert_eq!(value["schemaVersion"], PROTOCOL_VERSION);
        assert_eq!(value["threadId"], "thread-1");
        assert_eq!(value["turnId"], "turn-1");
        assert_eq!(value["state"], "streaming");
        assert!(value.get("error").is_none());
    }

    #[test]
    fn agent_event_envelope_is_versioned_and_flattened() {
        let event = AgentEventEnvelope::new(AgentEvent::TurnCancelled {
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
            started_at_ms: 10,
            completed_at_ms: 25,
            duration_ms: 15,
        });

        let value = serde_json::to_value(event).expect("agent event should serialize");

        assert_eq!(value["schemaVersion"], AGENT_EVENT_SCHEMA_VERSION);
        assert_eq!(value["type"], "turn_cancelled");
        assert_eq!(value["threadId"], "thread-1");
        assert_eq!(value["startedAtMs"], 10);
        assert_eq!(value["completedAtMs"], 25);
        assert_eq!(value["durationMs"], 15);
    }

    #[test]
    fn usage_event_separates_turn_totals_from_the_active_context_window() {
        let event = AgentEventEnvelope::new(AgentEvent::UsageUpdated {
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
            usage: TokenUsage {
                input_tokens: 90_000,
                output_tokens: 10_000,
                total_tokens: 100_000,
            },
            context_usage: TokenUsage {
                input_tokens: 14_000,
                output_tokens: 1_360,
                total_tokens: 15_360,
            },
        });

        let value = serde_json::to_value(event).expect("usage event should serialize");

        assert_eq!(value["schemaVersion"], 5);
        assert_eq!(value["type"], "usage_updated");
        assert_eq!(value["usage"]["totalTokens"], 100_000);
        assert_eq!(value["contextUsage"]["totalTokens"], 15_360);
    }

    #[test]
    fn assistant_item_lifecycle_uses_a_stable_public_identity() {
        let started = serde_json::to_value(AgentEventEnvelope::new(AgentEvent::ItemStarted {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            item_id: "message-1".into(),
            item_type: AgentItemType::AgentMessage,
        }))
        .unwrap();
        let completed = serde_json::to_value(AgentEventEnvelope::new(AgentEvent::ItemCompleted {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            item_id: "message-1".into(),
            item_type: AgentItemType::AgentMessage,
            status: AgentItemStatus::Completed,
        }))
        .unwrap();

        assert_eq!(started["type"], "item_started");
        assert_eq!(started["itemId"], "message-1");
        assert_eq!(started["itemType"], "agent_message");
        assert_eq!(completed["type"], "item_completed");
        assert_eq!(completed["itemId"], started["itemId"]);
        assert_eq!(completed["status"], "completed");
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

    #[test]
    fn assistant_text_delta_exposes_the_stable_item_id() {
        let event = AgentEventEnvelope::new(AgentEvent::TextDelta {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            item_id: "agent-message-1".into(),
            delta: "hello".into(),
        });
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["schemaVersion"], AGENT_EVENT_SCHEMA_VERSION);
        assert_eq!(value["type"], "text_delta");
        assert_eq!(value["itemId"], "agent-message-1");
        assert_eq!(value["delta"], "hello");
    }

    #[test]
    fn context_compaction_event_exposes_only_bounded_summary_counts() {
        let event = AgentEventEnvelope::new(AgentEvent::ContextCompacted {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            item_id: "compaction-1".into(),
            automatic: true,
            compacted_message_count: 18,
            user_constraint_count: 2,
            recent_tool_result_count: 3,
            recent_user_message_count: 4,
        });
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["type"], "context_compacted");
        assert_eq!(value["phase"], "executing");
        assert_eq!(value["itemId"], "compaction-1");
        assert_eq!(value["automatic"], true);
        assert_eq!(value["compactedMessageCount"], 18);
        assert_eq!(value["userConstraintCount"], 2);
        assert_eq!(value["recentToolResultCount"], 3);
        assert_eq!(value["recentUserMessageCount"], 4);
        assert!(value.get("summary").is_none());
    }

    #[test]
    fn mailbox_and_thread_control_requests_use_camel_case_contracts() {
        let steer = serde_json::to_value(TurnSteerRequest {
            thread_id: "thread-1".into(),
            expected_turn_id: "turn-1".into(),
            input: "adjust".into(),
            attachments: Vec::new(),
        })
        .unwrap();
        let queued_steer = serde_json::to_value(QueuedTurnSteerRequest {
            thread_id: "thread-1".into(),
            expected_turn_id: "turn-1".into(),
            queued_turn_id: "turn-2".into(),
        })
        .unwrap();
        let fork = serde_json::to_value(ThreadForkRequest {
            thread_id: "thread-1".into(),
            last_turn_id: Some("turn-1".into()),
        })
        .unwrap();
        let rollback = serde_json::to_value(ThreadRollbackRequest {
            thread_id: "thread-1".into(),
            num_turns: 2,
        })
        .unwrap();
        let mailbox = serde_json::to_value(ThreadMailboxSnapshot {
            schema_version: PROTOCOL_VERSION,
            thread_id: "thread-1".into(),
            revision: 7,
            active_turn_id: Some("turn-1".into()),
            pending: vec![QueuedTurn {
                schema_version: PROTOCOL_VERSION,
                turn_id: "turn-2".into(),
                thread_id: "thread-1".into(),
                kind: QueuedTurnKind::Message,
                input: "next".into(),
                agent_mode: None,
                workflow_id: Some("quality-assurance".into()),
                attachments: Vec::new(),
            }],
        })
        .unwrap();
        let steer_response = serde_json::to_value(TurnSteerResponse {
            schema_version: PROTOCOL_VERSION,
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
        })
        .unwrap();

        assert_eq!(steer["expectedTurnId"], "turn-1");
        assert_eq!(queued_steer["expectedTurnId"], "turn-1");
        assert_eq!(queued_steer["queuedTurnId"], "turn-2");
        assert_eq!(fork["lastTurnId"], "turn-1");
        assert_eq!(rollback["numTurns"], 2);
        assert_eq!(mailbox["schemaVersion"], PROTOCOL_VERSION);
        assert_eq!(mailbox["revision"], 7);
        assert_eq!(mailbox["pending"][0]["schemaVersion"], PROTOCOL_VERSION);
        assert_eq!(mailbox["pending"][0]["kind"], "message");
        assert_eq!(mailbox["pending"][0]["workflowId"], "quality-assurance");
        assert_eq!(mailbox["activeTurnId"], "turn-1");
        assert_eq!(steer_response["schemaVersion"], PROTOCOL_VERSION);
        assert_eq!(steer_response["threadId"], "thread-1");

        let legacy_queued: QueuedTurn = serde_json::from_value(serde_json::json!({
            "schemaVersion": PROTOCOL_VERSION,
            "turnId": "legacy-turn",
            "threadId": "thread-1",
            "kind": "message",
            "input": "legacy",
            "agentMode": null,
            "attachments": []
        }))
        .unwrap();
        assert_eq!(legacy_queued.workflow_id, None);
    }
}
