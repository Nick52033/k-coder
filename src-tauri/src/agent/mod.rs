use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::advanced::{
    COMPLETE_WORKFLOW_NODE_TOOL_NAME, REQUEST_USER_INPUT_TOOL_NAME, RequestUserInputTool,
    RuntimeMetrics,
};
use crate::context::{self, CompactionSummary, DEFAULT_CONTEXT_LIMIT};
use crate::logging::StructuredLogger;
use crate::policy::{
    ApprovalError, ApprovalManager, PolicyDecision, UserInputError, UserInputManager,
};
use crate::protocol::{
    AgentActivityStatus, AgentEvent, AgentEventEnvelope, AgentItemStatus, AgentItemType, AgentMode,
    ApprovalAction, ApprovalMode, ApprovalRequest, ApprovalResolution, ChangeSet, ChatMessage,
    ContentBlock, ExpectedFileHash, ImageAttachment, MessageRole, PROTOCOL_VERSION, PatchPreview,
    ReasoningEffort, TokenUsage, TokenUsageDetails, ToolCall, ToolResult, TurnError, TurnState,
    UserInputAction, UserInputQuestion, UserInputRequest, UserInputRequestKind,
    UserInputResolution,
};
use crate::providers::{Provider, ProviderError, ProviderEvent, ProviderMessage, ProviderRequest};
use crate::storage::{StorageError, StoredEvent, StoredEventKind, ThreadRepository, now_ms};
use crate::tools::{
    ApprovedToolExecution, PlanReconciliationContext, ToolContext, ToolError, ToolProgress,
    ToolRegistry, tool_progress_channel,
};

mod input;
use input::truncate_utf8;
pub(crate) mod instructions;
pub mod mailbox;
mod provider_history;
mod read_observation;
#[cfg(test)]
use read_observation::ReadObservationDecision;
use read_observation::{ReadObservationTracker, read_observation_result};
pub mod query_rewrite;
pub mod thread_operation;
pub(crate) use input::build_user_message;
use mailbox::TurnControl;
use provider_history::{last_active_context_usage, provider_history};

#[cfg(test)]
use input::{chat_to_provider, user_message};

const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_RESPONSE_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_REASONING_SUMMARY_BYTES: usize = 64 * 1024;
const MAX_PROVIDER_CONTEXT_BYTES: usize = 512 * 1024;
const MAX_TOOL_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_IDENTICAL_TOOL_CALLS: usize = 2;
/// 工具失败写入本地运行日志时保留的错误输出上限，避免一次失败写爆日志文件。
const MAX_TOOL_FAILURE_OUTPUT_BYTES: usize = 4 * 1024;
const PROGRESS_CHECK_WINDOW: usize = 5;
const MAX_NO_PROGRESS_WINDOWS: usize = 3;
const MAX_PROTOCOL_RETRIES: usize = 5;
/// Provider 流式响应的 idle 超时：每个事件之间最长静默时间。
/// 超过即判定流已死亡，未产生输出时自动重试并向界面发布重连事件；
/// 已有输出则失败而不是无限挂起。参考 codex 的 5 分钟默认。
const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
pub const DEFAULT_SOFT_TURN_PROVIDER_CALLS: u32 = 100;
pub const DEFAULT_SOFT_TURN_TOTAL_TOKENS: u64 = 5_000_000;
// Tool execution and provider latency alone must not interrupt an authorized turn.
pub const DEFAULT_SOFT_TURN_DURATION_MS: Option<u64> = None;
/// A hard cumulative cap prevents repeated continuation approvals from turning one
/// Turn into an unbounded model/tool loop. ZCode 的普通 Turn 不设调用次数硬上限，
/// loop 边界完全由上下文自动压缩与无进展检测承担；k-Coder 保留该背扑但放宽到 10 个
/// 软额度段，让真实边界仍是「每段人工续跑确认 + 无进展检测 + 自动压缩」。
pub const DEFAULT_HARD_TURN_PROVIDER_CALLS: u32 = 1_000;

const TURN_CONTINUATION_TOOL_CALL_ID: &str = "runtime-turn-continuation";
const TURN_CONTINUE: &str = "continue";
const TURN_COMPACT_AND_CONTINUE: &str = "compact_and_continue";
const TURN_STOP: &str = "stop";
const MAX_PLAN_RECONCILIATION_DRAFT_BYTES: usize = 16 * 1024;
const PLAN_RECONCILIATION_INSTRUCTIONS: &str = "[计划收尾门禁]\n当前回复已经形成最终答复草稿，但本轮刚刚更新的普通计划仍有进行中步骤。请先调用 update_plan 提交完整 steps 列表，只把已真实完成的步骤标为 completed，未完成步骤保留真实状态；工具成功后再输出最终答复。不要为了通过门禁伪造完成状态。收尾同步不得新增步骤、删除步骤、改变步骤数量或新增/替换步骤 ID；必须提交收尾请求开始前已有的完整 steps 列表，只更新真实状态和 detail，步骤文本也不要改名。若工具拒绝了计划结构，请按拒绝原因修正后重试。\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoftTurnLimits {
    provider_calls: u32,
    total_tokens: u64,
    duration_ms: Option<u64>,
}

impl Default for SoftTurnLimits {
    fn default() -> Self {
        Self {
            provider_calls: DEFAULT_SOFT_TURN_PROVIDER_CALLS,
            total_tokens: DEFAULT_SOFT_TURN_TOTAL_TOKENS,
            duration_ms: DEFAULT_SOFT_TURN_DURATION_MS,
        }
    }
}

impl SoftTurnLimits {
    #[cfg(test)]
    fn new(provider_calls: u32, total_tokens: u64, duration_ms: u64) -> Self {
        Self {
            provider_calls: provider_calls.max(1),
            total_tokens: total_tokens.max(1),
            duration_ms: Some(duration_ms),
        }
    }
}

struct SoftTurnSegment {
    provider_calls_at_start: u32,
    total_tokens_at_start: u64,
    started_at: Instant,
}

impl SoftTurnSegment {
    fn new(provider_calls: u32, total_tokens: u64) -> Self {
        Self {
            provider_calls_at_start: provider_calls,
            total_tokens_at_start: total_tokens,
            started_at: Instant::now(),
        }
    }

    fn usage(&self, provider_calls: u32, total_tokens: u64) -> SoftTurnSegmentUsage {
        SoftTurnSegmentUsage {
            provider_calls: provider_calls.saturating_sub(self.provider_calls_at_start),
            total_tokens: total_tokens.saturating_sub(self.total_tokens_at_start),
            duration_ms: self.started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }
    }

    fn reset(&mut self, provider_calls: u32, total_tokens: u64) {
        *self = Self::new(provider_calls, total_tokens);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SoftTurnSegmentUsage {
    provider_calls: u32,
    total_tokens: u64,
    duration_ms: u64,
}

impl SoftTurnSegmentUsage {
    fn exceeds(self, limits: SoftTurnLimits) -> bool {
        self.provider_calls > 0
            && (self.provider_calls >= limits.provider_calls
                || self.total_tokens >= limits.total_tokens
                || limits
                    .duration_ms
                    .is_some_and(|duration_ms| self.duration_ms >= duration_ms))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnContinuationDecision {
    Continue,
    CompactAndContinue,
    Stop,
}

/// 进展快照：用于检测任务是否有实质性进展
#[derive(Clone, PartialEq, Eq)]
struct ProgressSnapshot {
    /// 已观察到的不同文件内容变更和成功工具结果。
    progress_fingerprints: HashSet<u64>,
}

impl ProgressSnapshot {
    fn from_events(events: &[StoredEvent]) -> Self {
        let mut progress_fingerprints = HashSet::new();
        let mut reads = ReadObservationTracker::default();

        for event in events {
            let mut hasher = DefaultHasher::new();
            match &event.kind {
                StoredEventKind::ChangeApplied { change_set } => {
                    "change".hash(&mut hasher);
                    format!("{:?}", change_set.files).hash(&mut hasher);
                    progress_fingerprints.insert(hasher.finish());
                }
                StoredEventKind::ToolResult { name, result, .. } if result.success => {
                    if result
                        .metadata
                        .get("contentSuppressed")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        continue;
                    }
                    "tool".hash(&mut hasher);
                    name.hash(&mut hasher);
                    match name.as_str() {
                        "read_file" => {
                            if reads.observe(result).is_some() {
                                continue;
                            }
                            // Legacy or incomplete metadata has no reliable coverage range.
                            result.metadata.get("path").hash(&mut hasher);
                            result.metadata.get("fileRevision").hash(&mut hasher);
                            result.output.hash(&mut hasher);
                        }
                        "list_directory" => {
                            result.metadata.get("path").hash(&mut hasher);
                            result.metadata.get("directoryRevision").hash(&mut hasher);
                        }
                        "search_repository" => {
                            result.metadata.get("query").hash(&mut hasher);
                            result.metadata.get("resultRevision").hash(&mut hasher);
                        }
                        "run_command" => {
                            result.metadata.get("exitCode").hash(&mut hasher);
                            result.metadata.get("observationRevision").hash(&mut hasher);
                        }
                        _ => result.output.hash(&mut hasher),
                    }
                    progress_fingerprints.insert(hasher.finish());
                }
                _ => {}
            }
        }

        progress_fingerprints.extend(reads.progress_fingerprints());
        Self {
            progress_fingerprints,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunTurnRequest {
    pub thread_id: String,
    pub input: String,
    pub agent_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnOutcome {
    pub schema_version: u32,
    pub thread_id: String,
    pub turn_id: String,
    pub state: TurnState,
    pub error: Option<String>,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy)]
struct TurnTiming {
    started_at_ms: u64,
    completed_at_ms: u64,
    duration_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentRuntimeError {
    #[error("turn input is invalid: {0}")]
    InvalidInput(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Approval(#[from] ApprovalError),
    #[error(transparent)]
    UserInput(#[from] UserInputError),
    #[error("runtime instructions could not be compiled: {0}")]
    RuntimeInstructions(String),
    #[error("turn completion guard failed: {0}")]
    TurnCompletionGuard(String),
    #[error("change audit failed: {storage_error}; rollback also failed: {rollback_error}")]
    AuditCompensation {
        storage_error: String,
        rollback_error: String,
    },
}

pub trait EventPublisher: Send + Sync {
    fn publish(&self, event: AgentEventEnvelope);
}

pub trait RuntimeInstructionProvider: Send + Sync {
    /// Compile one immutable snapshot for an outer provider request. A
    /// transient retry of that request reuses the same snapshot.
    fn compile(&self) -> Result<String, String>;
}

/// Optional host-side check run immediately before a turn is marked completed.
///
/// The guard is deliberately injected by the command layer instead of making the
/// agent loop depend on PlanStore or workflow storage. Implementations return true
/// only when the current turn needs one bounded reconciliation request.
pub trait TurnCompletionGuard: Send + Sync {
    fn needs_reconciliation(&self, turn_started_at_ms: u64) -> Result<bool, String>;

    /// Returns the host-owned step identity snapshot to enforce while the
    /// completion reconciliation request is running.  Existing custom guards
    /// may omit this and retain the historical behavior; the live ordinary
    /// plan guard supplies it.
    fn reconciliation_context(
        &self,
        _turn_started_at_ms: u64,
    ) -> Result<Option<PlanReconciliationContext>, String> {
        Ok(None)
    }
}

impl<F> TurnCompletionGuard for F
where
    F: Fn(u64) -> Result<bool, String> + Send + Sync,
{
    fn needs_reconciliation(&self, turn_started_at_ms: u64) -> Result<bool, String> {
        self(turn_started_at_ms)
    }
}

impl<F> RuntimeInstructionProvider for F
where
    F: Fn() -> Result<String, String> + Send + Sync,
{
    fn compile(&self) -> Result<String, String> {
        self()
    }
}

pub struct AgentRuntime {
    repository: Arc<dyn ThreadRepository>,
    tools: ToolRegistry,
    workspace_root: PathBuf,
    approvals: Arc<ApprovalManager>,
    approval_mode: ApprovalMode,
    user_inputs: Arc<UserInputManager>,
    runtime_instruction_provider: Arc<dyn RuntimeInstructionProvider>,
    turn_completion_guard: Option<Arc<dyn TurnCompletionGuard>>,
    max_total_tokens: Option<u64>,
    max_provider_calls: Option<u32>,
    soft_turn_limits: Option<SoftTurnLimits>,
    context_limit: usize,
    working_context_limit: usize,
    metrics: Option<RuntimeMetrics>,
    reasoning_effort: ReasoningEffort,
    supports_vision: bool,
    logger: Option<StructuredLogger>,
    transient_retry_delays: Vec<Duration>,
    stream_idle_timeout: Duration,
}

impl AgentRuntime {
    pub fn new(repository: Arc<dyn ThreadRepository>) -> Self {
        Self::with_tools(
            repository,
            ToolRegistry::read_only(),
            std::env::current_dir().expect("current directory must be available"),
        )
    }

    pub fn with_tools(
        repository: Arc<dyn ThreadRepository>,
        tools: ToolRegistry,
        workspace_root: PathBuf,
    ) -> Self {
        Self::with_tools_and_approvals(
            repository,
            tools,
            workspace_root,
            Arc::new(ApprovalManager::new(std::time::Duration::from_secs(5 * 60))),
        )
    }

    pub fn with_tools_and_approvals(
        repository: Arc<dyn ThreadRepository>,
        tools: ToolRegistry,
        workspace_root: PathBuf,
        approvals: Arc<ApprovalManager>,
    ) -> Self {
        Self {
            repository,
            tools,
            workspace_root,
            approvals,
            approval_mode: ApprovalMode::Ask,
            user_inputs: Arc::new(UserInputManager::new()),
            runtime_instruction_provider: Arc::new(|| Ok(String::new())),
            turn_completion_guard: None,
            max_total_tokens: None,
            max_provider_calls: None,
            soft_turn_limits: None,
            context_limit: DEFAULT_CONTEXT_LIMIT,
            working_context_limit: context::default_working_context_limit(DEFAULT_CONTEXT_LIMIT),
            metrics: None,
            reasoning_effort: ReasoningEffort::default(),
            supports_vision: false,
            logger: None,
            transient_retry_delays: vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
            ],
            stream_idle_timeout: DEFAULT_STREAM_IDLE_TIMEOUT,
        }
    }

    pub fn with_runtime_instructions(mut self, instructions: String) -> Self {
        self.runtime_instruction_provider = Arc::new(move || Ok(instructions.clone()));
        self
    }

    pub fn with_runtime_instruction_provider(
        mut self,
        provider: Arc<dyn RuntimeInstructionProvider>,
    ) -> Self {
        self.runtime_instruction_provider = provider;
        self
    }

    pub fn with_turn_completion_guard(mut self, guard: Arc<dyn TurnCompletionGuard>) -> Self {
        self.turn_completion_guard = Some(guard);
        self
    }

    pub fn with_logger(mut self, logger: StructuredLogger) -> Self {
        self.logger = Some(logger);
        self
    }

    #[cfg(test)]
    fn with_transient_retry_delays(mut self, delays: Vec<Duration>) -> Self {
        self.transient_retry_delays = delays;
        self
    }

    #[cfg(test)]
    fn with_stream_idle_timeout(mut self, timeout: Duration) -> Self {
        self.stream_idle_timeout = timeout;
        self
    }

    pub fn with_approval_mode(mut self, mode: ApprovalMode) -> Self {
        self.approval_mode = mode;
        self
    }

    pub fn with_token_budget(mut self, max_total_tokens: u64) -> Self {
        self.max_total_tokens = Some(max_total_tokens);
        self
    }

    pub fn with_provider_call_budget(mut self, max_provider_calls: u32) -> Self {
        self.max_provider_calls = Some(max_provider_calls.max(1));
        self
    }

    pub fn with_soft_turn_limits(mut self, limits: SoftTurnLimits) -> Self {
        self.soft_turn_limits = Some(limits);
        self
    }

    pub fn with_context_limit(mut self, context_limit: usize) -> Self {
        self.context_limit = context_limit.max(1_024);
        self.working_context_limit = context::default_working_context_limit(self.context_limit);
        self
    }

    pub fn with_working_context_limit(mut self, working_context_limit: usize) -> Self {
        self.working_context_limit =
            context::normalize_working_context_limit(self.context_limit, working_context_limit);
        self
    }

    pub fn with_metrics(mut self, metrics: RuntimeMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn with_reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = effort;
        self
    }

    pub fn with_vision_support(mut self, supports_vision: bool) -> Self {
        self.supports_vision = supports_vision;
        self
    }

    pub fn with_user_inputs(mut self, manager: Arc<UserInputManager>) -> Self {
        self.user_inputs = manager;
        self
    }

    pub fn user_input_manager(&self) -> Arc<UserInputManager> {
        self.user_inputs.clone()
    }

    pub async fn run_turn(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        request: RunTurnRequest,
        cancellation: CancellationToken,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        self.run_turn_with_attachments(
            provider,
            model,
            request,
            Vec::new(),
            cancellation,
            publisher,
        )
        .await
    }

    pub async fn run_turn_with_attachments(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        request: RunTurnRequest,
        attachments: Vec<ImageAttachment>,
        cancellation: CancellationToken,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        self.run_turn_with_attachments_and_id(
            provider,
            model,
            request,
            attachments,
            Uuid::new_v4().to_string(),
            cancellation,
            publisher,
        )
        .await
    }

    pub async fn run_turn_with_attachments_and_id(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        request: RunTurnRequest,
        attachments: Vec<ImageAttachment>,
        turn_id: String,
        cancellation: CancellationToken,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        self.run_turn_with_attachments_id_and_optional_control(
            provider,
            model,
            request,
            attachments,
            turn_id,
            cancellation,
            None,
            publisher,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run_turn_with_attachments_id_and_control(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        request: RunTurnRequest,
        attachments: Vec<ImageAttachment>,
        turn_id: String,
        cancellation: CancellationToken,
        control: Arc<TurnControl>,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        self.run_turn_with_attachments_id_and_optional_control(
            provider,
            model,
            request,
            attachments,
            turn_id,
            cancellation,
            Some(control),
            publisher,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_turn_with_attachments_id_and_optional_control(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        request: RunTurnRequest,
        attachments: Vec<ImageAttachment>,
        turn_id: String,
        cancellation: CancellationToken,
        control: Option<Arc<TurnControl>>,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        let message = build_user_message(&request.input, attachments, self.supports_vision)?;
        let agent_mode = request
            .agent_mode
            .as_deref()
            .map(AgentMode::from_str)
            .unwrap_or_default();
        self.run_turn_inner(
            provider,
            model,
            request.thread_id,
            Some(message),
            agent_mode,
            false,
            turn_id,
            cancellation,
            control,
            publisher,
        )
        .await
    }

    pub async fn retry_turn(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        thread_id: String,
        agent_mode: AgentMode,
        cancellation: CancellationToken,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        self.retry_turn_with_id_and_optional_control(
            provider,
            model,
            thread_id,
            agent_mode,
            Uuid::new_v4().to_string(),
            cancellation,
            None,
            publisher,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn retry_turn_with_id_and_control(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        thread_id: String,
        agent_mode: AgentMode,
        turn_id: String,
        cancellation: CancellationToken,
        control: Arc<TurnControl>,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        self.retry_turn_with_id_and_optional_control(
            provider,
            model,
            thread_id,
            agent_mode,
            turn_id,
            cancellation,
            Some(control),
            publisher,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn retry_turn_with_id_and_optional_control(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        thread_id: String,
        agent_mode: AgentMode,
        turn_id: String,
        cancellation: CancellationToken,
        control: Option<Arc<TurnControl>>,
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

        self.run_turn_inner(
            provider,
            model,
            thread_id,
            None,
            agent_mode,
            true,
            turn_id,
            cancellation,
            control,
            publisher,
        )
        .await
    }

    pub async fn compact_thread(
        &self,
        thread_id: &str,
    ) -> Result<CompactionSummary, AgentRuntimeError> {
        let history =
            provider_history(self.repository.load(thread_id).await?, self.supports_vision);
        let (summary, _) = context::compact(
            history.messages(),
            self.working_context_limit.min(self.context_limit),
            history.summary(),
            history.user_context(),
        );
        if summary.compacted_message_count > 0 {
            let compaction_event = StoredEvent::new(
                thread_id,
                None,
                StoredEventKind::ContextCompacted {
                    summary: summary.clone(),
                    automatic: false,
                },
            );
            let item_id = compaction_event.event_id.clone();
            self.repository
                .append(StoredEvent::new(
                    thread_id,
                    None,
                    StoredEventKind::ItemStarted {
                        item_id: item_id.clone(),
                        item_type: AgentItemType::ContextCompaction,
                    },
                ))
                .await?;
            self.repository.append(compaction_event).await?;
            if let Some(metrics) = &self.metrics {
                metrics.compaction(
                    summary.estimated_before_tokens,
                    summary.estimated_after_tokens,
                    summary.compacted_message_count,
                    false,
                );
            }
            self.repository
                .append(StoredEvent::new(
                    thread_id,
                    None,
                    StoredEventKind::ItemCompleted {
                        item_id: item_id.clone(),
                        item_type: AgentItemType::ContextCompaction,
                        status: AgentItemStatus::Completed,
                    },
                ))
                .await?;
        }
        Ok(summary)
    }

    async fn run_turn_inner(
        &self,
        provider: Arc<dyn Provider>,
        model: String,
        thread_id: String,
        new_input: Option<ChatMessage>,
        agent_mode: AgentMode,
        retry_continuation: bool,
        turn_id: String,
        cancellation: CancellationToken,
        control: Option<Arc<TurnControl>>,
        publisher: Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        let existing = self.repository.load(&thread_id).await?;
        let previous_turn_interrupted = existing
            .iter()
            .rev()
            .find_map(|event| match event.kind {
                StoredEventKind::TurnFailed { .. } | StoredEventKind::TurnCancelled => Some(true),
                StoredEventKind::TurnCompleted { .. } | StoredEventKind::TurnStarted => Some(false),
                _ => None,
            })
            .unwrap_or(false);
        if existing
            .iter()
            .any(|event| matches!(event.kind, StoredEventKind::ThreadArchived))
        {
            return Err(AgentRuntimeError::InvalidInput(
                "archived threads cannot accept new turns".to_string(),
            ));
        }

        let started_user_message = new_input.clone();
        if let Some(message) = new_input {
            self.repository
                .append(StoredEvent::new(
                    &thread_id,
                    None,
                    StoredEventKind::UserMessage { message },
                ))
                .await?;
        }

        self.repository
            .append(StoredEvent::new(
                &thread_id,
                Some(turn_id.clone()),
                StoredEventKind::TurnModeSelected { mode: agent_mode },
            ))
            .await?;
        let turn_started_event = StoredEvent::new(
            &thread_id,
            Some(turn_id.clone()),
            StoredEventKind::TurnStarted,
        );
        let turn_started_at_ms = turn_started_event.created_at_ms;
        self.repository.append(turn_started_event).await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnStarted {
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
            user_message: started_user_message.clone(),
        }));

        let result = async {
        if cancellation.is_cancelled() {
            return self
                .finish_cancelled(&thread_id, &turn_id, &publisher)
                .await;
        }
        // 图片识别一律交给模型：当前模型不具备多模态能力时不发起 Provider 请求，
        // 直接在对话里给出提示，避免调用模型后得到无意义回答。
        let vision_unsupported = !self.supports_vision
            && started_user_message.as_ref().is_some_and(|message| {
                message
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::Image { .. }))
            });
        if vision_unsupported {
            return self
                .finish_vision_unsupported(&thread_id, &turn_id, &model, &publisher)
                .await;
        }

        let mut total_usage = TokenUsage::default();
        let mut has_usage = false;
        let mut provider_call_index = 0u32;
        let mut provider_context_bytes = 0usize;
        let mut last_call_signature = None::<String>;
        let mut identical_call_streak = 0usize;
        let mut read_observations = ReadObservationTracker::default();
        let token_budget = self.max_total_tokens;
        let mut soft_turn_segment = self
            .soft_turn_limits
            .map(|_| SoftTurnSegment::new(provider_call_index, total_usage.total_tokens));
        let mut force_compaction = false;
        let tool_definitions = self.tools.provider_definitions();
        let mut plan_reconciliation_requested = false;
        let mut plan_reconciliation_draft = None::<String>;
        let mut plan_reconciliation_request_pending = false;
        let mut plan_reconciliation_context = None::<PlanReconciliationContext>;
        // Reuse the streamed assistant item when the completion guard asks for one
        // reconciliation request. This lets the terminal TurnCompleted event replace
        // the temporary draft in the live timeline instead of leaving two answers.
        let mut plan_reconciliation_item_id = None::<String>;

        // 进展检测变量
        let mut no_progress_count = 0usize;
        let mut last_snapshot: Option<ProgressSnapshot> = None;

        let mut iteration = 0usize;
        loop {
            if cancellation.is_cancelled() {
                return self
                    .finish_cancelled(&thread_id, &turn_id, &publisher)
                    .await;
            }
            if let Some(control) = &control {
                self.persist_steered_messages(
                    &thread_id,
                    &turn_id,
                    control.take_pending(),
                    &publisher,
                )
                .await?;
            }
            if self
                .max_provider_calls
                .is_some_and(|limit| provider_call_index >= limit)
            {
                let limit = self.max_provider_calls.unwrap_or_default();
                let message = format!(
                    "单个 Turn 已达到模型调用硬上限（{} 次），为防止执行循环已停止；请检查当前进展后开启新 Turn。",
                    limit
                );
                return self
                    .finish_failed_with_error(
                        &thread_id,
                        &turn_id,
                        TurnError::provider_call_limit_exceeded(
                            message,
                            provider_call_index,
                            limit,
                        ),
                        &publisher,
                    )
                    .await;
            }
            if let (Some(limits), Some(segment)) =
                (self.soft_turn_limits, soft_turn_segment.as_mut())
            {
                let segment_usage = segment.usage(provider_call_index, total_usage.total_tokens);
                if segment_usage.exceeds(limits) {
                    match self
                        .request_turn_continuation(
                            &thread_id,
                            &turn_id,
                            segment_usage,
                            limits,
                            cancellation.clone(),
                            &publisher,
                        )
                        .await?
                    {
                        TurnContinuationDecision::Continue => {}
                        TurnContinuationDecision::CompactAndContinue => {
                            force_compaction = true;
                        }
                        TurnContinuationDecision::Stop => {
                            return self
                                .finish_cancelled(&thread_id, &turn_id, &publisher)
                                .await;
                        }
                    }
                    segment.reset(provider_call_index, total_usage.total_tokens);
                }
            }
            // 进展检测：每 PROGRESS_CHECK_WINDOW 轮检查一次
            if iteration > 0 && iteration % PROGRESS_CHECK_WINDOW == 0 {
                let events = self.repository.load(&thread_id).await?;
                let current_snapshot = ProgressSnapshot::from_events(&events);

                if let Some(ref last) = last_snapshot {
                    if current_snapshot == *last {
                        no_progress_count += 1;

                        // 警告：检测到无进展
                        eprintln!(
                            "[警告] 检测到无进展：连续 {} 个检查窗口（共 {} 轮）没有新的文件修改或工具输出",
                            no_progress_count,
                            no_progress_count * PROGRESS_CHECK_WINDOW
                        );

                        if no_progress_count >= MAX_NO_PROGRESS_WINDOWS {
                            publisher.publish(AgentEventEnvelope::new(AgentEvent::TextDelta {
                                thread_id: thread_id.clone(),
                                turn_id: turn_id.clone(),
                                item_id: format!("agent-message-{turn_id}-no-progress"),
                                delta: format!(
                                    "\n\n⚠️ 检测到连续 {} 轮无实质进展，提前终止任务。\n",
                                    no_progress_count * PROGRESS_CHECK_WINDOW
                                ),
                            }));

                            return self
                                .finish_failed(
                                    &thread_id,
                                    &turn_id,
                                    format!(
                                        "连续 {} 轮（{} 个检查窗口）无实质进展，任务可能陷入循环或无法继续",
                                        no_progress_count * PROGRESS_CHECK_WINDOW,
                                        no_progress_count
                                    ),
                                    &publisher,
                                )
                                .await;
                        }
                    } else {
                        // 有进展，重置计数器
                        if no_progress_count > 0 {
                            eprintln!("[信息] 检测到新进展，重置无进展计数器");
                        }
                        no_progress_count = 0;
                    }
                }

                last_snapshot = Some(current_snapshot);
            }

            let mut request_runtime_instructions = self
                .runtime_instruction_provider
                .compile()
                .map_err(AgentRuntimeError::RuntimeInstructions)?;
            if previous_turn_interrupted {
                request_runtime_instructions.push_str(instructions::INTERRUPTED_TASK);
            }
            if plan_reconciliation_requested {
                request_runtime_instructions.push_str(PLAN_RECONCILIATION_INSTRUCTIONS);
                if let Some(draft) = &plan_reconciliation_draft {
                    request_runtime_instructions.push_str(
                        "请在计划同步成功后复用或完善以下答复草稿，确保最终答复不会丢失已经完成的工作：\n",
                    );
                    request_runtime_instructions.push_str(truncate_utf8(
                        draft,
                        MAX_PLAN_RECONCILIATION_DRAFT_BYTES,
                    ));
                    request_runtime_instructions.push('\n');
                }
            }
            let events = self.repository.load(&thread_id).await?;
            let last_context_usage = last_active_context_usage(&events);
            let provider_history = provider_history(events, self.supports_vision);
            let mut history = provider_history.request_messages();
            if force_compaction
                || context::needs_compaction_for_request(
                    &history,
                    &request_runtime_instructions,
                    &tool_definitions,
                    self.working_context_limit.min(self.context_limit),
                )
                || last_context_usage.is_some_and(|usage| {
                    context::needs_compaction_for_usage(
                        usage.total_tokens,
                        self.working_context_limit.min(self.context_limit),
                    )
                })
            {
                force_compaction = false;
                let (summary, compacted) = context::compact(
                    provider_history.messages(),
                    self.working_context_limit.min(self.context_limit),
                    provider_history.summary(),
                    provider_history.user_context(),
                );
                if summary.compacted_message_count > 0 {
                    let compaction_event = StoredEvent::new(
                        &thread_id,
                        Some(turn_id.clone()),
                        StoredEventKind::ContextCompacted {
                            summary: summary.clone(),
                            automatic: true,
                        },
                    );
                    let item_id = compaction_event.event_id.clone();
                    self.start_item(
                        &thread_id,
                        &turn_id,
                        &item_id,
                        AgentItemType::ContextCompaction,
                        &publisher,
                    )
                    .await?;
                    self.repository
                        .append(compaction_event)
                        .await?;
                    if let Some(metrics) = &self.metrics {
                        metrics.compaction(
                            summary.estimated_before_tokens,
                            summary.estimated_after_tokens,
                            summary.compacted_message_count,
                            true,
                        );
                    }
                    publisher.publish(AgentEventEnvelope::new(AgentEvent::ContextCompacted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item_id: item_id.clone(),
                        automatic: true,
                        compacted_message_count: summary.compacted_message_count,
                        user_constraint_count: summary.user_constraints.len(),
                        recent_tool_result_count: summary.recent_tool_results.len(),
                        recent_user_message_count: summary.recent_user_messages.len(),
                    }));
                    self.complete_item(
                        &thread_id,
                        &turn_id,
                        &item_id,
                        AgentItemType::ContextCompaction,
                        AgentItemStatus::Completed,
                        &publisher,
                    )
                    .await?;
                    read_observations.reset_context();
                    history = compacted;
                }
            }
            if retry_continuation {
                history.push(ProviderMessage::Text {
                    role: MessageRole::User,
                    text: instructions::RETRY_CONTINUATION_REQUEST.to_string(),
                });
            }
            if !request_runtime_instructions.trim().is_empty() {
                history.insert(
                    0,
                    ProviderMessage::Text {
                        role: MessageRole::System,
                        text: request_runtime_instructions,
                    },
                );
            }
            let request = ProviderRequest {
                schema_version: PROTOCOL_VERSION,
                model: model.clone(),
                reasoning_effort: self.reasoning_effort,
                messages: history,
                tools: tool_definitions.clone(),
            };
            publisher.publish(AgentEventEnvelope::new(AgentEvent::ActivityStatusChanged {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                status: AgentActivityStatus::Thinking,
            }));
            let mut transient_retry_count = 0usize;
            let mut protocol_retry_count = 0usize;

            // 声明需要在重试循环外部的变量
            let mut response = String::new();
            let mut response_images = Vec::<ContentBlock>::new();
            let mut pending_tool_calls = Vec::<ToolCall>::new();
            let mut completed = false;
            let mut interrupted_for_steer = false;
            let reconciliation_request = plan_reconciliation_request_pending;
            plan_reconciliation_request_pending = false;
            let reusing_assistant_item = plan_reconciliation_item_id.is_some();
            let assistant_item_id = plan_reconciliation_item_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            if !reusing_assistant_item {
                self.start_item(
                    &thread_id,
                    &turn_id,
                    &assistant_item_id,
                    AgentItemType::AgentMessage,
                    &publisher,
                )
                .await?;
            }
            let provider_cancellation = control
                .as_ref()
                .map(|control| control.begin_provider_request(&cancellation))
                .unwrap_or_else(|| cancellation.child_token());

            // 外层循环：支持整个请求的重试
            'retry_loop: loop {
                publisher.publish(AgentEventEnvelope::new(AgentEvent::ActivityStatusChanged {
                    thread_id: thread_id.clone(), turn_id: turn_id.clone(), status: AgentActivityStatus::Thinking,
                }));
                let call_index = provider_call_index;
                provider_call_index = provider_call_index.saturating_add(1);
                let provider_started = std::time::Instant::now();
                let stream_result = tokio::select! {
                    _ = provider_cancellation.cancelled() => Err(ProviderError::Cancelled),
                    stream = provider.stream(request.clone(), provider_cancellation.clone()) => stream,
                };
                let mut stream = match stream_result {
                    Ok(stream) => stream,
                    Err(ProviderError::Cancelled) => {
                        self.record_provider_metric(provider_started, false, None);
                        if !cancellation.is_cancelled()
                            && provider_cancellation.is_cancelled()
                        {
                            interrupted_for_steer = true;
                            break 'retry_loop;
                        }
                        return self
                            .finish_cancelled(&thread_id, &turn_id, &publisher)
                            .await;
                    }
                    Err(error) => {
                        self.record_provider_metric(provider_started, false, None);
                        let retry_delay = if error.is_transient() {
                            self.transient_retry_delays
                                .get(transient_retry_count)
                                .copied()
                                .map(|delay| (error.rate_limit_delay().unwrap_or(delay), true))
                        } else if is_retryable_protocol_stream_error(&error)
                            && protocol_retry_count < MAX_PROTOCOL_RETRIES
                        {
                            Some((protocol_retry_delay(protocol_retry_count), false))
                        } else {
                            None
                        };

                        if let Some((delay, transient)) = retry_delay {
                            if error.rate_limit_delay().is_some() {
                                self.record_rate_limit_retry(&thread_id, &turn_id, &error, delay, transient_retry_count + 1);
                                publisher.publish(AgentEventEnvelope::new(AgentEvent::ProviderRetryWaiting {
                                    thread_id: thread_id.clone(), turn_id: turn_id.clone(),
                                    retry_at_ms: crate::providers::retry_at_ms(delay),
                                }));
                            } else if transient {
                                publisher.publish(AgentEventEnvelope::new(AgentEvent::ProviderStreamRetry {
                                    thread_id: thread_id.clone(), turn_id: turn_id.clone(),
                                    attempt: (transient_retry_count + 1) as u32,
                                    max_attempts: self.transient_retry_delays.len() as u32,
                                }));
                            }
                            if !wait_for_provider_retry(delay, &provider_cancellation).await {
                                if !cancellation.is_cancelled()
                                    && provider_cancellation.is_cancelled()
                                {
                                    interrupted_for_steer = true;
                                    break 'retry_loop;
                                }
                                return self
                                    .finish_cancelled(&thread_id, &turn_id, &publisher)
                                    .await;
                            }
                            if transient {
                                transient_retry_count += 1;
                            } else {
                                protocol_retry_count += 1;
                            }
                            self.record_provider_retry();
                            continue 'retry_loop;
                        }

                        let message = if error.is_transient()
                            && transient_retry_count > 0
                        {
                            format!(
                                "{} (已自动重试 {} 次)",
                                error, transient_retry_count
                            )
                        } else if is_retryable_protocol_stream_error(&error)
                            && protocol_retry_count > 0
                        {
                            format!("{} (已重试 {} 次)", error, protocol_retry_count)
                        } else {
                            error.to_string()
                        };
                        let mut turn_error = error.turn_error(message);
                        if is_retryable_protocol_stream_error(&error) {
                            turn_error.details = Some(json!({
                                "protocolRetries": protocol_retry_count,
                                "outputAlreadyStarted": false,
                            }));
                        }
                        // 重试已耗尽或不可重试：只有真实 HTTP 错误（4xx/5xx）才写本地
                        // 运行日志，网络抖动与取消不写。
                        self.record_http_failure(
                            &thread_id,
                            &turn_id,
                            &error,
                            error.is_transient(),
                        );
                        return self
                            .finish_failed_with_error(&thread_id, &turn_id, turn_error, &publisher)
                            .await;
                    }
                };

                let mut response_inner = String::new();
                let mut reasoning_summary_bytes = HashMap::<String, usize>::new();
                let mut reasoning_items_completed = HashSet::<String>::new();
                let mut responding_published = false;
                let mut pending_tool_calls_inner = Vec::new(); // 暂存 ToolCall，等 Completed 后再启动
                let mut response_images_inner = Vec::new();
                let mut iteration_usage_inner = None;
                let mut iteration_usage_details_inner = TokenUsageDetails::default();
                let mut iteration_provider_inner = None::<String>;
                let mut iteration_model_inner = Some(request.model.clone());
                let mut attempt_had_output = false;
                let completed_inner = loop {
                    let event = tokio::select! {
                        _ = provider_cancellation.cancelled() => {
                            if cancellation.is_cancelled() {
                                return self.finish_cancelled(&thread_id, &turn_id, &publisher).await;
                            }
                            self.record_provider_metric(
                                provider_started,
                                false,
                                iteration_usage_inner,
                            );
                            break None;
                        }
                        event = tokio::time::timeout(self.stream_idle_timeout, stream.next()) => {
                            match event {
                                Ok(event) => event,
                                Err(_elapsed) => Some(Err(ProviderError::Request(format!(
                                    "provider stream idle timeout: no events for {}s",
                                    self.stream_idle_timeout.as_secs()
                                )))),
                            }
                        },
                    };

                    match event {
                        Some(Ok(ProviderEvent::RetryWaiting { retry_at_ms })) => {
                            publisher.publish(AgentEventEnvelope::new(AgentEvent::ProviderRetryWaiting {
                                thread_id: thread_id.clone(), turn_id: turn_id.clone(), retry_at_ms,
                            }));
                        }
                        Some(Ok(ProviderEvent::RequestReady)) => {
                            publisher.publish(AgentEventEnvelope::new(AgentEvent::ActivityStatusChanged {
                                thread_id: thread_id.clone(), turn_id: turn_id.clone(), status: AgentActivityStatus::Thinking,
                            }));
                        }
                        Some(Ok(ProviderEvent::TextDelta { delta })) => {
                            attempt_had_output = true;
                            if response_inner.len().saturating_add(delta.len()) > MAX_RESPONSE_BYTES
                            {
                                return self
                                    .finish_failed(
                                        &thread_id,
                                        &turn_id,
                                        format!("response_limit: provider response exceeds {MAX_RESPONSE_BYTES} bytes"),
                                        &publisher,
                                    )
                                    .await;
                            }
                            response_inner.push_str(&delta);
                            if !responding_published {
                                publisher.publish(AgentEventEnvelope::new(
                                    AgentEvent::ActivityStatusChanged {
                                        thread_id: thread_id.clone(),
                                        turn_id: turn_id.clone(),
                                        status: AgentActivityStatus::Responding,
                                    },
                                ));
                                responding_published = true;
                            }
                            if !reconciliation_request {
                                publisher.publish(AgentEventEnvelope::new(AgentEvent::TextDelta {
                                    thread_id: thread_id.clone(),
                                    turn_id: turn_id.clone(),
                                    item_id: assistant_item_id.clone(),
                                    delta,
                                }));
                            }
                        }
                        Some(Ok(ProviderEvent::Image { mime_type, data })) => {
                            attempt_had_output = true;
                            let valid_mime = matches!(
                                mime_type.as_str(),
                                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                            );
                            let valid_base64 = base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                &data,
                            )
                            .is_ok();
                            if !valid_mime || !valid_base64 {
                                return self
                                    .finish_failed(
                                        &thread_id,
                                        &turn_id,
                                        "provider returned an invalid generated image".to_string(),
                                        &publisher,
                                    )
                                    .await;
                            }
                            if data.len() > MAX_RESPONSE_IMAGE_BYTES
                                || response_images_inner.iter().map(|image: &ContentBlock| match image {
                                    ContentBlock::Image { data_url, .. } => data_url.len(),
                                    _ => 0,
                                }).sum::<usize>().saturating_add(data.len()) > MAX_RESPONSE_IMAGE_BYTES
                            {
                                return self
                                    .finish_failed(
                                        &thread_id,
                                        &turn_id,
                                        format!("response_limit: generated images exceed {MAX_RESPONSE_IMAGE_BYTES} bytes"),
                                        &publisher,
                                    )
                                    .await;
                            }
                            let extension = mime_type
                                .split('/')
                                .nth(1)
                                .filter(|value| value.chars().all(|character| character.is_ascii_alphanumeric()))
                                .unwrap_or("png");
                            let index = response_images_inner.len() + 1;
                            response_images_inner.push(ContentBlock::Image {
                                name: format!("generated-image-{index}.{extension}"),
                                data_url: format!("data:{mime_type};base64,{data}"),
                            });
                        }
                        Some(Ok(ProviderEvent::ReasoningSummaryDelta { item_id, delta })) => {
                            attempt_had_output = true;
                            if reasoning_items_completed.contains(&item_id) {
                                continue;
                            }
                            if !reasoning_summary_bytes.contains_key(&item_id) {
                                self.start_item(
                                    &thread_id,
                                    &turn_id,
                                    &item_id,
                                    AgentItemType::Reasoning,
                                    &publisher,
                                )
                                .await?;
                            }
                            let total = reasoning_summary_bytes.entry(item_id.clone()).or_default();
                            *total = total.saturating_add(delta.len());
                            if *total > MAX_REASONING_SUMMARY_BYTES {
                                return self
                                    .finish_failed(
                                        &thread_id,
                                        &turn_id,
                                        format!(
                                            "response_limit: reasoning summary exceeds {MAX_REASONING_SUMMARY_BYTES} bytes"
                                        ),
                                        &publisher,
                                    )
                                    .await;
                            }
                            publisher.publish(AgentEventEnvelope::new(
                                AgentEvent::ReasoningSummaryDelta {
                                    thread_id: thread_id.clone(),
                                    turn_id: turn_id.clone(),
                                    item_id,
                                    delta,
                                },
                            ));
                        }
                        Some(Ok(ProviderEvent::ReasoningSummaryCompleted { item_id, summary })) => {
                            attempt_had_output = true;
                            if !reasoning_items_completed.insert(item_id.clone()) {
                                continue;
                            }
                            if !reasoning_summary_bytes.contains_key(&item_id) {
                                self.start_item(
                                    &thread_id,
                                    &turn_id,
                                    &item_id,
                                    AgentItemType::Reasoning,
                                    &publisher,
                                )
                                .await?;
                                reasoning_summary_bytes.insert(item_id.clone(), 0);
                            }
                            if summary.len() > MAX_REASONING_SUMMARY_BYTES {
                                return self
                                    .finish_failed(
                                        &thread_id,
                                        &turn_id,
                                        format!(
                                            "response_limit: reasoning summary exceeds {MAX_REASONING_SUMMARY_BYTES} bytes"
                                        ),
                                        &publisher,
                                    )
                                    .await;
                            }
                            self.repository
                                .append(StoredEvent::new(
                                    &thread_id,
                                    Some(turn_id.clone()),
                                    StoredEventKind::ReasoningSummary {
                                        item_id: item_id.clone(),
                                        summary: summary.clone(),
                                    },
                                ))
                                .await?;
                            publisher.publish(AgentEventEnvelope::new(
                                AgentEvent::ReasoningSummaryCompleted {
                                    thread_id: thread_id.clone(),
                                    turn_id: turn_id.clone(),
                                    item_id: item_id.clone(),
                                    summary,
                                },
                            ));
                            self.complete_item(
                                &thread_id,
                                &turn_id,
                                &item_id,
                                AgentItemType::Reasoning,
                                AgentItemStatus::Completed,
                                &publisher,
                            )
                            .await?;
                        }
                        Some(Ok(ProviderEvent::ToolCall { call })) => {
                            attempt_had_output = true;
                            // 先暂存，等 AI 完成后再启动执行
                            pending_tool_calls_inner.push(call);
                        }
                        Some(Ok(ProviderEvent::ProviderContext { provider, item })) => {
                            attempt_had_output = true;
                            let item_bytes = serde_json::to_vec(&item)
                                .map_err(|error| {
                                    AgentRuntimeError::InvalidInput(error.to_string())
                                })?
                                .len();
                            if provider_context_bytes.saturating_add(item_bytes)
                                > MAX_PROVIDER_CONTEXT_BYTES
                            {
                                return self
                                    .finish_failed(
                                        &thread_id,
                                        &turn_id,
                                        format!(
                                            "response_limit: provider context exceeds {MAX_PROVIDER_CONTEXT_BYTES} bytes"
                                        ),
                                        &publisher,
                                    )
                                    .await;
                            }
                            provider_context_bytes =
                                provider_context_bytes.saturating_add(item_bytes);
                            self.repository
                                .append(StoredEvent::new(
                                    &thread_id,
                                    Some(turn_id.clone()),
                                    StoredEventKind::ProviderContext { provider, item },
                                ))
                                .await?;
                        }
                        Some(Ok(ProviderEvent::ModelSelected { provider, model })) => {
                            iteration_provider_inner = Some(provider);
                            iteration_model_inner = Some(model);
                        }
                        Some(Ok(event @ ProviderEvent::Usage { .. }))
                        | Some(Ok(event @ ProviderEvent::DetailedUsage { .. })) => {
                            let (usage, details) = match event {
                                ProviderEvent::Usage { usage } => {
                                    (usage, TokenUsageDetails::default())
                                }
                                ProviderEvent::DetailedUsage { usage, details } => (usage, details),
                                _ => unreachable!(),
                            };
                            iteration_usage_inner = Some(usage);
                            iteration_usage_details_inner.merge_reported(details);
                            let aggregate = add_usage(total_usage, usage);
                            if let Some(budget) =
                                token_budget.filter(|budget| aggregate.total_tokens > *budget)
                            {
                                self.persist_provider_usage(
                                    &thread_id,
                                    &turn_id,
                                    call_index,
                                    usage,
                                    details,
                                    iteration_provider_inner.as_deref(),
                                    iteration_model_inner.as_deref(),
                                    &mut total_usage,
                                    &mut has_usage,
                                    &publisher,
                                )
                                .await?;
                                return self
                                    .finish_failed(
                                        &thread_id,
                                        &turn_id,
                                        format!(
                                            "token_budget_exceeded: used {} of {} tokens",
                                            total_usage.total_tokens, budget
                                        ),
                                        &publisher,
                                    )
                                    .await;
                            }
                        }
                        Some(Ok(ProviderEvent::Completed)) => {
                            self.record_provider_metric(
                                provider_started,
                                true,
                                iteration_usage_inner,
                            );
                            break Some(true);
                        }
                        Some(Err(ProviderError::Cancelled)) => {
                            self.record_provider_metric(
                                provider_started,
                                false,
                                iteration_usage_inner,
                            );
                            if let Some(usage) = iteration_usage_inner {
                                self.persist_provider_usage(
                                    &thread_id,
                                    &turn_id,
                                    call_index,
                                    usage,
                                    iteration_usage_details_inner,
                                    iteration_provider_inner.as_deref(),
                                    iteration_model_inner.as_deref(),
                                    &mut total_usage,
                                    &mut has_usage,
                                    &publisher,
                                )
                                .await?;
                            }
                            if cancellation.is_cancelled() {
                                return self
                                    .finish_cancelled(&thread_id, &turn_id, &publisher)
                                    .await;
                            }
                            if provider_cancellation.is_cancelled() {
                                break None;
                            }
                            return self
                                .finish_cancelled(&thread_id, &turn_id, &publisher)
                                .await;
                        }
                        Some(Err(error)) => {
                            self.record_provider_metric(
                                provider_started,
                                false,
                                iteration_usage_inner,
                            );

                            if let Some(usage) = iteration_usage_inner {
                                self.persist_provider_usage(
                                    &thread_id,
                                    &turn_id,
                                    call_index,
                                    usage,
                                    iteration_usage_details_inner,
                                    iteration_provider_inner.as_deref(),
                                    iteration_model_inner.as_deref(),
                                    &mut total_usage,
                                    &mut has_usage,
                                    &publisher,
                                )
                                .await?;
                            }

                            let transient_stream_error =
                                error.is_transient() && !attempt_had_output;
                            let retry_delay = if transient_stream_error {
                                self.transient_retry_delays
                                    .get(transient_retry_count)
                                    .copied()
                                    .map(|delay| (error.rate_limit_delay().unwrap_or(delay), true))
                            } else if is_retryable_protocol_stream_error(&error)
                                && !attempt_had_output
                                && protocol_retry_count < MAX_PROTOCOL_RETRIES
                            {
                                Some((protocol_retry_delay(protocol_retry_count), false))
                            } else {
                                None
                            };
                            if let Some((delay, transient)) = retry_delay {
                            if error.rate_limit_delay().is_some() {
                                self.record_rate_limit_retry(&thread_id, &turn_id, &error, delay, transient_retry_count + 1);
                                publisher.publish(AgentEventEnvelope::new(AgentEvent::ProviderRetryWaiting {
                                    thread_id: thread_id.clone(), turn_id: turn_id.clone(),
                                    retry_at_ms: crate::providers::retry_at_ms(delay),
                                }));
                            } else if transient {
                                publisher.publish(AgentEventEnvelope::new(AgentEvent::ProviderStreamRetry {
                                    thread_id: thread_id.clone(), turn_id: turn_id.clone(),
                                    attempt: (transient_retry_count + 1) as u32,
                                    max_attempts: self.transient_retry_delays.len() as u32,
                                }));
                            }
                                if !wait_for_provider_retry(delay, &provider_cancellation).await {
                                    if !cancellation.is_cancelled()
                                        && provider_cancellation.is_cancelled()
                                    {
                                        interrupted_for_steer = true;
                                        break 'retry_loop;
                                    }
                                    return self
                                        .finish_cancelled(&thread_id, &turn_id, &publisher)
                                        .await;
                                }
                                if transient {
                                    transient_retry_count += 1;
                                } else {
                                    protocol_retry_count += 1;
                                }
                                self.record_provider_retry();
                                continue 'retry_loop;
                            }

                            let message = if error.is_transient()
                                && transient_retry_count > 0
                            {
                                format!(
                                    "{} (已自动重试 {} 次)",
                                    error, transient_retry_count
                                )
                            } else if is_retryable_protocol_stream_error(&error)
                                && protocol_retry_count > 0
                            {
                                format!("{} (已重试 {} 次)", error, protocol_retry_count)
                            } else {
                                error.to_string()
                            };
                            let mut turn_error = error.turn_error(message);
                            if is_retryable_protocol_stream_error(&error) {
                                turn_error.details = Some(json!({
                                    "protocolRetries": protocol_retry_count,
                                    "outputAlreadyStarted": attempt_had_output,
                                }));
                            }
                            self.record_http_failure(
                                &thread_id,
                                &turn_id,
                                &error,
                                error.is_transient(),
                            );
                            return self
                                .finish_failed_with_error(&thread_id, &turn_id, turn_error, &publisher)
                                .await;
                        }
                        None => {
                            self.record_provider_metric(
                                provider_started,
                                false,
                                iteration_usage_inner,
                            );
                            if let Some(usage) = iteration_usage_inner {
                                self.persist_provider_usage(
                                    &thread_id,
                                    &turn_id,
                                    call_index,
                                    usage,
                                    iteration_usage_details_inner,
                                    iteration_provider_inner.as_deref(),
                                    iteration_model_inner.as_deref(),
                                    &mut total_usage,
                                    &mut has_usage,
                                    &publisher,
                                )
                                .await?;
                                iteration_usage_inner = None;
                            }
                            break Some(false);
                        }
                    }
                };

                if let Some(usage) = iteration_usage_inner {
                    self.persist_provider_usage(
                        &thread_id,
                        &turn_id,
                        call_index,
                        usage,
                        iteration_usage_details_inner,
                        iteration_provider_inner.as_deref(),
                        iteration_model_inner.as_deref(),
                        &mut total_usage,
                        &mut has_usage,
                        &publisher,
                    )
                    .await?;
                }
                response = response_inner;
                response_images = response_images_inner;
                pending_tool_calls = pending_tool_calls_inner;
                if let Some(provider_completed) = completed_inner {
                    completed = provider_completed;
                } else {
                    interrupted_for_steer = true;
                }
                break;
            } // 'retry_loop 结束

            if let Some(control) = &control {
                control.end_provider_request();
            }
            if interrupted_for_steer {
                if cancellation.is_cancelled() {
                    return self
                        .finish_cancelled(&thread_id, &turn_id, &publisher)
                        .await;
                }
                let steered = control
                    .as_ref()
                    .map(|control| control.take_pending())
                    .unwrap_or_default();
                if steered.is_empty() {
                    return self
                        .finish_cancelled(&thread_id, &turn_id, &publisher)
                        .await;
                }
                self.continue_after_provider_steer(
                    &thread_id,
                    &turn_id,
                    &assistant_item_id,
                    response,
                    response_images,
                    steered,
                    &publisher,
                )
                .await?;
                iteration = iteration.saturating_add(1);
                continue;
            }

            if !completed {
                return self
                    .finish_failed(
                        &thread_id,
                        &turn_id,
                        ProviderError::Interrupted.to_string(),
                        &publisher,
                    )
                    .await;
            }
            // AI 完成输出后的处理
            if pending_tool_calls.is_empty() {
                if response.is_empty() && response_images.is_empty() {
                    return self
                        .finish_failed(
                            &thread_id,
                            &turn_id,
                            "provider completed without text or a tool call".to_string(),
                            &publisher,
                        )
                        .await;
                }
                if let Some(steered) = control
                    .as_ref()
                    .and_then(|control| control.close_if_idle())
                {
                    let message = assistant_message_with_content(
                        assistant_item_id.clone(),
                        response,
                        response_images,
                    );
                    self.repository
                        .append(StoredEvent::new(
                            &thread_id,
                            Some(turn_id.clone()),
                            StoredEventKind::AssistantMessage { message },
                        ))
                        .await?;
                    self.complete_active_items(
                        &thread_id,
                        &turn_id,
                        AgentItemType::AgentMessage,
                        AgentItemStatus::Completed,
                        &publisher,
                    )
                    .await?;
                    self.persist_steered_messages(
                        &thread_id,
                        &turn_id,
                        steered,
                        &publisher,
                    )
                    .await?;
                    iteration = iteration.saturating_add(1);
                    continue;
                }
                if let Some(guard) = &self.turn_completion_guard {
                    let needs_reconciliation = guard
                        .needs_reconciliation(turn_started_at_ms)
                        .map_err(AgentRuntimeError::TurnCompletionGuard)?;
                    if needs_reconciliation {
                        if !plan_reconciliation_requested {
                            plan_reconciliation_requested = true;
                            plan_reconciliation_context = guard
                                .reconciliation_context(turn_started_at_ms)
                                .map_err(AgentRuntimeError::TurnCompletionGuard)?;
                            plan_reconciliation_draft = Some(response.clone());
                            plan_reconciliation_item_id = Some(assistant_item_id.clone());
                            plan_reconciliation_request_pending = true;
                            publisher.publish(AgentEventEnvelope::new(AgentEvent::TextReset {
                                thread_id: thread_id.clone(),
                                turn_id: turn_id.clone(),
                                item_id: assistant_item_id.clone(),
                            }));
                            publisher.publish(AgentEventEnvelope::new(
                                AgentEvent::ActivityStatusChanged {
                                    thread_id: thread_id.clone(),
                                    turn_id: turn_id.clone(),
                                    status: AgentActivityStatus::Finalizing,
                                },
                            ));
                            // Keep the item active and reuse its ID for the bounded
                            // reconciliation request. The terminal TurnCompleted event then
                            // replaces the temporary streamed draft with the authoritative
                            // persisted answer; no draft AssistantMessage is written.
                            iteration = iteration.saturating_add(1);
                            continue;
                        }
                        return self
                            .finish_failed(
                                &thread_id,
                                &turn_id,
                                "计划收尾同步失败：本轮结束前仍有进行中的步骤；请检查计划后重新发送或继续。".to_string(),
                                &publisher,
                            )
                            .await;
                    }
                }
                publisher.publish(AgentEventEnvelope::new(AgentEvent::ActivityStatusChanged {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    status: AgentActivityStatus::Finalizing,
                }));
                return self
                    .finish_completed(
                        &thread_id,
                        &turn_id,
                        &assistant_item_id,
                        response,
                        response_images,
                        has_usage.then_some(total_usage),
                        &publisher,
                    )
                    .await;
            }

            publisher.publish(AgentEventEnvelope::new(AgentEvent::ActivityStatusChanged {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                status: AgentActivityStatus::RunningTool,
            }));

            // 持久化 AssistantToolCalls 事件
            self.repository
                .append(StoredEvent::new(
                    &thread_id,
                    Some(turn_id.clone()),
                    StoredEventKind::AssistantToolCalls {
                        item_id: Some(assistant_item_id.clone()),
                        // The first guarded response is a temporary draft. Keep tool
                        // calls for provider history, but do not project that draft as a
                        // second commentary message after refresh.
                        text: if reconciliation_request {
                            String::new()
                        } else {
                            response.clone()
                        },
                        calls: pending_tool_calls.clone(),
                    },
                ))
                .await?;
            if !reconciliation_request {
                self.complete_active_items(
                    &thread_id,
                    &turn_id,
                    AgentItemType::AgentMessage,
                    AgentItemStatus::Completed,
                    &publisher,
                )
                .await?;
            }
            for call in &pending_tool_calls {
                self.start_item(
                    &thread_id,
                    &turn_id,
                    &call.id,
                    AgentItemType::Tool,
                    &publisher,
                )
                .await?;
                publisher.publish(AgentEventEnvelope::new(AgentEvent::ToolQueued {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    call: call.clone(),
                }));
            }

            let mut stop_reason: Option<String> = None;
            let mut cancelled_batch = false;
            let mut fatal_error = None;
            for call in pending_tool_calls {
                let signature = call_signature(&call);
                if last_call_signature.as_deref() == Some(signature.as_str()) {
                    identical_call_streak = identical_call_streak.saturating_add(1);
                } else {
                    last_call_signature = Some(signature);
                    identical_call_streak = 1;
                }
                let repeated_identical_call = identical_call_streak > MAX_IDENTICAL_TOOL_CALLS;

                if call.name != "read_file" && repeated_identical_call {
                    let reason = format!(
                        "repeated_tool_call: {} was requested with identical arguments more than {MAX_IDENTICAL_TOOL_CALLS} consecutive times",
                        call.name
                    );
                    stop_reason = Some(reason.clone());

                    let result = failure_result(reason);
                    if let Some(metrics) = &self.metrics {
                        metrics.tool(result.success);
                    }
                    self.persist_tool_result(
                        &thread_id,
                        &turn_id,
                        &call,
                        &result,
                        AgentItemStatus::Failed,
                        &publisher,
                    )
                    .await?;
                    continue;
                }

                if let Some(reason) = &stop_reason {
                    let result = failure_result(format!("tool execution skipped: {reason}"));
                    if let Some(metrics) = &self.metrics {
                        metrics.tool(result.success);
                    }
                    let status = if cancelled_batch {
                        AgentItemStatus::Cancelled
                    } else {
                        AgentItemStatus::Failed
                    };
                    self.persist_tool_result(
                        &thread_id,
                        &turn_id,
                        &call,
                        &result,
                        status,
                        &publisher,
                    )
                    .await?;
                    continue;
                }

                let context = ToolContext {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    call_id: call.id.clone(),
                    workspace_root: self.workspace_root.clone(),
                    approval: None,
                    progress: None,
                    plan_reconciliation: if plan_reconciliation_requested {
                        plan_reconciliation_context.clone()
                    } else {
                        None
                    },
                };

                let tool_started_at = tokio::time::Instant::now();
                let (mut result, mut item_status) = match self
                    .execute_tool_with_progress(context, &call, cancellation.clone(), &publisher)
                    .await
                {
                    Ok(Some(tool_result)) => {
                        let result = bound_tool_result(tool_result);
                        let status = if result.success {
                            AgentItemStatus::Completed
                        } else {
                            AgentItemStatus::Failed
                        };
                        (result, status)
                    }
                    Ok(None) => {
                        cancelled_batch = true;
                        stop_reason = Some("turn cancellation".to_string());
                        (
                            failure_result("tool execution was cancelled".to_string()),
                            AgentItemStatus::Cancelled,
                        )
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if fatal_error.is_none() {
                            cancellation.cancel();
                            stop_reason = Some(format!("tool batch aborted: {message}"));
                            fatal_error = Some(error);
                        }
                        (failure_result(message), AgentItemStatus::Failed)
                    }
                };

                if is_pending_subagent_wait(&call, &result, tool_started_at.elapsed()) {
                    // A bounded blocking wait is legitimate while children do work.
                    // Snapshot polling, finished results and failures retain loop guards.
                    identical_call_streak = 0;
                    no_progress_count = 0;
                }

                if call.name == "read_file"
                    && let Some(decision) = read_observations.observe(&result)
                {
                    result = read_observation_result(result, decision);
                } else if call.name == "read_file" && repeated_identical_call {
                    let reason = format!(
                        "repeated_tool_call: {} was requested with identical arguments more than {MAX_IDENTICAL_TOOL_CALLS} consecutive times without producing a versioned observation",
                        call.name
                    );
                    result = failure_result(reason.clone());
                    item_status = AgentItemStatus::Failed;
                    stop_reason = Some(reason);
                }

                if call.name == COMPLETE_WORKFLOW_NODE_TOOL_NAME && result.success {
                    read_observations.reset_context();
                }

                if let Some(metrics) = &self.metrics {
                    metrics.tool(result.success);
                }
                self.persist_tool_result(
                    &thread_id,
                    &turn_id,
                    &call,
                    &result,
                    item_status,
                    &publisher,
                )
                .await?;
            }

            if let Some(error) = fatal_error {
                return Err(error);
            }
            if cancelled_batch {
                return self
                    .finish_cancelled(&thread_id, &turn_id, &publisher)
                    .await;
            }
            if let Some(reason) = stop_reason {
                return self
                    .finish_failed(&thread_id, &turn_id, reason, &publisher)
                    .await;
            }
            iteration = iteration.saturating_add(1);
        }
        }
        .await;

        match result {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                let message = error.to_string();
                let _ = self
                    .finish_failed(&thread_id, &turn_id, message, &publisher)
                    .await;
                Err(error)
            }
        }
    }

    async fn persist_steered_messages(
        &self,
        thread_id: &str,
        turn_id: &str,
        messages: Vec<ChatMessage>,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        for message in messages {
            self.repository
                .append(StoredEvent::new(
                    thread_id,
                    Some(turn_id.to_string()),
                    StoredEventKind::UserMessage {
                        message: message.clone(),
                    },
                ))
                .await?;
            publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnSteered {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                message,
            }));
        }
        Ok(())
    }

    async fn continue_after_provider_steer(
        &self,
        thread_id: &str,
        turn_id: &str,
        assistant_item_id: &str,
        response: String,
        response_images: Vec<ContentBlock>,
        steered: Vec<ChatMessage>,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        if !response.is_empty() || !response_images.is_empty() {
            let message = assistant_message_with_content(
                assistant_item_id.to_string(),
                response,
                response_images,
            );
            self.repository
                .append(StoredEvent::new(
                    thread_id,
                    Some(turn_id.to_string()),
                    StoredEventKind::AssistantMessage { message },
                ))
                .await?;
        }
        self.complete_active_non_message_items(
            thread_id,
            turn_id,
            AgentItemStatus::Cancelled,
            publisher,
        )
        .await?;
        self.complete_item(
            thread_id,
            turn_id,
            assistant_item_id,
            AgentItemType::AgentMessage,
            AgentItemStatus::Cancelled,
            publisher,
        )
        .await?;
        self.persist_steered_messages(thread_id, turn_id, steered, publisher)
            .await
    }

    async fn execute_tool_with_progress(
        &self,
        mut context: ToolContext,
        call: &ToolCall,
        cancellation: CancellationToken,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<Option<ToolResult>, AgentRuntimeError> {
        let (progress_tx, mut progress_rx) = tool_progress_channel();
        context.progress = Some(progress_tx);
        self.repository
            .append(StoredEvent::new(
                &context.thread_id,
                Some(context.turn_id.clone()),
                StoredEventKind::ToolStarted {
                    call_id: call.id.clone(),
                },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::ToolStarted {
            thread_id: context.thread_id.clone(),
            turn_id: context.turn_id.clone(),
            call: call.clone(),
        }));

        let result = {
            let execution = self.execute_tool_call(&context, call, cancellation, publisher);
            tokio::pin!(execution);
            let mut progress_open = true;
            loop {
                tokio::select! {
                    result = &mut execution => break result,
                    progress = progress_rx.recv(), if progress_open => {
                        if let Some(progress) = progress {
                            publish_tool_progress(
                                publisher,
                                &context.thread_id,
                                &context.turn_id,
                                &call.id,
                                progress,
                            );
                        } else {
                            progress_open = false;
                        }
                    }
                }
            }
        };
        while let Ok(progress) = progress_rx.try_recv() {
            publish_tool_progress(
                publisher,
                &context.thread_id,
                &context.turn_id,
                &call.id,
                progress,
            );
        }

        result
    }

    async fn execute_tool_call(
        &self,
        context: &ToolContext,
        call: &ToolCall,
        cancellation: CancellationToken,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<Option<ToolResult>, AgentRuntimeError> {
        // 拦截 request_user_input：不经过普通 dispatch，而是通过 UserInputManager
        // 向前端发起提问并阻塞等待回答。
        if call.name == REQUEST_USER_INPUT_TOOL_NAME {
            return self
                .execute_request_user_input(context, call, cancellation, publisher)
                .await;
        }

        // 拦截 todo_write：直接发送事件
        if call.name == crate::advanced::TODO_WRITE_TOOL_NAME {
            return self.execute_todo_write(context, call, publisher).await;
        }

        let authorization = match self.tools.authorization(&call.name, &call.arguments) {
            Ok(authorization) => authorization,
            Err(error) => return Ok(Some(failure_result(error.to_string()))),
        };
        match authorization.decision {
            PolicyDecision::Deny { reason } => {
                return Ok(Some(failure_result(format!(
                    "tool execution denied: {reason}"
                ))));
            }
            PolicyDecision::Allow => {
                return Ok(Some(
                    match self
                        .tools
                        .dispatch_authorized(
                            context,
                            &call.name,
                            call.arguments.clone(),
                            cancellation,
                        )
                        .await
                    {
                        Ok(result) => result,
                        Err(ToolError::Cancelled) => return Ok(None),
                        Err(error) => failure_result(error.to_string()),
                    },
                ));
            }
            PolicyDecision::RequireApproval { reason } => {
                let preview = match self.tools.preview(context, &call.name, &call.arguments) {
                    Ok(preview) => preview,
                    Err(error) => return Ok(Some(failure_result(error.to_string()))),
                };
                let request_id = Uuid::new_v4().to_string();
                let created_at_ms = now_ms();
                let auto_approve = self.approval_mode == ApprovalMode::FullAccess;
                let request = ApprovalRequest {
                    id: request_id.clone(),
                    thread_id: context.thread_id.clone(),
                    turn_id: context.turn_id.clone(),
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    reason: if auto_approve {
                        format!("full-access mode automatically approved: {reason}")
                    } else {
                        reason
                    },
                    auto_approved: auto_approve,
                    risk: authorization.risk,
                    arguments: call.arguments.clone(),
                    preview: preview.clone(),
                    created_at_ms,
                    expires_at_ms: created_at_ms.saturating_add(self.approvals.timeout_ms()),
                };
                let receiver = if auto_approve {
                    None
                } else {
                    match self.approvals.register(&request_id).await {
                        Ok(receiver) => Some(receiver),
                        Err(error) => return Ok(Some(failure_result(error.to_string()))),
                    }
                };
                if let Err(error) = self
                    .start_item(
                        &context.thread_id,
                        &context.turn_id,
                        &request_id,
                        AgentItemType::Approval,
                        publisher,
                    )
                    .await
                {
                    if !auto_approve {
                        self.approvals.discard(&request_id).await;
                    }
                    return Err(error);
                }
                if let Err(error) = self
                    .repository
                    .append(StoredEvent::new(
                        &context.thread_id,
                        Some(context.turn_id.clone()),
                        StoredEventKind::ApprovalRequested {
                            request: request.clone(),
                        },
                    ))
                    .await
                {
                    if !auto_approve {
                        self.approvals.discard(&request_id).await;
                    }
                    return Err(error.into());
                }
                publisher.publish(AgentEventEnvelope::new(AgentEvent::ApprovalRequested {
                    thread_id: context.thread_id.clone(),
                    turn_id: context.turn_id.clone(),
                    request,
                }));
                let resolution = if auto_approve {
                    ApprovalResolution {
                        action: ApprovalAction::Approved,
                        patch: None,
                        selected_paths: Vec::new(),
                        expected_hashes: Vec::new(),
                    }
                } else {
                    publisher.publish(AgentEventEnvelope::new(AgentEvent::ActivityStatusChanged {
                        thread_id: context.thread_id.clone(),
                        turn_id: context.turn_id.clone(),
                        status: AgentActivityStatus::AwaitingApproval,
                    }));
                    match self
                        .approvals
                        .wait(
                            &request_id,
                            receiver.expect("interactive approvals register a receiver"),
                            cancellation.clone(),
                        )
                        .await
                    {
                        Ok(resolution) => resolution,
                        Err(ApprovalError::Cancelled) => ApprovalResolution {
                            action: ApprovalAction::Cancelled,
                            patch: None,
                            selected_paths: Vec::new(),
                            expected_hashes: Vec::new(),
                        },
                        Err(_error) => ApprovalResolution {
                            action: ApprovalAction::Rejected,
                            patch: None,
                            selected_paths: Vec::new(),
                            expected_hashes: Vec::new(),
                        },
                    }
                };
                self.persist_approval_resolution(context, &request_id, &resolution, publisher)
                    .await?;

                match resolution.action {
                    ApprovalAction::Rejected => {
                        return Ok(Some(failure_result(
                            "approval_rejected: user rejected the proposed change".to_string(),
                        )));
                    }
                    ApprovalAction::TimedOut => {
                        return Ok(Some(failure_result(
                            "approval_timed_out: proposed change was not approved before expiry"
                                .to_string(),
                        )));
                    }
                    ApprovalAction::Cancelled => return Ok(None),
                    ApprovalAction::Approved => {}
                }

                let Some(preview) = preview else {
                    if resolution.patch.is_some()
                        || !resolution.selected_paths.is_empty()
                        || !resolution.expected_hashes.is_empty()
                    {
                        return Ok(Some(failure_result(
                            "approval_invalid: external tool approval cannot contain patch scope"
                                .to_string(),
                        )));
                    }
                    return Ok(Some(
                        match self
                            .tools
                            .dispatch_authorized(
                                context,
                                &call.name,
                                call.arguments.clone(),
                                cancellation,
                            )
                            .await
                        {
                            Ok(result) => result,
                            Err(ToolError::Cancelled) => return Ok(None),
                            Err(error) => failure_result(error.to_string()),
                        },
                    ));
                };

                if resolution.patch.is_some() && call.name != "apply_patch" {
                    return Ok(Some(failure_result(
                        "approval_invalid: this tool does not accept an edited patch".to_string(),
                    )));
                }
                let patch_was_edited = resolution
                    .patch
                    .as_ref()
                    .is_some_and(|patch| patch != &preview.patch);
                let approved_preview = if patch_was_edited {
                    let mut edited_arguments = call.arguments.clone();
                    let Some(arguments) = edited_arguments.as_object_mut() else {
                        return Ok(Some(failure_result(
                            "approval_invalid: tool arguments are not an object".to_string(),
                        )));
                    };
                    arguments.insert(
                        "patch".to_string(),
                        Value::String(resolution.patch.clone().unwrap_or_default()),
                    );
                    match self.tools.preview(context, &call.name, &edited_arguments) {
                        Ok(Some(preview)) => preview,
                        Ok(None) => {
                            return Ok(Some(failure_result(
                                "approval_invalid: edited patch has no reviewable preview"
                                    .to_string(),
                            )));
                        }
                        Err(error) => return Ok(Some(failure_result(error.to_string()))),
                    }
                } else {
                    preview.clone()
                };
                let selected_paths = if resolution.selected_paths.is_empty() {
                    approved_preview
                        .files
                        .iter()
                        .map(|file| file.path.clone())
                        .collect()
                } else {
                    resolution.selected_paths.clone()
                };
                let expected_hashes = if resolution.expected_hashes.is_empty() {
                    if patch_was_edited {
                        return Ok(Some(failure_result(
                            "approval_invalid: edited patch is missing reviewed file hashes"
                                .to_string(),
                        )));
                    }
                    preview_hashes(&approved_preview)
                } else {
                    resolution.expected_hashes.clone()
                };
                if let Err(message) =
                    validate_approval_scope(&approved_preview, &selected_paths, &expected_hashes)
                {
                    return Ok(Some(failure_result(format!("approval_invalid: {message}"))));
                }
                let mut approved_context = context.clone();
                approved_context.approval = Some(ApprovedToolExecution {
                    patch: resolution.patch.clone(),
                    selected_paths,
                    expected_hashes,
                });
                let result = match self
                    .tools
                    .dispatch_authorized(
                        &approved_context,
                        &call.name,
                        call.arguments.clone(),
                        cancellation,
                    )
                    .await
                {
                    Ok(result) => result,
                    Err(ToolError::Cancelled) => return Ok(None),
                    Err(error) => failure_result(error.to_string()),
                };
                if let Some(change_set) = change_set_from_result(&result) {
                    let change_item_id = change_set.id.clone();
                    self.start_item(
                        &context.thread_id,
                        &context.turn_id,
                        &change_item_id,
                        AgentItemType::Change,
                        publisher,
                    )
                    .await?;
                    if let Err(error) = self
                        .repository
                        .append(StoredEvent::new(
                            &context.thread_id,
                            Some(context.turn_id.clone()),
                            StoredEventKind::ChangeApplied {
                                change_set: change_set.clone(),
                            },
                        ))
                        .await
                    {
                        let storage_error = error.to_string();
                        if let Err(rollback_error) = self
                            .tools
                            .rollback_change(context.workspace_root.clone(), change_set.clone())
                            .await
                        {
                            let _ = self
                                .complete_item(
                                    &context.thread_id,
                                    &context.turn_id,
                                    &change_item_id,
                                    AgentItemType::Change,
                                    AgentItemStatus::Failed,
                                    publisher,
                                )
                                .await;
                            return Err(AgentRuntimeError::AuditCompensation {
                                storage_error,
                                rollback_error: rollback_error.to_string(),
                            });
                        }
                        self.complete_item(
                            &context.thread_id,
                            &context.turn_id,
                            &change_item_id,
                            AgentItemType::Change,
                            AgentItemStatus::Failed,
                            publisher,
                        )
                        .await?;
                        return Err(error.into());
                    }
                    publisher.publish(AgentEventEnvelope::new(AgentEvent::ChangeApplied {
                        thread_id: context.thread_id.clone(),
                        turn_id: context.turn_id.clone(),
                        change_set,
                    }));
                    self.complete_item(
                        &context.thread_id,
                        &context.turn_id,
                        &change_item_id,
                        AgentItemType::Change,
                        AgentItemStatus::Completed,
                        publisher,
                    )
                    .await?;
                }
                Ok(Some(result))
            }
        }
    }

    async fn request_turn_continuation(
        &self,
        thread_id: &str,
        turn_id: &str,
        usage: SoftTurnSegmentUsage,
        limits: SoftTurnLimits,
        cancellation: CancellationToken,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnContinuationDecision, AgentRuntimeError> {
        let request_id = Uuid::new_v4().to_string();
        let created_at_ms = now_ms();
        let time_allowance = limits
            .duration_ms
            .map(|duration_ms| format!(" / {} 秒", duration_ms.div_ceil(1_000)))
            .unwrap_or_default();
        let request = UserInputRequest {
            id: request_id,
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            tool_call_id: TURN_CONTINUATION_TOOL_CALL_ID.to_string(),
            kind: UserInputRequestKind::TurnContinuation,
            questions: vec![UserInputQuestion {
                question: format!(
                    "当前执行段已调用模型 {} 次、累计消耗 {} tokens、运行 {} 秒。继续后会获得新一段额度（{} 次调用 / {} tokens{}）。如需继续，请发送“继续”（点击“继续执行”即可）；也可以选择“压缩后继续”或“停止执行”。",
                    usage.provider_calls,
                    usage.total_tokens,
                    usage.duration_ms.div_ceil(1_000),
                    limits.provider_calls,
                    limits.total_tokens,
                    time_allowance,
                ),
                options: vec![
                    TURN_CONTINUE.to_string(),
                    TURN_COMPACT_AND_CONTINUE.to_string(),
                    TURN_STOP.to_string(),
                ],
            }],
            created_at_ms,
            expires_at_ms: None,
        };
        let resolution = self
            .await_user_input(request, cancellation, publisher)
            .await?;
        if resolution.action != UserInputAction::Answered {
            return Ok(TurnContinuationDecision::Stop);
        }
        Ok(
            match resolution
                .answers
                .first()
                .map(|answer| answer.answer.as_str())
            {
                Some(TURN_CONTINUE) => TurnContinuationDecision::Continue,
                Some(TURN_COMPACT_AND_CONTINUE) => TurnContinuationDecision::CompactAndContinue,
                _ => TurnContinuationDecision::Stop,
            },
        )
    }

    async fn await_user_input(
        &self,
        request: UserInputRequest,
        cancellation: CancellationToken,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<UserInputResolution, AgentRuntimeError> {
        let request_id = request.id.clone();
        let receiver = self.user_inputs.register(&request_id).await?;
        if let Err(error) = self
            .start_item(
                &request.thread_id,
                &request.turn_id,
                &request_id,
                AgentItemType::UserInput,
                publisher,
            )
            .await
        {
            self.user_inputs.discard(&request_id).await;
            return Err(error);
        }
        if let Err(error) = self
            .repository
            .append(StoredEvent::new(
                &request.thread_id,
                Some(request.turn_id.clone()),
                StoredEventKind::UserInputRequested {
                    request: request.clone(),
                },
            ))
            .await
        {
            self.user_inputs.discard(&request_id).await;
            return Err(error.into());
        }
        publisher.publish(AgentEventEnvelope::new(AgentEvent::UserInputRequested {
            thread_id: request.thread_id.clone(),
            turn_id: request.turn_id.clone(),
            request: request.clone(),
        }));
        let resolution = match self
            .user_inputs
            .wait(&request_id, receiver, cancellation)
            .await
        {
            Ok(resolution) => resolution,
            Err(UserInputError::Cancelled) => UserInputResolution {
                action: UserInputAction::Cancelled,
                answers: Vec::new(),
            },
            Err(error) => return Err(error.into()),
        };
        self.persist_user_input_resolution(
            &request.thread_id,
            &request.turn_id,
            &request_id,
            &resolution,
            publisher,
        )
        .await?;
        Ok(resolution)
    }

    /// 执行 `request_user_input`：向前端发起提问并阻塞等待回答。
    async fn execute_request_user_input(
        &self,
        context: &ToolContext,
        call: &ToolCall,
        cancellation: CancellationToken,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<Option<ToolResult>, AgentRuntimeError> {
        let args = match RequestUserInputTool::parse_arguments(&call.arguments) {
            Ok(args) => args,
            Err(error) => return Ok(Some(failure_result(error.to_string()))),
        };
        let request_id = Uuid::new_v4().to_string();
        let created_at_ms = now_ms();
        let request = UserInputRequest {
            id: request_id.clone(),
            thread_id: context.thread_id.clone(),
            turn_id: context.turn_id.clone(),
            tool_call_id: call.id.clone(),
            kind: UserInputRequestKind::ModelQuestion,
            questions: args
                .questions
                .iter()
                .map(|q| crate::protocol::UserInputQuestion {
                    question: q.question.clone(),
                    options: q.options.clone(),
                })
                .collect(),
            created_at_ms,
            expires_at_ms: None,
        };
        let resolution = match self
            .await_user_input(request, cancellation, publisher)
            .await
        {
            Ok(resolution) => resolution,
            Err(AgentRuntimeError::UserInput(error)) => {
                return Ok(Some(failure_result(error.to_string())));
            }
            Err(error) => return Err(error),
        };
        match resolution.action {
            UserInputAction::Answered => {
                let summary = resolution
                    .answers
                    .iter()
                    .map(|a| format!("Q: {}\nA: {}", a.question, a.answer))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                Ok(Some(ToolResult {
                    success: true,
                    output: summary,
                    metadata: serde_json::to_value(&resolution.answers)
                        .unwrap_or(serde_json::Value::Null),
                }))
            }
            UserInputAction::Skipped => Ok(Some(failure_result(
                "user_skipped: the user skipped the questions".to_string(),
            ))),
            UserInputAction::Cancelled => Ok(None),
        }
    }

    /// 执行 `todo_write`：更新任务清单并发送事件
    async fn execute_todo_write(
        &self,
        context: &ToolContext,
        call: &ToolCall,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<Option<ToolResult>, AgentRuntimeError> {
        use crate::advanced::TodoWriteArgs;

        let args = match TodoWriteArgs::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => return Ok(Some(failure_result(error))),
        };

        let todos = args.to_todo_items();

        self.repository
            .append(StoredEvent::new(
                &context.thread_id,
                Some(context.turn_id.clone()),
                StoredEventKind::TodoUpdated {
                    todos: todos.clone(),
                },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TodoUpdated {
            thread_id: context.thread_id.clone(),
            turn_id: context.turn_id.clone(),
            todos,
        }));

        Ok(Some(ToolResult {
            success: true,
            output: "Task list updated successfully.".to_string(),
            metadata: serde_json::Value::Null,
        }))
    }

    async fn persist_approval_resolution(
        &self,
        context: &ToolContext,
        request_id: &str,
        resolution: &ApprovalResolution,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        self.repository
            .append(StoredEvent::new(
                &context.thread_id,
                Some(context.turn_id.clone()),
                StoredEventKind::ApprovalResolved {
                    request_id: request_id.to_string(),
                    resolution: resolution.clone(),
                },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::ApprovalResolved {
            thread_id: context.thread_id.clone(),
            turn_id: context.turn_id.clone(),
            request_id: request_id.to_string(),
            resolution: resolution.clone(),
        }));
        self.complete_item(
            &context.thread_id,
            &context.turn_id,
            request_id,
            AgentItemType::Approval,
            match resolution.action {
                ApprovalAction::Approved => AgentItemStatus::Completed,
                ApprovalAction::Rejected | ApprovalAction::TimedOut => AgentItemStatus::Failed,
                ApprovalAction::Cancelled => AgentItemStatus::Cancelled,
            },
            publisher,
        )
        .await?;
        Ok(())
    }

    async fn persist_user_input_resolution(
        &self,
        thread_id: &str,
        turn_id: &str,
        request_id: &str,
        resolution: &crate::protocol::UserInputResolution,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::UserInputResolved {
                    request_id: request_id.to_string(),
                    resolution: resolution.clone(),
                },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::UserInputResolved {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            request_id: request_id.to_string(),
            resolution: resolution.clone(),
        }));
        self.complete_item(
            thread_id,
            turn_id,
            request_id,
            AgentItemType::UserInput,
            match resolution.action {
                UserInputAction::Answered => AgentItemStatus::Completed,
                UserInputAction::Skipped => AgentItemStatus::Failed,
                UserInputAction::Cancelled => AgentItemStatus::Cancelled,
            },
            publisher,
        )
        .await?;
        Ok(())
    }

    async fn persist_tool_result(
        &self,
        thread_id: &str,
        turn_id: &str,
        call: &ToolCall,
        result: &ToolResult,
        status: AgentItemStatus,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        if !result.success {
            self.record_tool_failure(thread_id, turn_id, call, result, status);
        }
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::ToolResult {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    result: result.clone(),
                },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::ToolCompleted {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            call_id: call.id.clone(),
            name: call.name.clone(),
            result: result.clone(),
        }));
        self.complete_item(
            thread_id,
            turn_id,
            &call.id,
            AgentItemType::Tool,
            status,
            publisher,
        )
        .await?;
        Ok(())
    }

    /// 工具失败埋点：所有失败（含被跳过、被取消和致命中止）都会写入本地运行日志，
    /// 并按日志原始 `threadId` 标记来源对话。
    fn record_tool_failure(
        &self,
        thread_id: &str,
        turn_id: &str,
        call: &ToolCall,
        result: &ToolResult,
        status: AgentItemStatus,
    ) {
        let Some(logger) = &self.logger else {
            return;
        };
        let _ = logger.log(
            "error",
            "tool_failed",
            json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "tool": call.name,
                "callId": call.id,
                "itemStatus": status,
                // 事后查因要能判定是哪一条指令失败，所以失败调用的参数本体一并记录；
                // 日志 compact 会按预算截断，敏感字段另有 redact 兜底。
                "arguments": call.arguments,
                "output": truncate_utf8(&result.output, MAX_TOOL_FAILURE_OUTPUT_BYTES),
            }),
        );
    }

    async fn finish_completed(
        &self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        text: String,
        images: Vec<ContentBlock>,
        usage: Option<TokenUsage>,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        let message = assistant_message_with_content(item_id.to_string(), text, images);
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::AssistantMessage {
                    message: message.clone(),
                },
            ))
            .await?;
        self.complete_active_non_message_items(
            thread_id,
            turn_id,
            AgentItemStatus::Failed,
            publisher,
        )
        .await?;
        self.complete_active_items(
            thread_id,
            turn_id,
            AgentItemType::AgentMessage,
            AgentItemStatus::Completed,
            publisher,
        )
        .await?;
        let timing = self
            .append_terminal_event(thread_id, turn_id, StoredEventKind::TurnCompleted { usage })
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnCompleted {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            message,
            usage,
            started_at_ms: timing.started_at_ms,
            completed_at_ms: timing.completed_at_ms,
            duration_ms: timing.duration_ms,
        }));
        if let Some(metrics) = &self.metrics {
            metrics.task(true);
        }
        Ok(outcome(
            thread_id,
            turn_id,
            TurnState::Completed,
            None,
            timing,
        ))
    }

    /// 当前模型没有多模态能力时，直接以助手消息回复提示，不发起 Provider 请求。
    /// 用户消息里的图片已随 UserMessage 事件落库，历史中仍然可见。
    async fn finish_vision_unsupported(
        &self,
        thread_id: &str,
        turn_id: &str,
        model: &str,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        let item_id = Uuid::new_v4().to_string();
        self.start_item(
            thread_id,
            turn_id,
            &item_id,
            AgentItemType::AgentMessage,
            publisher,
        )
        .await?;
        let message = assistant_message_with_content(
            item_id.clone(),
            vision_unsupported_message(model),
            Vec::new(),
        );
        publisher.publish(AgentEventEnvelope::new(AgentEvent::ItemStarted {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            item_id: item_id.clone(),
            item_type: AgentItemType::AgentMessage,
        }));
        self.complete_item(
            thread_id,
            turn_id,
            &item_id,
            AgentItemType::AgentMessage,
            AgentItemStatus::Completed,
            publisher,
        )
        .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::ItemCompleted {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            item_id: item_id.clone(),
            item_type: AgentItemType::AgentMessage,
            status: AgentItemStatus::Completed,
        }));
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::AssistantMessage {
                    message: message.clone(),
                },
            ))
            .await?;
        let timing = self
            .append_terminal_event(
                thread_id,
                turn_id,
                StoredEventKind::TurnCompleted { usage: None },
            )
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnCompleted {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            message,
            usage: None,
            started_at_ms: timing.started_at_ms,
            completed_at_ms: timing.completed_at_ms,
            duration_ms: timing.duration_ms,
        }));
        Ok(outcome(
            thread_id,
            turn_id,
            TurnState::Completed,
            None,
            timing,
        ))
    }

    async fn finish_failed(
        &self,
        thread_id: &str,
        turn_id: &str,
        message: String,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        self.finish_failed_with_error(thread_id, turn_id, TurnError::classify(message), publisher)
            .await
    }

    async fn finish_failed_with_error(
        &self,
        thread_id: &str,
        turn_id: &str,
        error: TurnError,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        let message = error.message.clone();
        if let Some(logger) = &self.logger {
            let _ = logger.log(
                "error",
                "turn_failed",
                serde_json::json!({
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "message": message,
                }),
            );
        }
        self.complete_active_non_message_items(
            thread_id,
            turn_id,
            AgentItemStatus::Failed,
            publisher,
        )
        .await?;
        self.complete_active_items(
            thread_id,
            turn_id,
            AgentItemType::AgentMessage,
            AgentItemStatus::Failed,
            publisher,
        )
        .await?;
        let timing = self
            .append_terminal_event(
                thread_id,
                turn_id,
                StoredEventKind::TurnFailed {
                    message: message.clone(),
                    error: Some(error.clone()),
                },
            )
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnFailed {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            message: message.clone(),
            error: Some(error),
            started_at_ms: timing.started_at_ms,
            completed_at_ms: timing.completed_at_ms,
            duration_ms: timing.duration_ms,
        }));
        if let Some(metrics) = &self.metrics {
            metrics.task(false);
        }
        Ok(outcome(
            thread_id,
            turn_id,
            TurnState::Failed,
            Some(message),
            timing,
        ))
    }

    async fn finish_cancelled(
        &self,
        thread_id: &str,
        turn_id: &str,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        self.complete_active_non_message_items(
            thread_id,
            turn_id,
            AgentItemStatus::Cancelled,
            publisher,
        )
        .await?;
        self.complete_active_items(
            thread_id,
            turn_id,
            AgentItemType::AgentMessage,
            AgentItemStatus::Cancelled,
            publisher,
        )
        .await?;
        let timing = self
            .append_terminal_event(thread_id, turn_id, StoredEventKind::TurnCancelled)
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnCancelled {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            started_at_ms: timing.started_at_ms,
            completed_at_ms: timing.completed_at_ms,
            duration_ms: timing.duration_ms,
        }));
        if let Some(metrics) = &self.metrics {
            metrics.task(false);
        }
        Ok(outcome(
            thread_id,
            turn_id,
            TurnState::Cancelled,
            None,
            timing,
        ))
    }

    async fn start_item(
        &self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        item_type: AgentItemType,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::ItemStarted {
                    item_id: item_id.to_string(),
                    item_type,
                },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::ItemStarted {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            item_id: item_id.to_string(),
            item_type,
        }));
        Ok(())
    }

    async fn complete_item(
        &self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        item_type: AgentItemType,
        status: AgentItemStatus,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::ItemCompleted {
                    item_id: item_id.to_string(),
                    item_type,
                    status,
                },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::ItemCompleted {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            item_id: item_id.to_string(),
            item_type,
            status,
        }));
        Ok(())
    }

    async fn complete_active_items(
        &self,
        thread_id: &str,
        turn_id: &str,
        item_type: AgentItemType,
        status: AgentItemStatus,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        let events = self.repository.load(thread_id).await?;
        let mut active_item_ids = Vec::<String>::new();
        for event in &events {
            if event.turn_id.as_deref() != Some(turn_id) {
                continue;
            }
            match &event.kind {
                StoredEventKind::ItemStarted {
                    item_id: started_item_id,
                    item_type: started_type,
                } if *started_type == item_type && !active_item_ids.contains(started_item_id) => {
                    active_item_ids.push(started_item_id.clone());
                }
                StoredEventKind::ItemCompleted {
                    item_id: completed_item_id,
                    item_type: completed_type,
                    ..
                } if *completed_type == item_type => {
                    active_item_ids.retain(|item_id| item_id != completed_item_id);
                }
                _ => {}
            }
        }
        for item_id in active_item_ids {
            self.complete_item(thread_id, turn_id, &item_id, item_type, status, publisher)
                .await?;
        }
        Ok(())
    }

    async fn complete_active_non_message_items(
        &self,
        thread_id: &str,
        turn_id: &str,
        status: AgentItemStatus,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        for item_type in [
            AgentItemType::Reasoning,
            AgentItemType::Tool,
            AgentItemType::Approval,
            AgentItemType::Change,
            AgentItemType::ContextCompaction,
            AgentItemType::UserInput,
        ] {
            self.complete_active_items(thread_id, turn_id, item_type, status, publisher)
                .await?;
        }
        Ok(())
    }

    async fn append_terminal_event(
        &self,
        thread_id: &str,
        turn_id: &str,
        kind: StoredEventKind,
    ) -> Result<TurnTiming, AgentRuntimeError> {
        let started_at_ms = self
            .repository
            .load(thread_id)
            .await?
            .into_iter()
            .find(|event| {
                event.turn_id.as_deref() == Some(turn_id)
                    && matches!(event.kind, StoredEventKind::TurnStarted)
            })
            .map(|event| event.created_at_ms)
            .unwrap_or_else(now_ms);
        let terminal = StoredEvent::new(thread_id, Some(turn_id.to_string()), kind);
        let completed_at_ms = terminal.created_at_ms;
        self.repository.append(terminal).await?;
        Ok(TurnTiming {
            started_at_ms,
            completed_at_ms,
            duration_ms: completed_at_ms.saturating_sub(started_at_ms),
        })
    }

    async fn persist_provider_usage(
        &self,
        thread_id: &str,
        turn_id: &str,
        call_index: u32,
        usage: TokenUsage,
        details: TokenUsageDetails,
        provider: Option<&str>,
        model: Option<&str>,
        total_usage: &mut TokenUsage,
        has_usage: &mut bool,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        *total_usage = add_usage(*total_usage, usage);
        *has_usage = true;
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(turn_id.to_string()),
                StoredEventKind::ProviderCallUsage {
                    call_index,
                    usage,
                    details,
                    provider: provider.map(str::to_string),
                    model: model.map(str::to_string),
                },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::UsageUpdated {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            usage: *total_usage,
            context_usage: usage,
        }));
        Ok(())
    }

    fn record_provider_metric(
        &self,
        started: std::time::Instant,
        success: bool,
        usage: Option<TokenUsage>,
    ) {
        if let Some(metrics) = &self.metrics {
            let usage = usage.unwrap_or_default();
            metrics.provider(
                started.elapsed().as_millis() as u64,
                success,
                usage.input_tokens,
                usage.output_tokens,
            );
        }
    }

    fn record_rate_limit_retry(
        &self,
        thread_id: &str,
        turn_id: &str,
        error: &ProviderError,
        delay: Duration,
        retry_number: usize,
    ) {
        if let Some(logger) = &self.logger {
            // Provider adapters already strip their credential. Apply the shared
            // secret-token filter too, and never log request bodies or headers.
            let message: String = crate::execution::redact(&error.to_string())
                .chars()
                .take(1024)
                .collect();
            let _ = logger.log("info", "provider_rate_limited", json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "message": message,
                "retryNumber": retry_number,
                "retryDelayMs": delay.as_millis().min(u64::MAX as u128) as u64,
                "delaySource": if matches!(error, ProviderError::RateLimited { retry_after: Some(_), .. }) {
                    "retry_after"
                } else {
                    "default"
                },
            }));
        }
    }

    /// Provider/模型请求失败埋点。只针对真实 HTTP 错误（4xx/5xx），网络抖动和
    /// 用户取消不写日志：前者是诊断线索，后者是主动行为不是异常。
    ///
    /// 与 `provider_rate_limited` 分工不同：那条在自动重试前写，可能重试后成功；
    /// 这条在错误真正终止 Turn 时写，是稳定的失败事实，事后可据此查因。
    fn record_http_failure(
        &self,
        thread_id: &str,
        turn_id: &str,
        error: &ProviderError,
        retryable: bool,
    ) {
        let Some(status) = error.http_status() else {
            return;
        };
        let Some(logger) = &self.logger else {
            return;
        };
        // Provider adapters already strip their credential. Apply the shared
        // secret-token filter too, and never log request bodies or headers.
        let message: String = crate::execution::redact(&error.to_string())
            .chars()
            .take(1024)
            .collect();
        let _ = logger.log(
            "error",
            "provider_http_failed",
            json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "status": status,
                "message": message,
                // 4xx/5xx 里只有一小部分可自动重试，用来区分「客户端参数问题」与「服务端可恢复」。
                "retryable": retryable,
                "retryAfterMs": error.rate_limit_delay().map(|delay| delay.as_millis().min(u64::MAX as u128) as u64),
            }),
        );
    }

    fn record_provider_retry(&self) {
        if let Some(metrics) = &self.metrics {
            metrics.retry();
        }
    }
}

fn is_retryable_protocol_stream_error(error: &ProviderError) -> bool {
    if matches!(error, ProviderError::InvalidToolArguments(_)) {
        return true;
    }
    let ProviderError::InvalidResponse(message) = error else {
        return false;
    };
    let message = message.to_ascii_lowercase();
    message.contains("incomplete tool call")
        || message.contains("invalid json arguments")
        || message.contains("returned invalid json")
}

fn protocol_retry_delay(retry_count: usize) -> Duration {
    let multiplier = 1u64 << retry_count.min(4);
    Duration::from_millis((200 * multiplier).min(4_000))
}

async fn wait_for_provider_retry(delay: Duration, cancellation: &CancellationToken) -> bool {
    if cancellation.is_cancelled() {
        return false;
    }
    tokio::select! {
        _ = cancellation.cancelled() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

fn preview_hashes(preview: &PatchPreview) -> Vec<ExpectedFileHash> {
    preview
        .files
        .iter()
        .map(|file| ExpectedFileHash {
            path: file.path.clone(),
            before_hash: file.before_hash.clone(),
        })
        .collect()
}

fn validate_approval_scope(
    preview: &PatchPreview,
    selected_paths: &[String],
    expected_hashes: &[ExpectedFileHash],
) -> Result<(), String> {
    let available = preview
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.before_hash.as_ref()))
        .collect::<HashMap<_, _>>();
    let selected = selected_paths
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if selected.len() != selected_paths.len() {
        return Err("selected file paths contain duplicates".to_string());
    }
    if selected.is_empty() {
        return Err("at least one reviewed file must be selected".to_string());
    }
    if let Some(path) = selected.iter().find(|path| !available.contains_key(**path)) {
        return Err(format!(
            "selected file was not in the reviewed preview: {path}"
        ));
    }
    let mut expected = HashMap::new();
    for item in expected_hashes {
        if expected
            .insert(item.path.as_str(), item.before_hash.as_ref())
            .is_some()
        {
            return Err(format!(
                "file hash was provided more than once: {}",
                item.path
            ));
        }
    }
    if expected.len() != selected.len() || expected.keys().any(|path| !selected.contains(path)) {
        return Err("reviewed file hashes do not match the selected files".to_string());
    }
    for path in selected {
        if expected.get(path).copied() != available.get(path).copied() {
            return Err(format!(
                "reviewed file hash does not match the preview: {path}"
            ));
        }
    }
    Ok(())
}

fn change_set_from_result(result: &ToolResult) -> Option<ChangeSet> {
    result
        .metadata
        .get("changeSet")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}

fn publish_tool_progress(
    publisher: &Arc<dyn EventPublisher>,
    thread_id: &str,
    turn_id: &str,
    call_id: &str,
    progress: ToolProgress,
) {
    publisher.publish(AgentEventEnvelope::new(AgentEvent::ToolOutputDelta {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
        call_id: call_id.to_string(),
        stream: progress.stream,
        cursor: progress.cursor,
        delta: progress.delta,
    }));
}

fn call_signature(call: &ToolCall) -> String {
    format!("{}:{}", call.name, canonical_json(&call.arguments))
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_default(),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn is_pending_subagent_wait(call: &ToolCall, result: &ToolResult, elapsed: Duration) -> bool {
    if call.name != "wait_agent" || !result.success || elapsed < Duration::from_secs(1) {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(&result.output) else {
        return false;
    };
    let active = |agent: &Value| {
        matches!(
            agent["state"].as_str(),
            Some("queued" | "running" | "blocked")
        )
    };
    if call.arguments.get("agentIds").is_some() {
        value["timedOut"] == true
            && value["agents"]
                .as_array()
                .is_some_and(|agents| !agents.is_empty() && agents.iter().all(active))
    } else {
        active(&value)
    }
}

fn bound_tool_result(mut result: ToolResult) -> ToolResult {
    if result.output.len() <= MAX_TOOL_OUTPUT_BYTES {
        return result;
    }
    let original_bytes = result.output.len();
    let marker = format!(
        "\n...[tool output truncated: omitted {} bytes]...\n",
        original_bytes.saturating_sub(MAX_TOOL_OUTPUT_BYTES)
    );
    let available = MAX_TOOL_OUTPUT_BYTES.saturating_sub(marker.len());
    let head_budget = available / 2;
    let tail_budget = available.saturating_sub(head_budget);
    let mut head_end = head_budget.min(result.output.len());
    while head_end > 0 && !result.output.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = result.output.len().saturating_sub(tail_budget);
    while tail_start < result.output.len() && !result.output.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let original = std::mem::take(&mut result.output);
    result.output = format!(
        "{}{}{}",
        &original[..head_end],
        marker,
        &original[tail_start..]
    );
    if !result.metadata.is_object() {
        result.metadata = json!({});
    }
    result.metadata["retainedOutputRanges"] = json!([[0, head_end], [tail_start, original_bytes]]);
    result.metadata["outputTruncated"] = Value::Bool(true);
    result.metadata["originalOutputBytes"] = Value::from(original_bytes as u64);
    result.metadata["omittedOutputBytes"] =
        Value::from(original_bytes.saturating_sub(result.output.len()) as u64);
    result
}

fn failure_result(message: String) -> ToolResult {
    ToolResult {
        success: false,
        output: message,
        metadata: json!({ "error": true }),
    }
}

fn add_usage(left: TokenUsage, right: TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: left.input_tokens.saturating_add(right.input_tokens),
        output_tokens: left.output_tokens.saturating_add(right.output_tokens),
        total_tokens: left.total_tokens.saturating_add(right.total_tokens),
    }
}

#[cfg(test)]
fn text_message(role: MessageRole, text: String) -> ChatMessage {
    text_message_with_id(role, Uuid::new_v4().to_string(), text)
}

#[cfg(test)]
fn text_message_with_id(role: MessageRole, id: String, text: String) -> ChatMessage {
    ChatMessage {
        schema_version: PROTOCOL_VERSION,
        id,
        role,
        content: vec![ContentBlock::Text { text }],
        created_at_ms: now_ms(),
    }
}

fn assistant_message_with_content(
    id: String,
    text: String,
    images: Vec<ContentBlock>,
) -> ChatMessage {
    let mut content = Vec::with_capacity(1 + images.len());
    if !text.is_empty() {
        content.push(ContentBlock::Text { text });
    }
    content.extend(images);
    ChatMessage {
        schema_version: PROTOCOL_VERSION,
        id,
        role: MessageRole::Assistant,
        content,
        created_at_ms: now_ms(),
    }
}

/// 当前模型没有多模态能力时的回复文案。图片识别已全部交给模型，
/// 这里只说明现状与可选动作，不再提供本地 OCR 兜底。
fn vision_unsupported_message(model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        "当前模型不支持图片识别：请改用支持多模态的模型，或用文字描述图片内容。".to_string()
    } else {
        format!("当前模型（{model}）不支持图片识别：请改用支持多模态的模型，或用文字描述图片内容。")
    }
}

fn outcome(
    thread_id: &str,
    turn_id: &str,
    state: TurnState,
    error: Option<String>,
    timing: TurnTiming,
) -> TurnOutcome {
    TurnOutcome {
        schema_version: PROTOCOL_VERSION,
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
        state,
        error,
        started_at_ms: timing.started_at_ms,
        completed_at_ms: timing.completed_at_ms,
        duration_ms: timing.duration_ms,
    }
}

#[cfg(test)]
mod tests {
    mod read_observation_regressions {
        include!("read_observation_tests.rs");
    }
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;

    use super::*;
    use crate::protocol::{ToolDefinition, ToolRisk, UserInputAnswer};
    use crate::providers::testing::FakeProvider;
    use crate::storage::{JsonlThreadRepository, TurnTimelineItem};
    use crate::tools::ToolHandler;

    #[test]
    fn validates_and_maps_bounded_image_attachments() {
        let vision_message = user_message(
            "inspect this screenshot".into(),
            vec![ImageAttachment {
                name: "screen.png".into(),
                data_url: "data:image/png;base64,iVBORw0KGgo=".into(),
            }],
            true,
        )
        .unwrap();
        assert!(matches!(
            &vision_message.content[1],
            ContentBlock::Image { name, .. } if name == "screen.png"
        ));
        assert!(matches!(
            chat_to_provider(vision_message, true),
            Some(ProviderMessage::UserContent { text, images })
                if text == "inspect this screenshot" && images.len() == 1
        ));
        assert!(
            user_message(
                "bad image".into(),
                vec![ImageAttachment {
                    name: "bad.svg".into(),
                    data_url: "data:image/svg+xml;base64,PHN2Zy8+".into(),
                }],
                true,
            )
            .is_err()
        );

        let image_only = user_message(
            String::new(),
            vec![ImageAttachment {
                name: "only.png".into(),
                data_url: "data:image/png;base64,iVBORw0KGgo=".into(),
            }],
            true,
        )
        .unwrap();
        assert!(matches!(
            chat_to_provider(image_only, true),
            Some(ProviderMessage::UserContent { text, images })
                if text == "请分析用户提供的图片。" && images.len() == 1
        ));
    }

    #[derive(Default)]
    struct RecordingPublisher {
        events: Mutex<Vec<AgentEventEnvelope>>,
    }

    impl EventPublisher for RecordingPublisher {
        fn publish(&self, event: AgentEventEnvelope) {
            self.events.lock().unwrap().push(event);
        }
    }

    struct PreStreamProvider {
        outcomes: Mutex<VecDeque<Result<Vec<Result<ProviderEvent, ProviderError>>, ProviderError>>>,
        requests: Mutex<Vec<ProviderRequest>>,
    }

    impl PreStreamProvider {
        fn new(
            outcomes: Vec<Result<Vec<Result<ProviderEvent, ProviderError>>, ProviderError>>,
        ) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<ProviderRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Provider for PreStreamProvider {
        async fn stream(
            &self,
            request: ProviderRequest,
            cancellation: CancellationToken,
        ) -> Result<crate::providers::ProviderStream, ProviderError> {
            if cancellation.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            self.requests.lock().unwrap().push(request);
            match self.outcomes.lock().unwrap().pop_front() {
                Some(Ok(events)) => Ok(Box::pin(futures_util::stream::iter(events))),
                Some(Err(error)) => Err(error),
                None => Err(ProviderError::InvalidResponse(
                    "pre-stream test provider ran out of outcomes".into(),
                )),
            }
        }
    }

    struct CancellingPublisher {
        cancellation: CancellationToken,
        started_calls: Mutex<Vec<String>>,
        events: Mutex<Vec<AgentEventEnvelope>>,
    }

    impl EventPublisher for CancellingPublisher {
        fn publish(&self, event: AgentEventEnvelope) {
            if let AgentEvent::ToolStarted { call, .. } = &event.event {
                self.started_calls.lock().unwrap().push(call.id.clone());
                self.cancellation.cancel();
            }
            self.events.lock().unwrap().push(event);
        }
    }

    struct ReasoningCancellingPublisher {
        cancellation: CancellationToken,
        events: Mutex<Vec<AgentEventEnvelope>>,
    }

    impl EventPublisher for ReasoningCancellingPublisher {
        fn publish(&self, event: AgentEventEnvelope) {
            let should_cancel = matches!(&event.event, AgentEvent::ReasoningSummaryDelta { .. });
            self.events.lock().unwrap().push(event);
            if should_cancel {
                self.cancellation.cancel();
            }
        }
    }

    struct ResolvingPublisher {
        events: Mutex<Vec<AgentEventEnvelope>>,
        approvals: Arc<ApprovalManager>,
        resolution: ApprovalResolution,
        mutation: Option<Box<dyn Fn() + Send + Sync>>,
    }

    impl EventPublisher for ResolvingPublisher {
        fn publish(&self, event: AgentEventEnvelope) {
            if let AgentEvent::ApprovalRequested { request, .. } = &event.event {
                if let Some(mutation) = &self.mutation {
                    mutation();
                }
                let approvals = self.approvals.clone();
                let request_id = request.id.clone();
                let resolution = self.resolution.clone();
                tokio::spawn(async move {
                    approvals
                        .resolve(&request_id, resolution)
                        .await
                        .expect("approval should resolve");
                });
            }
            self.events.lock().unwrap().push(event);
        }
    }

    struct UserInputResolvingPublisher {
        events: Mutex<Vec<AgentEventEnvelope>>,
        user_inputs: Arc<UserInputManager>,
        answer: &'static str,
    }

    impl UserInputResolvingPublisher {
        fn new(user_inputs: Arc<UserInputManager>, answer: &'static str) -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                user_inputs,
                answer,
            }
        }
    }

    impl EventPublisher for UserInputResolvingPublisher {
        fn publish(&self, event: AgentEventEnvelope) {
            if let AgentEvent::UserInputRequested { request, .. } = &event.event
                && request.kind == UserInputRequestKind::TurnContinuation
            {
                let user_inputs = self.user_inputs.clone();
                let request_id = request.id.clone();
                let question = request.questions[0].question.clone();
                let answer = self.answer.to_string();
                tokio::spawn(async move {
                    user_inputs
                        .resolve(
                            &request_id,
                            UserInputResolution {
                                action: UserInputAction::Answered,
                                answers: vec![UserInputAnswer { question, answer }],
                            },
                        )
                        .await
                        .expect("turn continuation should resolve");
                });
            }
            self.events.lock().unwrap().push(event);
        }
    }

    struct RejectChangeAuditRepository {
        inner: Arc<JsonlThreadRepository>,
    }

    #[async_trait]
    impl ThreadRepository for RejectChangeAuditRepository {
        async fn append(&self, event: StoredEvent) -> Result<(), StorageError> {
            if matches!(&event.kind, StoredEventKind::ChangeApplied { .. }) {
                return Err(StorageError::Io(
                    "injected change audit failure".to_string(),
                ));
            }
            self.inner.append(event).await
        }

        async fn load(&self, thread_id: &str) -> Result<Vec<StoredEvent>, StorageError> {
            self.inner.load(thread_id).await
        }
    }

    struct SlowTool;

    struct DelayTool;

    struct ExternalTool;

    struct AssertFileTool;

    #[async_trait]
    impl ToolHandler for DelayTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "delay".to_string(),
                description: "Complete after a bounded test delay".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "delayMs": { "type": "integer", "minimum": 0, "maximum": 1000 },
                        "label": { "type": "string" }
                    },
                    "required": ["delayMs", "label"],
                    "additionalProperties": false
                }),
            }
        }

        async fn execute(
            &self,
            _context: &ToolContext,
            arguments: Value,
            _cancellation: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            let delay_ms = arguments
                .get("delayMs")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    ToolError::InvalidArguments("delayMs must be an integer".to_string())
                })?;
            let label = arguments
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::InvalidArguments("label must be a string".to_string()))?;
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            Ok(ToolResult {
                success: true,
                output: label.to_string(),
                metadata: json!({}),
            })
        }
    }

    #[async_trait]
    impl ToolHandler for AssertFileTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "assert_file".to_string(),
                description: "Check a workspace file against expected text".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "expected": { "type": "string" }
                    },
                    "required": ["path", "expected"],
                    "additionalProperties": false
                }),
            }
        }

        async fn execute(
            &self,
            context: &ToolContext,
            arguments: Value,
            _cancellation: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::InvalidArguments("path must be a string".to_string()))?;
            let expected = arguments
                .get("expected")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ToolError::InvalidArguments("expected must be a string".to_string())
                })?;
            let actual = std::fs::read_to_string(context.workspace_root.join(path))
                .map_err(|error| ToolError::Execution(error.to_string()))?;
            let success = actual == expected;
            Ok(ToolResult {
                success,
                output: if success {
                    "file check passed".to_string()
                } else {
                    format!("file check failed: expected {expected:?}, got {actual:?}")
                },
                metadata: json!({}),
            })
        }
    }

    #[async_trait]
    impl ToolHandler for ExternalTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "mcp__fixture__write".into(),
                description: "external write fixture".into(),
                input_schema: json!({ "type": "object", "additionalProperties": false }),
            }
        }

        async fn execute(
            &self,
            _context: &ToolContext,
            _arguments: Value,
            _cancellation: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                success: true,
                output: "external completed".into(),
                metadata: json!({ "mcp": true }),
            })
        }
    }

    #[async_trait]
    impl ToolHandler for SlowTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "slow_read".to_string(),
                description: "Test cancellation".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            }
        }

        async fn execute(
            &self,
            _context: &ToolContext,
            _arguments: Value,
            cancellation: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            tokio::select! {
                _ = cancellation.cancelled() => Err(ToolError::Cancelled),
                _ = tokio::time::sleep(Duration::from_secs(10)) => Ok(ToolResult {
                    success: true,
                    output: "late".to_string(),
                    metadata: json!({}),
                }),
            }
        }
    }

    async fn runtime_fixture() -> (
        tempfile::TempDir,
        Arc<JsonlThreadRepository>,
        AgentRuntime,
        String,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(directory.path()).unwrap());
        let thread = repository.create_thread().await.unwrap();
        let runtime = AgentRuntime::with_tools(
            repository.clone(),
            ToolRegistry::read_only(),
            directory.path().to_path_buf(),
        );
        (directory, repository, runtime, thread.id)
    }

    #[tokio::test]
    async fn runtime_uses_the_configured_model_context_limit() {
        let (_directory, _repository, runtime, _thread_id) = runtime_fixture().await;

        let configured = runtime.with_context_limit(128_000);
        assert_eq!(configured.context_limit, 128_000);
        assert_eq!(
            configured.working_context_limit,
            context::DEFAULT_WORKING_CONTEXT_LIMIT
        );
        assert_eq!(
            configured
                .with_working_context_limit(64_000)
                .working_context_limit,
            64_000
        );
    }

    #[test]
    fn only_blocking_pending_subagent_waits_relax_loop_guards() {
        let mut call = ToolCall {
            id: "wait".into(),
            name: "wait_agent".into(),
            arguments: json!({"agentIds": ["a"]}),
            metadata: json!({}),
        };
        let mut result = ToolResult {
            success: true,
            output: json!({"timedOut": true, "agents": [{"id": "a", "state": "running"}]})
                .to_string(),
            metadata: json!({}),
        };
        assert!(is_pending_subagent_wait(
            &call,
            &result,
            Duration::from_secs(30)
        ));
        assert!(!is_pending_subagent_wait(
            &call,
            &result,
            Duration::from_millis(999)
        ));
        result.success = false;
        assert!(!is_pending_subagent_wait(
            &call,
            &result,
            Duration::from_secs(30)
        ));
        result.success = true;
        for output in [
            json!({"timedOut": true, "agents": []}),
            json!({"timedOut": false, "agents": [{"state": "completed"}]}),
            json!({"timedOut": true, "agents": [{"state": "failed"}]}),
        ] {
            result.output = output.to_string();
            assert!(!is_pending_subagent_wait(
                &call,
                &result,
                Duration::from_secs(30)
            ));
        }
        call.arguments = json!({"agentId": "a"});
        result.output = json!({"state": "blocked"}).to_string();
        assert!(is_pending_subagent_wait(
            &call,
            &result,
            Duration::from_secs(30)
        ));
        result.output = "invalid JSON".into();
        assert!(!is_pending_subagent_wait(
            &call,
            &result,
            Duration::from_secs(30)
        ));
        result.output = json!({"state": "running"}).to_string();
        call.name = "run_command".into();
        assert!(!is_pending_subagent_wait(
            &call,
            &result,
            Duration::from_secs(30)
        ));
    }

    #[test]
    fn oversized_tool_results_keep_head_tail_and_metadata() {
        let original = format!("HEAD\n{}\nTAIL", "x".repeat(MAX_TOOL_OUTPUT_BYTES));
        let result = bound_tool_result(ToolResult {
            success: true,
            output: original,
            metadata: json!({}),
        });

        assert!(result.output.len() <= MAX_TOOL_OUTPUT_BYTES);
        assert!(result.output.starts_with("HEAD"));
        assert!(result.output.contains("tool output truncated"));
        assert!(result.output.ends_with("TAIL"));
        assert_eq!(result.metadata["outputTruncated"], Value::Bool(true));
        assert!(result.metadata["omittedOutputBytes"].as_u64().unwrap() > 0);
    }

    fn versioned_read_result(
        path: &str,
        revision: &str,
        start_line: usize,
        end_line: usize,
    ) -> ToolResult {
        ToolResult {
            success: true,
            output: "file contents".to_string(),
            metadata: json!({
                "path": path,
                "fileRevision": revision,
                "startLine": start_line,
                "endLine": end_line
            }),
        }
    }

    #[tokio::test]
    async fn repeated_failed_read_still_uses_the_generic_hard_stop() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let read_call = |id: &str| ProviderEvent::ToolCall {
            call: ToolCall {
                id: id.into(),
                name: "read_file".into(),
                arguments: json!({"path": "missing.txt"}),
                metadata: json!({}),
            },
        };
        let provider = Arc::new(FakeProvider::script(vec![
            vec![Ok(read_call("read-1")), Ok(ProviderEvent::Completed)],
            vec![Ok(read_call("read-2")), Ok(ProviderEvent::Completed)],
            vec![Ok(read_call("read-3")), Ok(ProviderEvent::Completed)],
        ]));

        let outcome = runtime
            .run_turn(
                provider,
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "read the missing file".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        assert!(
            outcome
                .error
                .as_deref()
                .is_some_and(|error| error.contains("repeated_tool_call")
                    && error.contains("without producing a versioned observation"))
        );
        let results = repository
            .load(&thread_id)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.kind {
                StoredEventKind::ToolResult {
                    call_id, result, ..
                } => Some((call_id, result)),
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        assert!(!results["read-1"].success);
        assert!(!results["read-2"].success);
        assert!(!results["read-3"].success);
        assert!(results["read-3"].output.contains("repeated_tool_call"));
    }

    #[tokio::test]
    async fn vision_unsupported_models_reply_without_calling_the_provider() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let publisher = Arc::new(RecordingPublisher::default());
        let image_attachment = ImageAttachment {
            name: "screen.png".to_string(),
            data_url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
        };
        let runtime = runtime.with_vision_support(false);
        let provider = Arc::new(FakeProvider::text(&["done"]));
        let outcome = runtime
            .run_turn_with_attachments(
                provider.clone(),
                "text-only".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "看看这张图".to_string(),
                    agent_mode: None,
                },
                vec![image_attachment],
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert!(outcome.error.is_none());
        // 能力闸门必须短路：不发起任何 Provider 请求。
        assert!(provider.requests().is_empty());
        let events = repository.load(&thread_id).await.unwrap();
        let assistant_text = events
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::AssistantMessage { message } => Some(message.text()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(assistant_text.len(), 1);
        assert!(assistant_text[0].contains("当前模型（text-only）不支持图片识别"));
        // 用户消息与图片仍然落库，历史可见。
        assert!(
            events
                .iter()
                .any(|event| { matches!(event.kind, StoredEventKind::UserMessage { .. }) })
        );
        assert!(repository.load(&thread_id).await.unwrap().iter().any(|event| {
            matches!(
                &event.kind,
                StoredEventKind::UserMessage { message } if message.content.iter().any(|block| {
                    matches!(block, ContentBlock::Image { .. })
                })
            )
        }));
        let published = publisher.events.lock().unwrap();
        assert!(published.iter().any(|event| matches!(
            &event.event,
            AgentEvent::TurnCompleted { message, .. } if message.text().contains("不支持图片识别")
        )));
        drop(published);
        assert!(
            repository
                .load(&thread_id)
                .await
                .unwrap()
                .iter()
                .all(|event| { !matches!(event.kind, StoredEventKind::TurnFailed { .. }) })
        );
    }

    #[tokio::test]
    async fn vision_capable_models_still_receive_images() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let publisher = Arc::new(RecordingPublisher::default());
        let runtime = runtime.with_vision_support(true);
        let outcome = runtime
            .run_turn_with_attachments(
                Arc::new(FakeProvider::text(&["图里是一只猫"])),
                "vision-model".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "这张图是什么".to_string(),
                    agent_mode: None,
                },
                vec![ImageAttachment {
                    name: "cat.png".to_string(),
                    data_url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                }],
                CancellationToken::new(),
                publisher,
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
    }

    #[tokio::test]
    async fn caller_assigned_turn_id_is_used_for_events_and_persistence() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let publisher = Arc::new(RecordingPublisher::default());
        let turn_id = "turn-from-start-handle".to_string();

        let outcome = runtime
            .run_turn_with_attachments_and_id(
                Arc::new(FakeProvider::text(&["done"])),
                "fake-model".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "run asynchronously".to_string(),
                    agent_mode: None,
                },
                Vec::new(),
                turn_id.clone(),
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.turn_id, turn_id);
        assert!(publisher.events.lock().unwrap().iter().any(|event| {
            matches!(
                &event.event,
                AgentEvent::TurnStarted { turn_id: published, .. } if published == &turn_id
            )
        }));
        assert!(
            repository
                .load(&thread_id)
                .await
                .unwrap()
                .iter()
                .any(|event| {
                    event.turn_id.as_deref() == Some(turn_id.as_str())
                        && matches!(event.kind, StoredEventKind::TurnStarted)
                })
        );
        let history = repository.read_thread_history(&thread_id).await.unwrap();
        assert!(
            history
                .turns
                .data
                .iter()
                .any(|turn| { turn.id == turn_id && turn.state == TurnState::Completed })
        );
    }

    #[tokio::test]
    async fn steering_continues_the_same_turn_and_enters_the_next_provider_request() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(
            FakeProvider::script(vec![
                vec![
                    Ok(ProviderEvent::TextDelta {
                        delta: "partial answer".into(),
                    }),
                    Ok(ProviderEvent::TextDelta {
                        delta: " obsolete tail".into(),
                    }),
                    Ok(ProviderEvent::Completed),
                ],
                vec![
                    Ok(ProviderEvent::TextDelta {
                        delta: "adjusted answer".into(),
                    }),
                    Ok(ProviderEvent::Completed),
                ],
            ])
            .with_delay(Duration::from_millis(50)),
        );
        let publisher = Arc::new(RecordingPublisher::default());
        let control = TurnControl::new();
        let task_control = control.clone();
        let task_provider = provider.clone();
        let task_publisher = publisher.clone();
        let task_thread_id = thread_id.clone();

        let task = tokio::spawn(async move {
            runtime
                .run_turn_with_attachments_id_and_control(
                    task_provider,
                    "fake-model".into(),
                    RunTurnRequest {
                        thread_id: task_thread_id,
                        input: "initial request".into(),
                        agent_mode: None,
                    },
                    Vec::new(),
                    "turn-steered".into(),
                    CancellationToken::new(),
                    task_control,
                    task_publisher,
                )
                .await
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            while !publisher.events.lock().unwrap().iter().any(|event| {
                matches!(
                    &event.event,
                    AgentEvent::TextDelta { delta, .. } if delta == "partial answer"
                )
            }) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        control
            .steer(build_user_message("adjust it", Vec::new(), false).unwrap())
            .unwrap();
        let outcome = task.await.unwrap().unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(provider.requests().len(), 2);
        assert!(provider.requests()[1].messages.iter().any(|message| {
            matches!(
                message,
                ProviderMessage::Text { role: MessageRole::User, text } if text == "adjust it"
            )
        }));
        let published = publisher.events.lock().unwrap();
        assert!(published.iter().any(|event| {
            matches!(
                &event.event,
                AgentEvent::TurnSteered { turn_id, message, .. }
                    if turn_id == "turn-steered" && message.visible_text() == "adjust it"
            )
        }));
        assert_eq!(
            published
                .iter()
                .filter(|event| matches!(&event.event, AgentEvent::TurnStarted { .. }))
                .count(),
            1
        );
        assert_eq!(
            published
                .iter()
                .filter(|event| matches!(&event.event, AgentEvent::TurnCompleted { .. }))
                .count(),
            1
        );
        assert!(
            !published
                .iter()
                .any(|event| matches!(&event.event, AgentEvent::TurnCancelled { .. }))
        );
        drop(published);
        let stored = repository.load(&thread_id).await.unwrap();
        assert_eq!(
            stored
                .iter()
                .filter(|event| matches!(event.kind, StoredEventKind::UserMessage { .. }))
                .count(),
            2
        );
        let assistant_texts = stored
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::AssistantMessage { message } => Some(message.visible_text()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(assistant_texts, vec!["partial answer", "adjusted answer"]);
    }

    #[tokio::test]
    async fn stopping_after_steer_cancels_the_same_turn_without_a_third_request() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(
            FakeProvider::script(vec![
                vec![
                    Ok(ProviderEvent::TextDelta {
                        delta: "obsolete answer".into(),
                    }),
                    Ok(ProviderEvent::Completed),
                ],
                vec![
                    Ok(ProviderEvent::TextDelta {
                        delta: "adjusted answer".into(),
                    }),
                    Ok(ProviderEvent::Completed),
                ],
            ])
            .with_delay(Duration::from_millis(200)),
        );
        let publisher = Arc::new(RecordingPublisher::default());
        let control = TurnControl::new();
        let cancellation = CancellationToken::new();
        let task_control = control.clone();
        let task_provider = provider.clone();
        let task_publisher = publisher.clone();
        let task_thread_id = thread_id.clone();
        let task_cancellation = cancellation.clone();

        let task = tokio::spawn(async move {
            runtime
                .run_turn_with_attachments_id_and_control(
                    task_provider,
                    "fake-model".into(),
                    RunTurnRequest {
                        thread_id: task_thread_id,
                        input: "initial request".into(),
                        agent_mode: None,
                    },
                    Vec::new(),
                    "turn-steer-then-stop".into(),
                    task_cancellation,
                    task_control,
                    task_publisher,
                )
                .await
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            while provider.requests().is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        control
            .steer(build_user_message("adjust it", Vec::new(), false).unwrap())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while provider.requests().len() < 2 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();

        cancellation.cancel();
        let outcome = task.await.unwrap().unwrap();

        assert_eq!(outcome.state, TurnState::Cancelled);
        assert_eq!(provider.requests().len(), 2);
        let published = publisher.events.lock().unwrap();
        assert_eq!(
            published
                .iter()
                .filter(|event| matches!(&event.event, AgentEvent::TurnStarted { .. }))
                .count(),
            1
        );
        assert_eq!(
            published
                .iter()
                .filter(|event| matches!(&event.event, AgentEvent::TurnCancelled { .. }))
                .count(),
            1
        );
        assert!(
            !published
                .iter()
                .any(|event| matches!(&event.event, AgentEvent::TurnCompleted { .. }))
        );
        drop(published);
        assert_eq!(
            repository
                .load(&thread_id)
                .await
                .unwrap()
                .iter()
                .filter(|event| matches!(event.kind, StoredEventKind::UserMessage { .. }))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn completion_guard_reconciles_a_new_plan_once_without_persisting_the_draft() {
        let (directory, repository, _, thread_id) = runtime_fixture().await;
        let advanced = crate::advanced::AdvancedServices::new(directory.path()).unwrap();
        let handlers = advanced
            .tool_handlers(directory.path())
            .0
            .into_iter()
            .filter(|handler| handler.definition().name == "update_plan")
            .collect();
        let tools = ToolRegistry::new(handlers).unwrap();
        let guard_results = Arc::new(Mutex::new(VecDeque::from([true, false])));
        let guard_results_for_runtime = guard_results.clone();
        let guard: Arc<dyn TurnCompletionGuard> = Arc::new(move |_| {
            Ok(guard_results_for_runtime
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(false))
        });
        let runtime =
            AgentRuntime::with_tools(repository.clone(), tools, directory.path().to_path_buf())
                .with_turn_completion_guard(guard);
        let completed_steps = json!({
            "steps": [
                { "id": "one", "step": "实现功能", "status": "completed" },
                { "id": "two", "step": "验证结果", "status": "completed" }
            ]
        });
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "临时答复草稿".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "reconcile-plan".into(),
                        name: "update_plan".into(),
                        arguments: completed_steps,
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "最终答复".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(RecordingPublisher::default());
        let outcome = runtime
            .run_turn(
                provider.clone(),
                "fixture".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "实现并验证功能".into(),
                    agent_mode: Some("craft".into()),
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(provider.requests().len(), 3);
        assert_eq!(
            repository.read_thread(&thread_id).await.unwrap().messages[1].text(),
            "最终答复"
        );
        let plan = advanced.plans.get(&thread_id).unwrap().unwrap();
        assert!(
            plan.steps
                .iter()
                .all(|step| step.status == crate::advanced::PlanStepState::Completed)
        );
        let events = repository.load(&thread_id).await.unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            StoredEventKind::AssistantToolCalls { text, .. } if text.is_empty()
        )));
        assert!(!events.iter().any(|event| matches!(
            &event.kind,
            StoredEventKind::AssistantToolCalls { text, .. } if text == "临时答复草稿"
        )));
        let published = publisher.events.lock().unwrap();
        let completed = published
            .iter()
            .find_map(|event| match &event.event {
                AgentEvent::TurnCompleted { message, .. } => Some(message),
                _ => None,
            })
            .expect("guarded turn should complete");
        assert_eq!(completed.text(), "最终答复");
        assert!(published.iter().any(|event| matches!(
            &event.event,
            AgentEvent::ActivityStatusChanged {
                status: AgentActivityStatus::Finalizing,
                ..
            }
        )));
        assert!(published.iter().any(|event| matches!(
            &event.event,
            AgentEvent::TextReset { item_id, .. } if item_id == &completed.id
        )));
    }

    #[tokio::test]
    async fn completion_reconciliation_rejects_an_added_plan_step_before_persisting_it() {
        let (directory, repository, _, thread_id) = runtime_fixture().await;
        let advanced = crate::advanced::AdvancedServices::new(directory.path()).unwrap();
        let initial_request: crate::advanced::PlanUpdateRequest = serde_json::from_value(json!({
            "threadId": thread_id,
            "steps": [
                { "id": "one", "step": "实现", "status": "completed" },
                { "id": "two", "step": "验证", "status": "in_progress" }
            ]
        }))
        .unwrap();
        advanced.plans.update(initial_request).unwrap();
        let handlers = advanced
            .tool_handlers(directory.path())
            .0
            .into_iter()
            .filter(|handler| handler.definition().name == "update_plan")
            .collect();
        let tools = ToolRegistry::new(handlers).unwrap();
        let reconciliation = PlanReconciliationContext {
            revision: 1,
            steps: vec![
                crate::tools::PlanReconciliationStep {
                    id: "one".into(),
                    step: "实现".into(),
                },
                crate::tools::PlanReconciliationStep {
                    id: "two".into(),
                    step: "验证".into(),
                },
            ],
        };
        struct TestGuard {
            decisions: Mutex<VecDeque<bool>>,
            context: PlanReconciliationContext,
        }
        impl TurnCompletionGuard for TestGuard {
            fn needs_reconciliation(&self, _turn_started_at_ms: u64) -> Result<bool, String> {
                Ok(self.decisions.lock().unwrap().pop_front().unwrap_or(true))
            }

            fn reconciliation_context(
                &self,
                _turn_started_at_ms: u64,
            ) -> Result<Option<PlanReconciliationContext>, String> {
                Ok(Some(self.context.clone()))
            }
        }
        let runtime = AgentRuntime::with_tools(repository, tools, directory.path().to_path_buf())
            .with_turn_completion_guard(Arc::new(TestGuard {
                decisions: Mutex::new(VecDeque::from([true, true])),
                context: reconciliation,
            }));
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "临时答复".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "add-step".into(),
                        name: "update_plan".into(),
                        arguments: json!({
                            "steps": [
                                { "id": "one", "step": "实现", "status": "completed" },
                                { "id": "two", "step": "验证", "status": "completed" },
                                { "id": "three", "step": "向用户说明", "status": "in_progress" }
                            ]
                        }),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "最终答复".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let outcome = runtime
            .run_turn(
                provider,
                "fixture".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "执行计划任务".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        let plan = advanced.plans.get(&thread_id).unwrap().unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert!(!plan.steps.iter().any(|step| step.id == "three"));
        assert!(plan.steps.iter().any(|step| {
            step.id == "two" && step.status == crate::advanced::PlanStepState::InProgress
        }));
    }

    #[tokio::test]
    async fn completion_guard_fails_after_one_unsuccessful_reconciliation_request() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let guard_results = Arc::new(Mutex::new(VecDeque::from([true, true])));
        let guard_results_for_runtime = guard_results.clone();
        let runtime = runtime.with_turn_completion_guard(Arc::new(move |_| {
            Ok(guard_results_for_runtime
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(true))
        }));
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "第一份草稿".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "第二份草稿".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let outcome = runtime
            .run_turn(
                provider.clone(),
                "fixture".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "执行计划任务".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        assert_eq!(provider.requests().len(), 2);
        let events = repository.load(&thread_id).await.unwrap();
        assert!(events.iter().any(|event| matches!(
            event.kind,
            StoredEventKind::TurnFailed { ref message, .. }
                if message.contains("计划收尾同步失败")
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind, StoredEventKind::AssistantMessage { .. }))
        );
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
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();
        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert_eq!(result.state, TurnState::Completed);
        assert!(result.completed_at_ms >= result.started_at_ms);
        assert_eq!(
            result.duration_ms,
            result.completed_at_ms.saturating_sub(result.started_at_ms)
        );
        assert_eq!(detail.messages[1].text(), "hello world");
        assert_eq!(provider.requests()[0].messages.len(), 1);
        let published = publisher.events.lock().unwrap().clone();
        let text_item_ids = published
            .iter()
            .filter_map(|event| match &event.event {
                AgentEvent::TextDelta { item_id, .. } => Some(item_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let completed_message_id = published
            .iter()
            .find_map(|event| match &event.event {
                AgentEvent::TurnCompleted { message, .. } => Some(message.id.clone()),
                _ => None,
            })
            .expect("turn completion should publish the final assistant item");
        let item_lifecycle = published
            .iter()
            .filter_map(|event| match &event.event {
                AgentEvent::ItemStarted {
                    item_id, item_type, ..
                } => Some((item_id.clone(), *item_type, None)),
                AgentEvent::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                    ..
                } => Some((item_id.clone(), *item_type, Some(*status))),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(item_lifecycle.len(), 2);
        assert_eq!(item_lifecycle[0].0, completed_message_id);
        assert_eq!(item_lifecycle[0].1, AgentItemType::AgentMessage);
        assert_eq!(item_lifecycle[1].0, completed_message_id);
        assert_eq!(item_lifecycle[1].2, Some(AgentItemStatus::Completed));
        let stored_lifecycle = repository
            .load(&thread_id)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.kind {
                StoredEventKind::ItemStarted { item_id, item_type } => {
                    Some((item_id, item_type, None))
                }
                StoredEventKind::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                } => Some((item_id, item_type, Some(status))),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(stored_lifecycle, item_lifecycle);
        assert_eq!(text_item_ids.len(), 2);
        assert!(
            text_item_ids
                .iter()
                .all(|item_id| item_id == &completed_message_id)
        );
        assert!(matches!(
            detail.turn_timeline.as_slice(),
            [TurnTimelineItem::Text { id, text, .. }, TurnTimelineItem::Event { .. }]
                if id == &completed_message_id && text == "hello world"
        ));
        assert!(matches!(
            published
                .iter()
                .last()
                .map(|event| &event.event),
            Some(AgentEvent::TurnCompleted {
                started_at_ms,
                completed_at_ms,
                duration_ms,
                ..
            }) if *started_at_ms == result.started_at_ms
                && *completed_at_ms == result.completed_at_ms
                && *duration_ms == result.duration_ms
        ));
    }

    #[tokio::test]
    async fn persists_generated_images_as_assistant_content() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::new(vec![
            Ok(ProviderEvent::Image {
                mime_type: "image/png".into(),
                data: "AA==".into(),
            }),
            Ok(ProviderEvent::Completed),
        ]));
        let publisher = Arc::new(RecordingPublisher::default());

        let outcome = runtime
            .run_turn(
                provider,
                "gemini-3-pro-image-preview".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "draw a blue square".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert!(matches!(
            detail.messages[1].content.as_slice(),
            [ContentBlock::Image { name, data_url }]
                if name == "generated-image-1.png" && data_url == "data:image/png;base64,AA=="
        ));
        assert!(matches!(
            publisher.events.lock().unwrap().iter().find_map(|event| match &event.event {
                AgentEvent::TurnCompleted { message, .. } => Some(message),
                _ => None,
            }),
            Some(ChatMessage { content, .. })
                if matches!(content.as_slice(), [ContentBlock::Image { .. }])
        ));
    }

    #[tokio::test]
    async fn rejects_invalid_generated_image_payloads() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::new(vec![
            Ok(ProviderEvent::Image {
                mime_type: "image/svg+xml".into(),
                data: "AA==".into(),
            }),
            Ok(ProviderEvent::Completed),
        ]));
        let publisher = Arc::new(RecordingPublisher::default());

        let outcome = runtime
            .run_turn(
                provider,
                "gemini-3-pro-image-preview".into(),
                RunTurnRequest {
                    thread_id,
                    input: "draw it".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher,
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        assert!(outcome.error.unwrap().contains("invalid generated image"));
    }

    #[tokio::test]
    async fn persists_reasoning_item_lifecycle_and_ignores_duplicate_completion() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::new(vec![
            Ok(ProviderEvent::ReasoningSummaryDelta {
                item_id: "reasoning-1".into(),
                delta: "检查公开契约。".into(),
            }),
            Ok(ProviderEvent::ReasoningSummaryCompleted {
                item_id: "reasoning-1".into(),
                summary: "检查公开契约。".into(),
            }),
            Ok(ProviderEvent::ReasoningSummaryCompleted {
                item_id: "reasoning-1".into(),
                summary: "不应重复。".into(),
            }),
            Ok(ProviderEvent::ReasoningSummaryCompleted {
                item_id: "reasoning-2".into(),
                summary: "直接完成的安全摘要。".into(),
            }),
            Ok(ProviderEvent::TextDelta {
                delta: "完成。".into(),
            }),
            Ok(ProviderEvent::Completed),
        ]));
        let publisher = Arc::new(RecordingPublisher::default());

        let outcome = runtime
            .run_turn(
                provider,
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "inspect".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        let events = repository.load(&thread_id).await.unwrap();
        let reasoning_lifecycle = events
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::ItemStarted { item_id, item_type }
                | StoredEventKind::ItemCompleted {
                    item_id, item_type, ..
                } if *item_type == AgentItemType::Reasoning => Some(item_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            reasoning_lifecycle,
            ["reasoning-1", "reasoning-1", "reasoning-2", "reasoning-2"]
        );
        let summaries = events
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::ReasoningSummary { item_id, summary } => {
                    Some((item_id.as_str(), summary.as_str()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            summaries,
            [
                ("reasoning-1", "检查公开契约。"),
                ("reasoning-2", "直接完成的安全摘要。")
            ]
        );
        assert_eq!(
            publisher
                .events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| matches!(
                    &event.event,
                    AgentEvent::ItemCompleted {
                        item_type: AgentItemType::Reasoning,
                        status: AgentItemStatus::Completed,
                        ..
                    }
                ))
                .count(),
            2
        );
        assert!(
            !provider_history(events, false)
                .request_messages()
                .iter()
                .any(|message| match message {
                    ProviderMessage::Text { text, .. }
                    | ProviderMessage::UserContent { text, .. }
                    | ProviderMessage::AssistantToolCalls { text, .. } => text.contains("安全摘要"),
                    ProviderMessage::ToolResult { .. }
                    | ProviderMessage::ProviderContext { .. } => false,
                })
        );
    }

    #[tokio::test]
    async fn oversized_reasoning_item_is_closed_as_failed() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::new(vec![Ok(
            ProviderEvent::ReasoningSummaryDelta {
                item_id: "reasoning-limit".into(),
                delta: "x".repeat(MAX_REASONING_SUMMARY_BYTES + 1),
            },
        )]));

        let outcome = runtime
            .run_turn(
                provider,
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "bounded".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        assert!(
            repository
                .load(&thread_id)
                .await
                .unwrap()
                .iter()
                .any(|event| matches!(
                    &event.kind,
                    StoredEventKind::ItemCompleted {
                        item_id,
                        item_type: AgentItemType::Reasoning,
                        status: AgentItemStatus::Failed,
                    } if item_id == "reasoning-limit"
                ))
        );
    }

    #[tokio::test]
    async fn active_reasoning_item_is_closed_when_the_turn_is_cancelled() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let cancellation = CancellationToken::new();
        let publisher = Arc::new(ReasoningCancellingPublisher {
            cancellation: cancellation.clone(),
            events: Mutex::new(Vec::new()),
        });
        let provider = Arc::new(
            FakeProvider::new(vec![
                Ok(ProviderEvent::ReasoningSummaryDelta {
                    item_id: "reasoning-cancelled".into(),
                    delta: "正在检查。".into(),
                }),
                Ok(ProviderEvent::TextDelta {
                    delta: "不应完成".into(),
                }),
                Ok(ProviderEvent::Completed),
            ])
            .with_delay(Duration::from_millis(10)),
        );

        let outcome = runtime
            .run_turn(
                provider,
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "cancel reasoning".into(),
                    agent_mode: None,
                },
                cancellation,
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Cancelled);
        assert!(
            repository
                .load(&thread_id)
                .await
                .unwrap()
                .iter()
                .any(|event| matches!(
                    &event.kind,
                    StoredEventKind::ItemCompleted {
                        item_id,
                        item_type: AgentItemType::Reasoning,
                        status: AgentItemStatus::Cancelled,
                    } if item_id == "reasoning-cancelled"
                ))
        );
        assert!(
            publisher
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(
                    &event.event,
                    AgentEvent::ItemCompleted {
                        item_id,
                        item_type: AgentItemType::Reasoning,
                        status: AgentItemStatus::Cancelled,
                        ..
                    } if item_id == "reasoning-cancelled"
                ))
        );
    }

    #[tokio::test]
    async fn executes_a_native_tool_and_continues_until_final_text() {
        let (directory, repository, runtime, thread_id) = runtime_fixture().await;
        std::fs::write(directory.path().join("README.md"), "workspace docs").unwrap();
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "read_file".to_string(),
            arguments: json!({
                "path": "README.md",
                "offset": 0,
                "limit": 262_144,
                "startLine": 1,
                "lineCount": 220
            }),
            metadata: json!({}),
        };
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "I will read the workspace docs".to_string(),
                }),
                Ok(ProviderEvent::ToolCall { call: call.clone() }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "I read it".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(RecordingPublisher::default());
        let outcome = runtime
            .run_turn(
                provider.clone(),
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "read the docs".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();
        assert_eq!(outcome.state, TurnState::Completed);
        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        assert!(matches!(
            &requests[1].messages[1],
            ProviderMessage::AssistantToolCalls { text, calls }
                if text == "I will read the workspace docs" && calls.len() == 1
        ));
        assert!(matches!(
            requests[1].messages.last(),
            Some(ProviderMessage::ToolResult { output, .. })
                if output.starts_with("[read_file observation] ")
                    && output.contains(r#""path":"README.md""#)
                    && output.contains(r#""startLine":1"#)
                    && output.contains(r#""endLine":1"#)
                    && output.ends_with("workspace docs")
        ));
        let events = repository.load(&thread_id).await.unwrap();
        let stored_result = events.iter().find_map(|event| match &event.kind {
            StoredEventKind::ToolResult { result, .. } => Some(result),
            _ => None,
        });
        assert_eq!(
            stored_result.map(|result| result.output.as_str()),
            Some("workspace docs")
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind, StoredEventKind::ToolResult { .. }))
                .count(),
            1
        );
        let item_lifecycle = events
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::ItemStarted { item_id, item_type }
                | StoredEventKind::ItemCompleted {
                    item_id, item_type, ..
                } if *item_type == AgentItemType::AgentMessage => Some(item_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(item_lifecycle.len(), 4);
        assert_eq!(item_lifecycle[0], item_lifecycle[1]);
        assert_ne!(item_lifecycle[1], item_lifecycle[2]);
        assert_eq!(item_lifecycle[2], item_lifecycle[3]);
        let stored_tool_lifecycle = events
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::ItemStarted { item_id, item_type }
                    if *item_type == AgentItemType::Tool =>
                {
                    Some((item_id.clone(), None))
                }
                StoredEventKind::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                } if *item_type == AgentItemType::Tool => Some((item_id.clone(), Some(*status))),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stored_tool_lifecycle,
            [
                ("call-1".to_string(), None),
                ("call-1".to_string(), Some(AgentItemStatus::Completed)),
            ]
        );
        let published_tool_lifecycle = publisher
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match &event.event {
                AgentEvent::ItemStarted {
                    item_id, item_type, ..
                } if *item_type == AgentItemType::Tool => Some(format!("item_started:{item_id}")),
                AgentEvent::ToolStarted { call, .. } => Some(format!("tool_started:{}", call.id)),
                AgentEvent::ToolCompleted { call_id, .. } => {
                    Some(format!("tool_completed:{call_id}"))
                }
                AgentEvent::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                    ..
                } if *item_type == AgentItemType::Tool => {
                    Some(format!("item_completed:{item_id}:{status:?}"))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            published_tool_lifecycle,
            [
                "item_started:call-1",
                "tool_started:call-1",
                "tool_completed:call-1",
                "item_completed:call-1:Completed",
            ]
        );
        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert_eq!(detail.tool_activities.len(), 1);
        assert!(matches!(
            &detail.turn_timeline[..],
            [
                TurnTimelineItem::Text { text: progress, .. },
                TurnTimelineItem::Tool { .. },
                TurnTimelineItem::Text { text: answer, .. },
                TurnTimelineItem::Event { kind: crate::storage::TimelineEventKind::TurnCompleted, .. }
            ] if progress == "I will read the workspace docs" && answer == "I read it"
        ));
    }

    #[tokio::test]
    async fn executes_tool_calls_in_provider_order() {
        let directory = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(directory.path()).unwrap());
        let thread = repository.create_thread().await.unwrap();
        let tools = ToolRegistry::new(vec![Arc::new(DelayTool)]).unwrap();
        let runtime = AgentRuntime::with_tools(repository, tools, directory.path().to_path_buf());
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "slow".to_string(),
                        name: "delay".to_string(),
                        arguments: json!({ "delayMs": 200, "label": "slow" }),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "fast".to_string(),
                        name: "delay".to_string(),
                        arguments: json!({ "delayMs": 5, "label": "fast" }),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "done".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(RecordingPublisher::default());

        runtime
            .run_turn(
                provider,
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread.id,
                    input: "run both".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        let lifecycle = publisher
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match &event.event {
                AgentEvent::ToolQueued { call, .. } => Some(format!("queued:{}", call.id)),
                AgentEvent::ToolStarted { call, .. } => Some(format!("started:{}", call.id)),
                AgentEvent::ToolCompleted { call_id, .. } => Some(format!("completed:{call_id}")),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lifecycle,
            [
                "queued:slow",
                "queued:fast",
                "started:slow",
                "completed:slow",
                "started:fast",
                "completed:fast"
            ]
        );
    }

    #[tokio::test]
    async fn persists_user_input_before_waiting_and_records_its_resolution() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let call = ToolCall {
            id: "call-input".to_string(),
            name: REQUEST_USER_INPUT_TOOL_NAME.to_string(),
            arguments: json!({
                "questions": [{
                    "question": "Choose an approach",
                    "options": ["Conservative", "Fast"]
                }]
            }),
            metadata: json!({}),
        };
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall { call }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "Proceeding conservatively".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let runtime = Arc::new(runtime);
        let manager = runtime.user_input_manager();
        let publisher = Arc::new(RecordingPublisher::default());
        let run_runtime = runtime.clone();
        let run_thread_id = thread_id.clone();
        let run_publisher = publisher.clone();
        let run = tokio::spawn(async move {
            run_runtime
                .run_turn(
                    provider,
                    "fake".to_string(),
                    RunTurnRequest {
                        thread_id: run_thread_id,
                        input: "plan the change".to_string(),
                        agent_mode: None,
                    },
                    CancellationToken::new(),
                    run_publisher,
                )
                .await
        });

        let request = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let detail = repository.read_thread(&thread_id).await.unwrap();
                if let Some(input) = detail.user_inputs.first() {
                    break input.request.clone();
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("user input request should be persisted before the runtime waits");
        assert_eq!(manager.pending_count().await, 1);
        manager
            .resolve(
                &request.id,
                UserInputResolution {
                    action: UserInputAction::Answered,
                    answers: vec![crate::protocol::UserInputAnswer {
                        question: request.questions[0].question.clone(),
                        answer: "Conservative".to_string(),
                    }],
                },
            )
            .await
            .unwrap();

        assert_eq!(run.await.unwrap().unwrap().state, TurnState::Completed);
        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert!(matches!(
            detail.user_inputs[0].resolution,
            Some(UserInputResolution {
                action: UserInputAction::Answered,
                ..
            })
        ));
        assert!(detail.turn_timeline.iter().any(|item| matches!(
            item,
            TurnTimelineItem::Event {
                kind: crate::storage::TimelineEventKind::UserInputRequested,
                ..
            }
        )));
        assert!(detail.turn_timeline.iter().any(|item| matches!(
            item,
            TurnTimelineItem::Event {
                kind: crate::storage::TimelineEventKind::UserInputResolved,
                ..
            }
        )));
        let stored_lifecycle = repository
            .load(&thread_id)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.kind {
                StoredEventKind::ItemStarted { item_id, item_type }
                    if item_type == AgentItemType::UserInput =>
                {
                    Some((item_id, None))
                }
                StoredEventKind::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                } if item_type == AgentItemType::UserInput => Some((item_id, Some(status))),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stored_lifecycle,
            [
                (request.id.clone(), None),
                (request.id.clone(), Some(AgentItemStatus::Completed)),
            ]
        );
        let published_lifecycle = publisher
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match &event.event {
                AgentEvent::ItemStarted {
                    item_id, item_type, ..
                } if *item_type == AgentItemType::UserInput => Some((item_id.clone(), None)),
                AgentEvent::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                    ..
                } if *item_type == AgentItemType::UserInput => {
                    Some((item_id.clone(), Some(*status)))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(published_lifecycle, stored_lifecycle);
    }

    #[tokio::test]
    async fn unanswered_questions_and_continuations_wait_until_answered_or_interrupted() {
        for continuation in [false, true] {
            for interrupt in [false, true] {
                let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
                let call = if continuation {
                    ToolCall {
                        id: "inspect-before-wait".into(),
                        name: "list_directory".into(),
                        arguments: json!({"path": "."}),
                        metadata: json!({}),
                    }
                } else {
                    ToolCall {
                        id: "question-before-wait".into(),
                        name: REQUEST_USER_INPUT_TOOL_NAME.into(),
                        arguments: json!({"questions": [{
                            "question": "Choose an approach",
                            "options": ["Conservative", "Fast"]
                        }]}),
                        metadata: json!({}),
                    }
                };
                let provider = Arc::new(FakeProvider::script(vec![
                    vec![
                        Ok(ProviderEvent::ToolCall { call }),
                        Ok(ProviderEvent::Completed),
                    ],
                    vec![
                        Ok(ProviderEvent::TextDelta {
                            delta: "Continued after the answer".into(),
                        }),
                        Ok(ProviderEvent::Completed),
                    ],
                ]));
                let runtime = if continuation {
                    runtime.with_soft_turn_limits(SoftTurnLimits::new(1, u64::MAX, u64::MAX))
                } else {
                    runtime
                };
                let manager = runtime.user_input_manager();
                let cancellation = CancellationToken::new();
                let run = tokio::spawn({
                    let provider = provider.clone();
                    let thread_id = thread_id.clone();
                    let cancellation = cancellation.clone();
                    async move {
                        runtime
                            .run_turn(
                                provider,
                                "fake".into(),
                                RunTurnRequest {
                                    thread_id,
                                    input: "Complete the task".into(),
                                    agent_mode: None,
                                },
                                cancellation,
                                Arc::new(RecordingPublisher::default()),
                            )
                            .await
                    }
                });
                let request = tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        let detail = repository.read_thread(&thread_id).await.unwrap();
                        if let Some(input) = detail.user_inputs.first() {
                            break input.request.clone();
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect("request should be persisted before waiting");
                assert_eq!(request.expires_at_ms, None);
                assert_eq!(
                    request.kind,
                    if continuation {
                        UserInputRequestKind::TurnContinuation
                    } else {
                        UserInputRequestKind::ModelQuestion
                    }
                );
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(86_401)).await;
                tokio::task::yield_now().await;
                assert!(!run.is_finished());
                assert_eq!(manager.pending_count().await, 1);
                assert_eq!(provider.requests().len(), 1);
                tokio::time::resume();
                let waiting = repository.read_thread(&thread_id).await.unwrap();
                assert!(waiting.user_inputs[0].resolution.is_none());
                assert!(!matches!(
                    waiting.last_turn.unwrap().state,
                    TurnState::Completed | TurnState::Failed | TurnState::Cancelled
                ));
                if interrupt {
                    cancellation.cancel();
                } else {
                    manager
                        .resolve(
                            &request.id,
                            UserInputResolution {
                                action: UserInputAction::Answered,
                                answers: vec![crate::protocol::UserInputAnswer {
                                    question: request.questions[0].question.clone(),
                                    answer: if continuation {
                                        TURN_CONTINUE
                                    } else {
                                        "Conservative"
                                    }
                                    .into(),
                                }],
                            },
                        )
                        .await
                        .unwrap();
                }
                let outcome = tokio::time::timeout(Duration::from_secs(5), run)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    outcome.state,
                    if interrupt {
                        TurnState::Cancelled
                    } else {
                        TurnState::Completed
                    }
                );
                assert_eq!(provider.requests().len(), if interrupt { 1 } else { 2 });
                assert_eq!(manager.pending_count().await, 0);
                let detail = repository.read_thread(&thread_id).await.unwrap();
                assert_eq!(
                    detail.user_inputs[0].resolution.as_ref().unwrap().action,
                    if interrupt {
                        UserInputAction::Cancelled
                    } else {
                        UserInputAction::Answered
                    }
                );
            }
        }
    }

    #[tokio::test]
    async fn skipped_user_input_closes_item_as_failed() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let call = ToolCall {
            id: "call-skipped-input".to_string(),
            name: REQUEST_USER_INPUT_TOOL_NAME.to_string(),
            arguments: json!({
                "questions": [{
                    "question": "Choose an approach",
                    "options": ["Conservative", "Fast"]
                }]
            }),
            metadata: json!({}),
        };
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall { call }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "Continuing without an answer".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let runtime = Arc::new(runtime);
        let manager = runtime.user_input_manager();
        let run_runtime = runtime.clone();
        let run_thread_id = thread_id.clone();
        let run = tokio::spawn(async move {
            run_runtime
                .run_turn(
                    provider,
                    "fake".to_string(),
                    RunTurnRequest {
                        thread_id: run_thread_id,
                        input: "plan the change".to_string(),
                        agent_mode: None,
                    },
                    CancellationToken::new(),
                    Arc::new(RecordingPublisher::default()),
                )
                .await
        });

        let request_id = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let detail = repository.read_thread(&thread_id).await.unwrap();
                if let Some(input) = detail.user_inputs.first() {
                    break input.request.id.clone();
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("user input request should be persisted before the runtime waits");
        manager
            .resolve(
                &request_id,
                UserInputResolution {
                    action: UserInputAction::Skipped,
                    answers: Vec::new(),
                },
            )
            .await
            .unwrap();

        assert_eq!(run.await.unwrap().unwrap().state, TurnState::Completed);
        assert!(
            repository
                .load(&thread_id)
                .await
                .unwrap()
                .iter()
                .any(|event| matches!(
                    &event.kind,
                    StoredEventKind::ItemCompleted {
                        item_id,
                        item_type: AgentItemType::UserInput,
                        status: AgentItemStatus::Failed,
                    } if item_id == &request_id
                ))
        );
    }

    #[tokio::test]
    async fn cancelled_turn_retries_without_duplicating_the_user_message() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = runtime
            .run_turn(
                Arc::new(FakeProvider::text(&["unused"])),
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "retry this task".to_string(),
                    agent_mode: None,
                },
                cancellation,
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();
        assert_eq!(cancelled.state, TurnState::Cancelled);

        let retry_turn_id = "assigned-retry-turn".to_string();
        let completed = runtime
            .retry_turn_with_id_and_control(
                Arc::new(FakeProvider::text(&["retry completed"])),
                "fake".to_string(),
                thread_id.clone(),
                AgentMode::Craft,
                retry_turn_id.clone(),
                CancellationToken::new(),
                TurnControl::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();
        assert_eq!(completed.state, TurnState::Completed);
        assert_eq!(completed.turn_id, retry_turn_id);

        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert_eq!(
            detail
                .messages
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            1
        );
        assert_eq!(detail.messages.last().unwrap().text(), "retry completed");
    }

    #[tokio::test]
    async fn allows_a_progressing_turn_to_continue_past_twenty_four_tool_calls() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let paths = (0..25)
            .map(|index| {
                let path = format!("inspection-{index}");
                std::fs::create_dir(directory.path().join(&path)).unwrap();
                std::fs::write(
                    directory
                        .path()
                        .join(&path)
                        .join(format!("result-{index}.txt")),
                    format!("result {index}"),
                )
                .unwrap();
                path
            })
            .collect::<Vec<_>>();
        let mut scripts = paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                vec![
                    Ok(ProviderEvent::ToolCall {
                        call: ToolCall {
                            id: format!("call-{index}"),
                            name: "list_directory".to_string(),
                            arguments: json!({ "path": path }),
                            metadata: json!({}),
                        },
                    }),
                    Ok(ProviderEvent::Completed),
                ]
            })
            .collect::<Vec<_>>();
        scripts.push(vec![
            Ok(ProviderEvent::TextDelta {
                delta: "Complex turn completed".to_string(),
            }),
            Ok(ProviderEvent::Completed),
        ]);

        let outcome = runtime
            .run_turn(
                Arc::new(FakeProvider::script(scripts)),
                "fake".to_string(),
                RunTurnRequest {
                    thread_id,
                    input: "inspect the whole workspace".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
    }

    #[tokio::test]
    async fn no_progress_detection_stops_an_unbounded_failed_tool_loop() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let mut scripts = (0..21)
            .map(|index| {
                vec![
                    Ok(ProviderEvent::ToolCall {
                        call: ToolCall {
                            id: format!("failed-call-{index}"),
                            name: "list_directory".to_string(),
                            arguments: json!({ "path": format!("missing-{index}") }),
                            metadata: json!({}),
                        },
                    }),
                    Ok(ProviderEvent::Completed),
                ]
            })
            .collect::<Vec<_>>();
        scripts.push(vec![
            Ok(ProviderEvent::TextDelta {
                delta: "This response must not be reached".to_string(),
            }),
            Ok(ProviderEvent::Completed),
        ]);

        let outcome = runtime
            .run_turn(
                Arc::new(FakeProvider::script(scripts)),
                "fake".to_string(),
                RunTurnRequest {
                    thread_id,
                    input: "keep retrying missing paths".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        assert!(outcome.error.unwrap().contains("无实质进展"));
    }

    #[tokio::test]
    async fn ordinary_turn_does_not_fail_on_cumulative_million_token_usage() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let mut scripts = (0..7)
            .map(|index| {
                vec![
                    Ok(ProviderEvent::Usage {
                        usage: TokenUsage {
                            input_tokens: 145_000,
                            output_tokens: 100,
                            total_tokens: 145_100,
                        },
                    }),
                    Ok(ProviderEvent::ToolCall {
                        call: ToolCall {
                            id: format!("million-token-call-{index}"),
                            name: "list_directory".to_string(),
                            arguments: json!({ "path": format!("missing-{index}") }),
                            metadata: json!({}),
                        },
                    }),
                    Ok(ProviderEvent::Completed),
                ]
            })
            .collect::<Vec<_>>();
        scripts.push(vec![
            Ok(ProviderEvent::Usage {
                usage: TokenUsage {
                    input_tokens: 145_000,
                    output_tokens: 100,
                    total_tokens: 145_100,
                },
            }),
            Ok(ProviderEvent::TextDelta {
                delta: "Long turn completed".to_string(),
            }),
            Ok(ProviderEvent::Completed),
        ]);

        let outcome = runtime
            .run_turn(
                Arc::new(FakeProvider::script(scripts)),
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "complete a long task".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        let total_tokens = repository
            .load(&thread_id)
            .await
            .unwrap()
            .iter()
            .filter_map(|event| match event.kind {
                StoredEventKind::ProviderCallUsage { usage, .. } => Some(usage.total_tokens),
                _ => None,
            })
            .sum::<u64>();
        assert_eq!(total_tokens, 1_160_800);
    }

    #[test]
    fn soft_turn_segment_checks_calls_tokens_and_elapsed_time() {
        let limits = SoftTurnLimits::new(30, 1_000, 60_000);
        assert!(
            !SoftTurnSegmentUsage {
                provider_calls: 0,
                total_tokens: 2_000,
                duration_ms: 120_000,
            }
            .exceeds(limits)
        );
        assert!(
            SoftTurnSegmentUsage {
                provider_calls: 30,
                total_tokens: 0,
                duration_ms: 0,
            }
            .exceeds(limits)
        );
        assert!(
            SoftTurnSegmentUsage {
                provider_calls: 1,
                total_tokens: 1_000,
                duration_ms: 0,
            }
            .exceeds(limits)
        );
        assert!(
            SoftTurnSegmentUsage {
                provider_calls: 1,
                total_tokens: 0,
                duration_ms: 60_000,
            }
            .exceeds(limits)
        );
    }

    #[test]
    fn soft_turn_default_does_not_pause_for_elapsed_time_alone() {
        for duration_ms in [599_999, 600_000, 644_000, 86_400_000] {
            assert!(
                !SoftTurnSegmentUsage {
                    provider_calls: 21,
                    total_tokens: 599_384,
                    duration_ms,
                }
                .exceeds(SoftTurnLimits::default()),
                "productive turn paused after {duration_ms} ms"
            );
        }
    }

    #[tokio::test]
    async fn soft_turn_default_continuation_prompt_has_no_time_allowance() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let publisher: Arc<dyn EventPublisher> = Arc::new(UserInputResolvingPublisher::new(
            runtime.user_input_manager(),
            TURN_CONTINUE,
        ));
        runtime
            .request_turn_continuation(
                &thread_id,
                "default-limit-prompt",
                SoftTurnSegmentUsage {
                    provider_calls: 100,
                    total_tokens: 599_384,
                    duration_ms: 644_000,
                },
                SoftTurnLimits::default(),
                CancellationToken::new(),
                &publisher,
            )
            .await
            .unwrap();
        let detail = repository.read_thread(&thread_id).await.unwrap();
        let question = &detail.user_inputs[0].request.questions[0].question;
        assert!(question.contains("运行 644 秒"));
        assert!(question.contains("100 次调用 / 5000000 tokens）"));
    }

    #[test]
    fn soft_turn_default_requests_continuation_at_100_provider_calls() {
        let limits = SoftTurnLimits::default();

        assert_eq!(limits.provider_calls, 100);
        assert_eq!(limits.total_tokens, DEFAULT_SOFT_TURN_TOTAL_TOKENS);
        assert_eq!(limits.duration_ms, DEFAULT_SOFT_TURN_DURATION_MS);
        assert!(
            !SoftTurnSegmentUsage {
                provider_calls: 99,
                total_tokens: 0,
                duration_ms: 0,
            }
            .exceeds(limits)
        );
        assert!(
            SoftTurnSegmentUsage {
                provider_calls: 100,
                total_tokens: 0,
                duration_ms: 0,
            }
            .exceeds(limits)
        );
    }

    #[test]
    fn soft_turn_default_requests_continuation_at_five_million_tokens() {
        let limits = SoftTurnLimits::default();

        assert!(
            !SoftTurnSegmentUsage {
                provider_calls: 1,
                total_tokens: 4_999_999,
                duration_ms: 0,
            }
            .exceeds(limits)
        );
        assert!(
            SoftTurnSegmentUsage {
                provider_calls: 1,
                total_tokens: 5_000_000,
                duration_ms: 0,
            }
            .exceeds(limits)
        );
    }

    #[tokio::test]
    async fn soft_turn_limit_can_continue_with_a_fresh_segment() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "before-continuation".into(),
                        name: "list_directory".into(),
                        arguments: json!({ "path": "." }),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "continued safely".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(UserInputResolvingPublisher::new(
            runtime.user_input_manager(),
            TURN_CONTINUE,
        ));

        let outcome = runtime
            .with_soft_turn_limits(SoftTurnLimits::new(1, u64::MAX, u64::MAX))
            .run_turn(
                provider.clone(),
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "inspect and continue".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher,
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(provider.requests().len(), 2);
        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert_eq!(detail.user_inputs.len(), 1);
        assert_eq!(
            detail.user_inputs[0].request.kind,
            UserInputRequestKind::TurnContinuation
        );
        assert!(
            detail.user_inputs[0].request.questions[0]
                .question
                .contains("请发送“继续”")
        );
        assert_eq!(
            detail.user_inputs[0]
                .resolution
                .as_ref()
                .and_then(|resolution| resolution.answers.first())
                .map(|answer| answer.answer.as_str()),
            Some(TURN_CONTINUE)
        );
    }

    #[tokio::test]
    async fn soft_turn_limit_stop_cancels_before_another_provider_call() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "before-stop".into(),
                        name: "list_directory".into(),
                        arguments: json!({ "path": "." }),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "must not run".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(UserInputResolvingPublisher::new(
            runtime.user_input_manager(),
            TURN_STOP,
        ));

        let outcome = runtime
            .with_soft_turn_limits(SoftTurnLimits::new(1, u64::MAX, u64::MAX))
            .run_turn(
                provider.clone(),
                "fake".into(),
                RunTurnRequest {
                    thread_id,
                    input: "stop at the soft boundary".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher,
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Cancelled);
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn soft_turn_limit_can_force_compaction_before_continuing() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        for index in 0..8 {
            repository
                .append(StoredEvent::new(
                    &thread_id,
                    None,
                    StoredEventKind::AssistantMessage {
                        message: text_message(
                            MessageRole::Assistant,
                            format!("history-{index} {}", "x".repeat(300)),
                        ),
                    },
                ))
                .await
                .unwrap();
        }
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "before-forced-compaction".into(),
                        name: "list_directory".into(),
                        arguments: json!({ "path": "." }),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "compacted and continued".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(UserInputResolvingPublisher::new(
            runtime.user_input_manager(),
            TURN_COMPACT_AND_CONTINUE,
        ));

        let outcome = runtime
            .with_context_limit(2_000)
            .with_soft_turn_limits(SoftTurnLimits::new(1, u64::MAX, u64::MAX))
            .run_turn(
                provider,
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "compact before continuing".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher,
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert!(
            repository
                .load(&thread_id)
                .await
                .unwrap()
                .iter()
                .any(|event| {
                    matches!(
                        event.kind,
                        StoredEventKind::ContextCompacted {
                            automatic: true,
                            ..
                        }
                    )
                })
        );
    }

    #[tokio::test]
    async fn rate_limit_retry_is_visible_and_cancellation_prevents_replay() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let logger = StructuredLogger::new(directory.path()).unwrap();
        let runtime = runtime.with_logger(logger.clone());
        let provider = Arc::new(PreStreamProvider::new(vec![Err(
            ProviderError::RateLimited {
                message: "free tier".into(),
                retry_after: None,
            },
        )]));
        let publisher = Arc::new(RecordingPublisher::default());
        let token = CancellationToken::new();
        let task = tokio::spawn({
            let provider = provider.clone();
            let publisher = publisher.clone();
            let token = token.clone();
            async move {
                runtime
                    .run_turn(
                        provider,
                        "fixture".into(),
                        RunTurnRequest {
                            thread_id,
                            input: "read documents".into(),
                            agent_mode: None,
                        },
                        token,
                        publisher,
                    )
                    .await
                    .unwrap()
            }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if publisher
                    .events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event.event, AgentEvent::ProviderRetryWaiting { .. }))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        token.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .state,
            TurnState::Cancelled
        );
        assert_eq!(provider.requests().len(), 1);
        let logs = logger
            .read_logs(crate::logging::LogQuery {
                limit: None,
                level: None,
                event: Some("provider_rate_limited".into()),
                after_timestamp_ms: None,
            })
            .unwrap();
        assert_eq!(logs.records.len(), 1);
        assert_eq!(logs.records[0].fields["retryDelayMs"], 60_000);
        assert_eq!(logs.records[0].fields["delaySource"], "default");
        assert_eq!(logs.records[0].fields["retryNumber"], 1);
    }

    #[tokio::test]
    async fn failed_tools_are_logged_with_their_conversation_source() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let logger = StructuredLogger::new(directory.path()).unwrap();
        let runtime = runtime.with_logger(logger.clone());
        let scripts = vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "call-fail".into(),
                        name: "list_directory".into(),
                        arguments: json!({ "path": "missing-directory" }),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "done".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ];

        let outcome = runtime
            .run_turn(
                Arc::new(FakeProvider::script(scripts)),
                "fixture".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "inspect a missing directory".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        let logs = logger
            .read_logs(crate::logging::LogQuery {
                limit: None,
                level: Some("error".into()),
                event: Some("tool_failed".into()),
                after_timestamp_ms: None,
            })
            .unwrap();
        assert_eq!(logs.records.len(), 1);
        let record = &logs.records[0];
        assert_eq!(record.level, "error");
        // 日志原始 threadId 是来源对话的唯一标记，读取端据此关联对话名称。
        assert_eq!(record.thread_id.as_deref(), Some(thread_id.as_str()));
        assert_eq!(record.fields["threadId"], thread_id);
        assert_eq!(record.fields["tool"], "list_directory");
        // 调用与 Turn 标识只在会话事实事件里追得回来，运行日志只保留主要信息。
        assert!(record.fields.get("callId").is_none());
        assert!(record.fields.get("turnId").is_none());
        assert_eq!(record.fields["itemStatus"], "failed");
        // 事后查因要能判定是哪一条指令失败，失败调用的参数本体保留在日志里。
        assert_eq!(record.fields["arguments"]["path"], "missing-directory");
        let output = record.fields["output"].as_str().unwrap();
        assert!(output.contains("missing-directory"));
        assert!(output.len() <= MAX_TOOL_FAILURE_OUTPUT_BYTES);
        assert!(output.chars().count() <= crate::logging::MAX_FIELD_CHARS + 1);
    }

    #[tokio::test]
    async fn successful_tools_do_not_write_failure_logs() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let logger = StructuredLogger::new(directory.path()).unwrap();
        let runtime = runtime.with_logger(logger.clone());
        let scripts = vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "call-ok".into(),
                        name: "list_directory".into(),
                        arguments: json!({ "path": "." }),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "done".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ];

        let outcome = runtime
            .run_turn(
                Arc::new(FakeProvider::script(scripts)),
                "fixture".into(),
                RunTurnRequest {
                    thread_id,
                    input: "list the workspace root".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        let logs = logger
            .read_logs(crate::logging::LogQuery {
                limit: None,
                level: None,
                event: Some("tool_failed".into()),
                after_timestamp_ms: None,
            })
            .unwrap();
        assert!(logs.records.is_empty());
    }

    #[tokio::test]
    async fn provider_http_failures_are_logged_with_their_status_code() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let logger = StructuredLogger::new(directory.path()).unwrap();
        let runtime = runtime.with_logger(logger.clone());
        // 402 属于不可重试的 4xx：一次请求就直接终止，日志必须留下状态码。
        let provider = Arc::new(PreStreamProvider::new(vec![Err(ProviderError::Http {
            status: 402,
            message: "You exceeded your current quota".into(),
        })]));

        let outcome = runtime
            .run_turn(
                provider.clone(),
                "fixture".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "实现可点击状态面板".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        assert_eq!(provider.requests().len(), 1);
        let logs = logger
            .read_logs(crate::logging::LogQuery {
                limit: None,
                level: Some("error".into()),
                event: Some("provider_http_failed".into()),
                after_timestamp_ms: None,
            })
            .unwrap();
        assert_eq!(logs.records.len(), 1);
        let record = &logs.records[0];
        assert_eq!(record.fields["status"], 402);
        // 4xx 参数/额度问题不可自动重试，要与 5xx 可恢复失败区分开。
        assert_eq!(record.fields["retryable"], false);
        let message = record.fields["message"].as_str().unwrap();
        assert!(message.contains("402"), "{message}");
        assert!(message.contains("quota"), "{message}");
        assert_eq!(record.thread_id.as_deref(), Some(thread_id.as_str()));
    }

    #[tokio::test]
    async fn transient_http_failures_are_not_logged_until_the_retries_are_exhausted() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let logger = StructuredLogger::new(directory.path()).unwrap();
        let runtime = runtime.with_logger(logger.clone());
        // 503 是可重试的 5xx：前两次失败只写 Info 级 provider_rate_limited，
        // 第三次重试后成功，因此不应留下任何 Error 级 HTTP 失败记录。
        let provider = Arc::new(PreStreamProvider::new(vec![
            Err(ProviderError::Http {
                status: 503,
                message: "busy once".into(),
            }),
            Err(ProviderError::Http {
                status: 503,
                message: "busy twice".into(),
            }),
            Ok(vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "recovered".into(),
                }),
                Ok(ProviderEvent::Completed),
            ]),
        ]));

        let outcome = runtime
            .with_transient_retry_delays(vec![Duration::from_millis(1); 3])
            .run_turn(
                provider.clone(),
                "fixture".into(),
                RunTurnRequest {
                    thread_id,
                    input: "inspect the repository".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(provider.requests().len(), 3);
        let logs = logger
            .read_logs(crate::logging::LogQuery {
                limit: None,
                level: Some("error".into()),
                event: Some("provider_http_failed".into()),
                after_timestamp_ms: None,
            })
            .unwrap();
        assert!(
            logs.records.is_empty(),
            "retries succeeded, so no HTTP failure should be recorded: {:?}",
            logs.records
        );
    }

    #[tokio::test]
    async fn exhausted_transient_http_retries_record_the_final_status_code() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let logger = StructuredLogger::new(directory.path()).unwrap();
        let runtime = runtime.with_logger(logger.clone());
        let provider = Arc::new(PreStreamProvider::new(
            (0..4)
                .map(|_| {
                    Err(ProviderError::Http {
                        status: 503,
                        message: "服务繁忙，请稍后重试".into(),
                    })
                })
                .collect(),
        ));

        let outcome = runtime
            .with_transient_retry_delays(vec![Duration::from_millis(1); 3])
            .run_turn(
                provider.clone(),
                "fixture".into(),
                RunTurnRequest {
                    thread_id,
                    input: "inspect the repository".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        let logs = logger
            .read_logs(crate::logging::LogQuery {
                limit: None,
                level: Some("error".into()),
                event: Some("provider_http_failed".into()),
                after_timestamp_ms: None,
            })
            .unwrap();
        assert_eq!(logs.records.len(), 1);
        let record = &logs.records[0];
        assert_eq!(record.fields["status"], 503);
        assert_eq!(record.fields["retryable"], true);
        assert_eq!(
            record.fields["message"].as_str().unwrap(),
            "provider returned HTTP 503: 服务繁忙，请稍后重试"
        );
        assert_eq!(provider.requests().len(), 4);
    }

    #[tokio::test]
    async fn network_and_cancellation_failures_do_not_write_http_failure_logs() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let logger = StructuredLogger::new(directory.path()).unwrap();
        let runtime = runtime.with_logger(logger.clone());
        let network_logger = logger.clone();
        // 网络抖动只记 turn_failed 的通用失败，不冒充 HTTP 状态码失败。
        let provider = Arc::new(PreStreamProvider::new(vec![Err(ProviderError::Request(
            "connection reset by peer".into(),
        ))]));

        let outcome = runtime
            .run_turn(
                provider,
                "fixture".into(),
                RunTurnRequest {
                    thread_id,
                    input: "inspect the repository".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        // 主动取消同样不写：那是用户行为不是异常。
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let logger = StructuredLogger::new(directory.path()).unwrap();
        let runtime = runtime.with_logger(logger.clone());
        let cancelled_logger = logger.clone();
        let token = CancellationToken::new();
        token.cancel();
        let outcome = runtime
            .run_turn(
                Arc::new(FakeProvider::script(vec![vec![Ok(
                    ProviderEvent::TextDelta {
                        delta: "never runs".into(),
                    },
                )]])),
                "fixture".into(),
                RunTurnRequest {
                    thread_id,
                    input: "cancel me".into(),
                    agent_mode: None,
                },
                token,
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();
        assert_eq!(outcome.state, TurnState::Cancelled);

        for logger in [network_logger, cancelled_logger] {
            let logs = logger
                .read_logs(crate::logging::LogQuery {
                    limit: None,
                    level: None,
                    event: Some("provider_http_failed".into()),
                    after_timestamp_ms: None,
                })
                .unwrap();
            assert!(
                logs.records.is_empty(),
                "network shake and cancellation must not be recorded as HTTP failures: {:?}",
                logs.records
            );
        }
    }

    #[test]
    fn tool_failure_output_is_bounded_before_it_reaches_the_log_file() {
        let oversized = "x".repeat(MAX_TOOL_FAILURE_OUTPUT_BYTES);
        let bounded = truncate_utf8(&oversized, 8);
        assert_eq!(bounded, "xxxxxxxx");

        // 多字节字符必须落在字符边界上，截断不能产生无效 UTF-8。
        let multibyte = "汉字测试".repeat(64);
        let bounded = truncate_utf8(&multibyte, 10);
        assert!(bounded.len() <= 10);
        assert!(multibyte.starts_with(bounded));
        assert_eq!(bounded.chars().count(), 3);
    }

    #[tokio::test]
    async fn recovered_rate_limits_keep_bounded_diagnostics_for_both_provider_paths() {
        for wrapped in [false, true] {
            let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
            let logger = StructuredLogger::new(directory.path()).unwrap();
            let inner = Arc::new(PreStreamProvider::new(vec![
                Err(ProviderError::RateLimited {
                    message: format!(
                        "fixture capacity reached sk-fixture-private {}",
                        "长".repeat(2000)
                    ),
                    retry_after: Some(Duration::from_millis(1)),
                }),
                Ok(vec![
                    Ok(ProviderEvent::TextDelta {
                        delta: "recovered".into(),
                    }),
                    Ok(ProviderEvent::Completed),
                ]),
            ]));
            let provider: Arc<dyn Provider> = if wrapped {
                crate::providers::RateLimitRegistry::default().wrap("fixture", inner.clone())
            } else {
                inner.clone()
            };
            let outcome = runtime
                .with_logger(logger.clone())
                .run_turn(
                    provider,
                    "fixture".into(),
                    RunTurnRequest {
                        thread_id: thread_id.clone(),
                        input: "private user prompt marker".into(),
                        agent_mode: None,
                    },
                    CancellationToken::new(),
                    Arc::new(RecordingPublisher::default()),
                )
                .await
                .unwrap();
            assert_eq!(outcome.state, TurnState::Completed);
            assert_eq!(inner.requests().len(), 2);
            let logs = logger
                .read_logs(crate::logging::LogQuery {
                    limit: None,
                    level: None,
                    event: Some("provider_rate_limited".into()),
                    after_timestamp_ms: None,
                })
                .unwrap();
            assert_eq!(logs.records.len(), 1);
            let fields = &logs.records[0].fields;
            assert_eq!(fields["threadId"], thread_id);
            // Turn 标识不进运行日志，重试序号、退避与脱敏原因才是面板要读的主要信息。
            assert!(fields.get("turnId").is_none());
            assert_eq!(fields["retryNumber"], 1);
            assert_eq!(fields["retryDelayMs"], 1);
            assert_eq!(fields["delaySource"], "retry_after");
            let message = fields["message"].as_str().unwrap();
            assert!(message.contains("fixture capacity reached [REDACTED]"));
            assert!(message.chars().count() <= crate::logging::MAX_FIELD_CHARS + 1);
            let persisted =
                std::fs::read_to_string(directory.path().join("logs/runtime.jsonl")).unwrap();
            assert!(!persisted.contains("sk-fixture-private"));
            assert!(!persisted.contains("private user prompt marker"));
        }
    }

    #[tokio::test]
    async fn exhausted_rate_limits_are_typed_and_partial_output_is_never_replayed() {
        for partial in [false, true] {
            let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
            let mut events = vec![];
            if partial {
                events.push(Ok(ProviderEvent::TextDelta {
                    delta: "already visible".into(),
                }));
            }
            events.push(Err(ProviderError::RateLimited {
                message: "Free-tier request limit reached".into(),
                retry_after: Some(Duration::from_millis(1)),
            }));
            let provider = Arc::new(FakeProvider::new(events));
            let outcome = runtime
                .run_turn(
                    provider.clone(),
                    "fixture".into(),
                    RunTurnRequest {
                        thread_id: thread_id.clone(),
                        input: "test quota".into(),
                        agent_mode: None,
                    },
                    CancellationToken::new(),
                    Arc::new(RecordingPublisher::default()),
                )
                .await
                .unwrap();
            assert_eq!(outcome.state, TurnState::Failed);
            assert_eq!(provider.requests().len(), if partial { 1 } else { 4 });
            let history = repository.read_thread_history(&thread_id).await.unwrap();
            assert_eq!(
                history.last_turn.unwrap().error.unwrap().code,
                "rate_limited"
            );
        }
    }

    #[tokio::test]
    async fn quota_failure_continuation_and_retry_preserve_compacted_progress_and_saved_plan() {
        for retry in [false, true] {
            let (directory, repository, _, thread_id) = runtime_fixture().await;
            let advanced = crate::advanced::AdvancedServices::new(directory.path()).unwrap();
            let handlers = advanced
                .tool_handlers(directory.path())
                .0
                .into_iter()
                .filter(|handler| handler.definition().name == "update_plan")
                .collect();
            let tools = ToolRegistry::new(handlers).unwrap();
            let make_runtime = || {
                let plans = advanced.plans.clone();
                let id = thread_id.clone();
                AgentRuntime::with_tools(
                    repository.clone(),
                    tools.clone(),
                    directory.path().to_path_buf(),
                )
                .with_runtime_instruction_provider(Arc::new(move || {
                    plans.runtime_instructions(&id)
                }))
            };
            let plan_call = |id: &str, complete: bool| ProviderEvent::ToolCall {
                call: ToolCall {
                    id: id.into(),
                    name: "update_plan".into(),
                    metadata: json!({}),
                    arguments: json!({"steps": (1..=5).map(|i| json!({
                    "id": i.to_string(), "step": format!("状态面板步骤 {i}"),
                    "status": if complete { "completed" } else if i == 1 { "in_progress" } else { "pending" },
                })).collect::<Vec<_>>()}),
                },
            };
            let failed_provider = Arc::new(PreStreamProvider::new(vec![
                Ok(vec![
                    Ok(ProviderEvent::TextDelta {
                        delta: "组件已实现，构建已通过，现在补充 e2e 回归。".into(),
                    }),
                    Ok(plan_call("initial-plan", false)),
                    Ok(ProviderEvent::Completed),
                ]),
                Err(ProviderError::Http {
                    status: 402,
                    message: "You exceeded your current quota".into(),
                }),
            ]));
            let runtime = make_runtime();
            let outcome = runtime
                .run_turn(
                    failed_provider.clone(),
                    "fixture".into(),
                    RunTurnRequest {
                        thread_id: thread_id.clone(),
                        input: "实现可点击状态面板".into(),
                        agent_mode: Some("craft".into()),
                    },
                    CancellationToken::new(),
                    Arc::new(RecordingPublisher::default()),
                )
                .await
                .unwrap();
            assert_eq!(outcome.state, TurnState::Failed);
            assert_eq!(failed_provider.requests().len(), 2); // No automatic quota retry.
            runtime.compact_thread(&thread_id).await.unwrap();
            drop(runtime);
            let resumed_provider = Arc::new(PreStreamProvider::new(vec![
                Ok(vec![
                    Ok(plan_call("reconciled-plan", true)),
                    Ok(ProviderEvent::Completed),
                ]),
                Ok(vec![
                    Ok(ProviderEvent::TextDelta {
                        delta: "剩余验证已完成。".into(),
                    }),
                    Ok(ProviderEvent::Completed),
                ]),
            ]));
            let runtime = make_runtime();
            let outcome = if retry {
                runtime
                    .retry_turn(
                        resumed_provider.clone(),
                        "fixture".into(),
                        thread_id.clone(),
                        AgentMode::Craft,
                        CancellationToken::new(),
                        Arc::new(RecordingPublisher::default()),
                    )
                    .await
            } else {
                runtime
                    .run_turn(
                        resumed_provider.clone(),
                        "fixture".into(),
                        RunTurnRequest {
                            thread_id: thread_id.clone(),
                            input: "继续".into(),
                            agent_mode: Some("craft".into()),
                        },
                        CancellationToken::new(),
                        Arc::new(RecordingPublisher::default()),
                    )
                    .await
            }
            .unwrap();
            assert_eq!(outcome.state, TurnState::Completed);
            let first = serde_json::to_string(&resumed_provider.requests()[0]).unwrap();
            assert!(first.contains("<interrupted_task_continuation>"));
            if retry {
                assert!(
                    first.contains("<retry_continuation_request>"),
                    "manual retry must carry a non-persistent continuation request"
                );
                assert!(first.contains("继续上一次未完成任务"));
            }
            assert!(first.contains("实现可点击状态面板"));
            assert!(first.contains("现在补充 e2e 回归"));
            assert!(first.contains("状态面板步骤 5"));
            assert!(first.contains("in_progress"));
            assert!(
                advanced
                    .plans
                    .get(&thread_id)
                    .unwrap()
                    .unwrap()
                    .steps
                    .iter()
                    .all(|step| step.status == crate::advanced::PlanStepState::Completed)
            );
            let events = repository.load(&thread_id).await.unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event.kind, StoredEventKind::UserMessage { .. }))
                    .count(),
                if retry { 1 } else { 2 }
            );

            let next = Arc::new(FakeProvider::text(&["新问题已回答"]));
            runtime
                .run_turn(
                    next.clone(),
                    "fixture".into(),
                    RunTurnRequest {
                        thread_id: thread_id.clone(),
                        input: "现在回答另一问题".into(),
                        agent_mode: None,
                    },
                    CancellationToken::new(),
                    Arc::new(RecordingPublisher::default()),
                )
                .await
                .unwrap();
            assert!(
                !serde_json::to_string(&next.requests()[0])
                    .unwrap()
                    .contains("<interrupted_task_continuation>")
            );
        }
    }

    #[tokio::test]
    async fn transient_pre_stream_failures_retry_the_same_request_then_succeed() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let metrics = RuntimeMetrics::new(directory.path()).unwrap();
        let provider = Arc::new(PreStreamProvider::new(vec![
            Err(ProviderError::Http {
                status: 503,
                message: "busy once".into(),
            }),
            Err(ProviderError::Http {
                status: 503,
                message: "busy twice".into(),
            }),
            Ok(vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "recovered".into(),
                }),
                Ok(ProviderEvent::Completed),
            ]),
        ]));

        let outcome = runtime
            .with_metrics(metrics.clone())
            .with_transient_retry_delays(vec![Duration::from_millis(1); 3])
            .run_turn(
                provider.clone(),
                "deepseek-v4-flash-0731".into(),
                RunTurnRequest {
                    thread_id,
                    input: "inspect the repository".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        let requests = provider.requests();
        assert_eq!(requests.len(), 3);
        assert!(requests.windows(2).all(|pair| pair[0] == pair[1]));
        let snapshot = metrics.snapshot().unwrap();
        assert_eq!(snapshot.provider_calls, 3);
        assert_eq!(snapshot.provider_failures, 2);
        assert_eq!(snapshot.retry_count, 2);
    }

    #[tokio::test]
    async fn transient_pre_stream_retry_exhaustion_preserves_the_http_failure() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let metrics = RuntimeMetrics::new(directory.path()).unwrap();
        let provider = Arc::new(PreStreamProvider::new(
            (0..4)
                .map(|_| {
                    Err(ProviderError::Http {
                        status: 503,
                        message: "服务繁忙，请稍后重试".into(),
                    })
                })
                .collect(),
        ));

        let outcome = runtime
            .with_metrics(metrics.clone())
            .with_transient_retry_delays(vec![Duration::from_millis(1); 3])
            .run_turn(
                provider.clone(),
                "deepseek-v4-flash-0731".into(),
                RunTurnRequest {
                    thread_id,
                    input: "inspect the repository".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        let error = outcome.error.unwrap();
        assert!(error.contains("provider returned HTTP 503"));
        assert!(error.contains("已自动重试 3 次"));
        assert_eq!(provider.requests().len(), 4);
        let snapshot = metrics.snapshot().unwrap();
        assert_eq!(snapshot.provider_calls, 4);
        assert_eq!(snapshot.provider_failures, 4);
        assert_eq!(snapshot.retry_count, 3);
    }

    #[tokio::test]
    async fn authentication_and_request_validation_failures_are_not_retried() {
        for status in [400, 401] {
            let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
            let provider = Arc::new(PreStreamProvider::new(vec![
                Err(ProviderError::Http {
                    status,
                    message: "do not replay".into(),
                }),
                Ok(vec![
                    Ok(ProviderEvent::TextDelta {
                        delta: "must not run".into(),
                    }),
                    Ok(ProviderEvent::Completed),
                ]),
            ]));

            let outcome = runtime
                .with_transient_retry_delays(vec![Duration::from_millis(1); 3])
                .run_turn(
                    provider.clone(),
                    "deepseek-v4-flash-0731".into(),
                    RunTurnRequest {
                        thread_id,
                        input: "do not retry an invalid request".into(),
                        agent_mode: None,
                    },
                    CancellationToken::new(),
                    Arc::new(RecordingPublisher::default()),
                )
                .await
                .unwrap();

            assert_eq!(outcome.state, TurnState::Failed);
            assert_eq!(provider.requests().len(), 1, "HTTP {status} was replayed");
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_transient_retry_backoff_without_another_request() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(PreStreamProvider::new(vec![
            Err(ProviderError::Http {
                status: 503,
                message: "busy".into(),
            }),
            Ok(vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "must not run".into(),
                }),
                Ok(ProviderEvent::Completed),
            ]),
        ]));
        let cancellation = CancellationToken::new();
        let task_provider = provider.clone();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            runtime
                .with_transient_retry_delays(vec![Duration::from_secs(30)])
                .run_turn(
                    task_provider,
                    "deepseek-v4-flash-0731".into(),
                    RunTurnRequest {
                        thread_id,
                        input: "cancel while waiting".into(),
                        agent_mode: None,
                    },
                    task_cancellation,
                    Arc::new(RecordingPublisher::default()),
                )
                .await
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            while provider.requests().is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        cancellation.cancel();
        let outcome = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("cancellation should interrupt backoff")
            .unwrap()
            .unwrap();

        assert_eq!(outcome.state, TurnState::Cancelled);
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn transient_stream_error_before_output_retries_then_succeeds() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::script(vec![
            vec![Err(ProviderError::Unavailable("servers overloaded".into()))],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "recovered".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));

        let outcome = runtime
            .with_transient_retry_delays(vec![Duration::from_millis(1); 3])
            .run_turn(
                provider.clone(),
                "deepseek-v4-flash-0731".into(),
                RunTurnRequest {
                    thread_id,
                    input: "retry an overloaded empty stream".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(provider.requests().len(), 2);
    }

    #[tokio::test]
    async fn transient_stream_error_after_output_is_not_retried() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "partial".into(),
                }),
                Err(ProviderError::Unavailable("servers overloaded".into())),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "must not run".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));

        let outcome = runtime
            .with_transient_retry_delays(vec![Duration::from_millis(1); 3])
            .run_turn(
                provider.clone(),
                "gpt-5".into(),
                RunTurnRequest {
                    thread_id,
                    input: "do not replay emitted output".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        assert_eq!(provider.requests().len(), 1);
    }

    /// 每个 outcome 是一组先行事件 + 是否在之后永久挂起（模拟流静默卡死）。
    struct IdleStreamProvider {
        outcomes: Mutex<VecDeque<(Vec<Result<ProviderEvent, ProviderError>>, bool)>>,
        requests: Mutex<Vec<ProviderRequest>>,
    }

    impl IdleStreamProvider {
        fn new(outcomes: Vec<(Vec<Result<ProviderEvent, ProviderError>>, bool)>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<ProviderRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Provider for IdleStreamProvider {
        async fn stream(
            &self,
            request: ProviderRequest,
            _cancellation: CancellationToken,
        ) -> Result<crate::providers::ProviderStream, ProviderError> {
            self.requests.lock().unwrap().push(request);
            match self.outcomes.lock().unwrap().pop_front() {
                Some((events, hang_after)) => {
                    if hang_after {
                        Ok(Box::pin(
                            futures_util::stream::iter(events)
                                .chain(futures_util::stream::pending()),
                        ))
                    } else {
                        Ok(Box::pin(futures_util::stream::iter(events)))
                    }
                }
                None => Err(ProviderError::InvalidResponse(
                    "idle stream test provider ran out of outcomes".into(),
                )),
            }
        }
    }

    #[tokio::test]
    async fn stream_idle_timeout_retries_visibly_then_succeeds() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let publisher = Arc::new(RecordingPublisher::default());
        let provider = Arc::new(IdleStreamProvider::new(vec![
            (vec![], true),
            (
                vec![
                    Ok(ProviderEvent::TextDelta {
                        delta: "recovered".into(),
                    }),
                    Ok(ProviderEvent::Completed),
                ],
                false,
            ),
        ]));

        let outcome = runtime
            .with_stream_idle_timeout(Duration::from_millis(20))
            .with_transient_retry_delays(vec![Duration::from_millis(1); 3])
            .run_turn(
                provider.clone(),
                "gpt-5".into(),
                RunTurnRequest {
                    thread_id,
                    input: "retry a stalled stream".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(provider.requests().len(), 2);
        let events = publisher.events.lock().unwrap();
        assert!(
            events.iter().any(|event| matches!(
                &event.event,
                AgentEvent::ProviderStreamRetry {
                    attempt: 1,
                    max_attempts: 3,
                    ..
                }
            )),
            "idle timeout should publish a visible stream retry event"
        );
    }

    #[tokio::test]
    async fn stream_idle_timeout_after_output_fails_without_replay() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let publisher = Arc::new(RecordingPublisher::default());
        let provider = Arc::new(IdleStreamProvider::new(vec![(
            vec![Ok(ProviderEvent::TextDelta {
                delta: "partial".into(),
            })],
            true,
        )]));

        let outcome = runtime
            .with_stream_idle_timeout(Duration::from_millis(20))
            .with_transient_retry_delays(vec![Duration::from_millis(1); 3])
            .run_turn(
                provider.clone(),
                "gpt-5".into(),
                RunTurnRequest {
                    thread_id,
                    input: "do not replay a stalled partial response".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        assert_eq!(provider.requests().len(), 1);
        let error = outcome.error.expect("stalled stream should fail the turn");
        assert!(error.contains("idle timeout"));
        let events = publisher.events.lock().unwrap();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.event, AgentEvent::ProviderStreamRetry { .. })),
            "output already started must not publish retry events"
        );
    }

    #[tokio::test]
    async fn transient_stream_retry_exhaustion_preserves_last_error_and_retry_count() {
        let (directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let metrics = RuntimeMetrics::new(directory.path()).unwrap();
        let provider = Arc::new(FakeProvider::script(
            (0..4)
                .map(|attempt| {
                    vec![Err(ProviderError::Unavailable(format!(
                        "servers overloaded on attempt {}",
                        attempt + 1
                    )))]
                })
                .collect(),
        ));

        let outcome = runtime
            .with_metrics(metrics.clone())
            .with_transient_retry_delays(vec![Duration::from_millis(1); 3])
            .run_turn(
                provider.clone(),
                "gpt-5".into(),
                RunTurnRequest {
                    thread_id,
                    input: "retry temporary server overloads".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        let error = outcome
            .error
            .expect("the failed turn should retain an error");
        assert!(error.contains("servers overloaded on attempt 4"));
        assert!(error.contains("已自动重试 3 次"));
        assert_eq!(provider.requests().len(), 4);
        let snapshot = metrics.snapshot().unwrap();
        assert_eq!(snapshot.provider_calls, 4);
        assert_eq!(snapshot.provider_failures, 4);
        assert_eq!(snapshot.retry_count, 3);
    }

    #[tokio::test]
    async fn invalid_tool_arguments_before_stream_retry_the_same_request() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(PreStreamProvider::new(vec![
            Err(ProviderError::InvalidToolArguments(
                "trailing characters".into(),
            )),
            Ok(vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "recovered".into(),
                }),
                Ok(ProviderEvent::Completed),
            ]),
        ]));
        let outcome = runtime
            .run_turn(
                provider.clone(),
                "fake".into(),
                RunTurnRequest {
                    thread_id,
                    input: "retry malformed arguments".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();
        assert_eq!(outcome.state, TurnState::Completed);
        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].messages, requests[1].messages);
    }

    #[tokio::test]
    async fn cancelling_invalid_tool_argument_backoff_prevents_another_request() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::new(vec![Err(
            ProviderError::InvalidToolArguments("trailing characters".into()),
        )]));
        let cancellation = CancellationToken::new();
        let run_provider = provider.clone();
        let run_cancellation = cancellation.clone();
        let run = tokio::spawn(async move {
            runtime
                .run_turn(
                    run_provider,
                    "fake".into(),
                    RunTurnRequest {
                        thread_id,
                        input: "cancel retry".into(),
                        agent_mode: None,
                    },
                    run_cancellation,
                    Arc::new(RecordingPublisher::default()),
                )
                .await
                .unwrap()
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while provider.requests().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancellation.cancel();
        assert_eq!(run.await.unwrap().state, TurnState::Cancelled);
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn invalid_tool_arguments_exhaust_retry_budget_then_allow_manual_retry() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::script(
            (0..=MAX_PROTOCOL_RETRIES)
                .map(|_| {
                    vec![Err(ProviderError::InvalidToolArguments(
                        "trailing characters".into(),
                    ))]
                })
                .collect(),
        ));
        let failed = runtime
            .run_turn(
                provider.clone(),
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "ask a question".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();
        assert_eq!(failed.state, TurnState::Failed);
        assert_eq!(provider.requests().len(), MAX_PROTOCOL_RETRIES + 1);
        let history = repository.read_thread_history(&thread_id).await.unwrap();
        let error = history.last_turn.unwrap().error.unwrap();
        assert_eq!(error.code, "provider_invalid_response");
        assert!(error.retryable);
        assert!(error.message.contains("已重试 5 次"));
        assert_eq!(
            error.details,
            Some(json!({ "protocolRetries": 5, "outputAlreadyStarted": false }))
        );
        let completed = runtime
            .retry_turn(
                Arc::new(FakeProvider::text(&["recovered"])),
                "fake".into(),
                thread_id.clone(),
                AgentMode::Craft,
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();
        assert_eq!(completed.state, TurnState::Completed);
        assert_eq!(
            repository
                .read_thread(&thread_id)
                .await
                .unwrap()
                .messages
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn retryable_provider_failure_counts_failed_attempt_usage() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::Usage {
                    usage: TokenUsage {
                        input_tokens: 3,
                        output_tokens: 2,
                        total_tokens: 5,
                    },
                }),
                Err(ProviderError::InvalidToolArguments(
                    "function call returned invalid JSON arguments".into(),
                )),
            ],
            vec![
                Ok(ProviderEvent::Usage {
                    usage: TokenUsage {
                        input_tokens: 4,
                        output_tokens: 3,
                        total_tokens: 7,
                    },
                }),
                Ok(ProviderEvent::TextDelta {
                    delta: "complete".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));

        let outcome = runtime
            .run_turn(
                provider.clone(),
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "retry safely".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(provider.requests().len(), 2);
        let usage_events = repository
            .load(&thread_id)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.kind {
                StoredEventKind::ProviderCallUsage {
                    call_index, usage, ..
                } => Some((call_index, usage.total_tokens)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(usage_events, vec![(0, 5), (1, 7)]);
        assert_eq!(
            repository
                .read_thread(&thread_id)
                .await
                .unwrap()
                .last_usage
                .unwrap()
                .total_tokens,
            12
        );
    }

    #[tokio::test]
    async fn provider_usage_persists_breakdown_and_selected_model() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let details = crate::protocol::TokenUsageDetails {
            cached_input_tokens: Some(40),
            uncached_input_tokens: Some(60),
            cache_write_input_tokens: None,
            reasoning_output_tokens: Some(10),
        };
        let provider = Arc::new(FakeProvider::script(vec![vec![
            Ok(ProviderEvent::ModelSelected {
                provider: "openai".into(),
                model: "gpt-test".into(),
            }),
            Ok(ProviderEvent::DetailedUsage {
                usage: TokenUsage {
                    input_tokens: 100,
                    output_tokens: 30,
                    total_tokens: 130,
                },
                details,
            }),
            Ok(ProviderEvent::Usage {
                usage: TokenUsage {
                    input_tokens: 100,
                    output_tokens: 30,
                    total_tokens: 130,
                },
            }),
            Ok(ProviderEvent::TextDelta {
                delta: "complete".into(),
            }),
            Ok(ProviderEvent::Completed),
        ]]));

        runtime
            .run_turn(
                provider,
                "configured-model".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "track detailed usage".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        let usage = repository
            .load(&thread_id)
            .await
            .unwrap()
            .into_iter()
            .find_map(|event| match event.kind {
                StoredEventKind::ProviderCallUsage {
                    usage,
                    details,
                    provider,
                    model,
                    ..
                } => Some((usage, details, provider, model)),
                _ => None,
            })
            .expect("the provider call should persist usage");
        assert_eq!(usage.0.total_tokens, 130);
        assert_eq!(usage.1, details);
        assert_eq!(usage.2.as_deref(), Some("openai"));
        assert_eq!(usage.3.as_deref(), Some("gpt-test"));

        let summary = repository.projection().usage_summary().unwrap();
        assert_eq!(summary.total_tokens, 130);
        assert_eq!(summary.cached_input_tokens, Some(40));
        assert_eq!(summary.reasoning_output_tokens, Some(10));
        assert_eq!(summary.models.len(), 1);
        assert_eq!(summary.models[0].provider.as_deref(), Some("openai"));
        assert_eq!(summary.models[0].model.as_deref(), Some("gpt-test"));
    }

    #[tokio::test]
    async fn provider_failure_after_visible_output_is_not_retried() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::script(vec![vec![
            Ok(ProviderEvent::TextDelta {
                delta: "partial".into(),
            }),
            Ok(ProviderEvent::Usage {
                usage: TokenUsage {
                    input_tokens: 3,
                    output_tokens: 2,
                    total_tokens: 5,
                },
            }),
            Err(ProviderError::InvalidToolArguments(
                "function call returned invalid JSON arguments".into(),
            )),
        ]]));

        let publisher = Arc::new(RecordingPublisher::default());
        let outcome = runtime
            .run_turn(
                provider.clone(),
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "do not duplicate output".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        assert_eq!(provider.requests().len(), 1);
        assert!(
            publisher
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(
                    &event.event,
                    AgentEvent::ItemCompleted {
                        item_type: AgentItemType::AgentMessage,
                        status: AgentItemStatus::Failed,
                        ..
                    }
                ))
        );
        assert_eq!(
            repository
                .read_thread(&thread_id)
                .await
                .unwrap()
                .last_usage
                .unwrap()
                .total_tokens,
            5
        );
    }

    #[tokio::test]
    async fn explicit_token_budget_still_stops_a_turn() {
        let (_directory, _repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::new(vec![
            Ok(ProviderEvent::Usage {
                usage: TokenUsage {
                    input_tokens: 900,
                    output_tokens: 101,
                    total_tokens: 1_001,
                },
            }),
            Ok(ProviderEvent::Completed),
        ]));

        let outcome = runtime
            .with_token_budget(1_000)
            .run_turn(
                provider,
                "fake".to_string(),
                RunTurnRequest {
                    thread_id,
                    input: "bounded task".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Failed);
        assert_eq!(
            outcome.error.as_deref(),
            Some("token_budget_exceeded: used 1001 of 1000 tokens")
        );
    }

    #[tokio::test]
    async fn provider_context_usage_triggers_mid_turn_compaction() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        for index in 0..20 {
            let role = if index % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            repository
                .append(StoredEvent::new(
                    &thread_id,
                    None,
                    StoredEventKind::AssistantMessage {
                        message: text_message(
                            role,
                            format!("history-{index} {}", "x".repeat(1_000)),
                        ),
                    },
                ))
                .await
                .unwrap();
        }
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::Usage {
                    usage: TokenUsage {
                        input_tokens: 8_800,
                        output_tokens: 200,
                        total_tokens: 9_000,
                    },
                }),
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "context-pressure-call".to_string(),
                        name: "list_directory".to_string(),
                        arguments: json!({ "path": "." }),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::Usage {
                    usage: TokenUsage {
                        input_tokens: 150,
                        output_tokens: 50,
                        total_tokens: 200,
                    },
                }),
                Ok(ProviderEvent::TextDelta {
                    delta: "Compacted and completed".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));

        let publisher = Arc::new(RecordingPublisher::default());
        let outcome = runtime
            .with_context_limit(10_000)
            .run_turn(
                provider.clone(),
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "continue after compaction".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        let events = repository.load(&thread_id).await.unwrap();
        let compaction_item_id = events
            .iter()
            .find_map(|event| match &event.kind {
                StoredEventKind::ContextCompacted {
                    automatic: true, ..
                } => Some(event.event_id.clone()),
                _ => None,
            })
            .expect("automatic compaction should be persisted");
        let stored_lifecycle = events
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::ItemStarted { item_id, item_type }
                    if *item_type == AgentItemType::ContextCompaction =>
                {
                    Some((item_id.clone(), None))
                }
                StoredEventKind::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                } if *item_type == AgentItemType::ContextCompaction => {
                    Some((item_id.clone(), Some(*status)))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stored_lifecycle,
            [
                (compaction_item_id.clone(), None),
                (compaction_item_id.clone(), Some(AgentItemStatus::Completed),),
            ]
        );
        assert!(
            publisher
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(
                    &event.event,
                    AgentEvent::ContextCompacted {
                        automatic: true,
                        compacted_message_count,
                        ..
                    } if *compacted_message_count > 0
                ))
        );
        let published_lifecycle = publisher
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match &event.event {
                AgentEvent::ItemStarted {
                    item_id, item_type, ..
                } if *item_type == AgentItemType::ContextCompaction => {
                    Some((item_id.clone(), None))
                }
                AgentEvent::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                    ..
                } if *item_type == AgentItemType::ContextCompaction => {
                    Some((item_id.clone(), Some(*status)))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(published_lifecycle, stored_lifecycle);
        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].messages.len() < requests[0].messages.len());
    }

    #[tokio::test]
    async fn manual_compaction_persists_a_completed_item_lifecycle() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        for index in 0..20 {
            let role = if index % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            repository
                .append(StoredEvent::new(
                    &thread_id,
                    None,
                    StoredEventKind::AssistantMessage {
                        message: text_message(
                            role,
                            format!("manual-history-{index} {}", "x".repeat(1_000)),
                        ),
                    },
                ))
                .await
                .unwrap();
        }

        let summary = runtime
            .with_context_limit(2_000)
            .compact_thread(&thread_id)
            .await
            .unwrap();
        assert!(summary.compacted_message_count > 0);
        let events = repository.load(&thread_id).await.unwrap();
        let compaction_item_id = events
            .iter()
            .find_map(|event| match &event.kind {
                StoredEventKind::ContextCompacted {
                    automatic: false, ..
                } => Some(event.event_id.clone()),
                _ => None,
            })
            .expect("manual compaction should be persisted");
        let lifecycle = events
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::ItemStarted { item_id, item_type }
                    if *item_type == AgentItemType::ContextCompaction =>
                {
                    Some((event.turn_id.clone(), item_id.clone(), None))
                }
                StoredEventKind::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                } if *item_type == AgentItemType::ContextCompaction => {
                    Some((event.turn_id.clone(), item_id.clone(), Some(*status)))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lifecycle,
            [
                (None, compaction_item_id.clone(), None),
                (None, compaction_item_id, Some(AgentItemStatus::Completed),),
            ]
        );
    }

    #[test]
    fn restored_compaction_renders_recent_tools_as_text() {
        let summary = CompactionSummary {
            contract_version: 2,
            summary: "repository inspected".to_string(),
            user_constraints: Vec::new(),
            recent_user_messages: Vec::new(),
            current_user_request: String::new(),
            user_clarifications: Vec::new(),
            recent_assistant_progress: Vec::new(),
            important_tool_observations: Vec::new(),
            recent_tool_results: vec![ProviderMessage::ToolResult {
                call_id: "orphaned-call".to_string(),
                name: "read_file".to_string(),
                success: true,
                output: "important result".to_string(),
            }],
            compacted_message_count: 10,
            estimated_before_tokens: 0,
            estimated_after_tokens: 0,
        };
        let history = provider_history(
            vec![StoredEvent::new(
                "thread",
                None,
                StoredEventKind::ContextCompacted {
                    summary,
                    automatic: true,
                },
            )],
            false,
        )
        .request_messages();

        assert!(matches!(
            history.as_slice(),
            [ProviderMessage::Text { text, .. }]
                if text.contains("tool read_file (true): important result")
        ));
    }

    #[test]
    fn restored_repeated_compaction_recovers_real_user_events() {
        let recursive_summary = CompactionSummary {
            contract_version: 3,
            summary: "User: [Compacted context v3]\nSummary:\nrecursive".to_string(),
            user_constraints: vec!["[Compacted context v3]\n必须递归".to_string()],
            recent_user_messages: vec!["[Compacted context v3]".to_string()],
            current_user_request: "[Compacted context v3]".to_string(),
            user_clarifications: Vec::new(),
            recent_assistant_progress: Vec::new(),
            important_tool_observations: vec!["tool read_file: inspected settings".to_string()],
            recent_tool_results: Vec::new(),
            compacted_message_count: 20,
            estimated_before_tokens: 60_000,
            estimated_after_tokens: 4_000,
        };
        let events = vec![
            StoredEvent::new(
                "thread",
                None,
                StoredEventKind::UserMessage {
                    message: user_message(
                        "请设计 workflow 设置并对照参考实现".to_string(),
                        Vec::new(),
                        false,
                    )
                    .unwrap(),
                },
            ),
            StoredEvent::new(
                "thread",
                Some("turn-1".to_string()),
                StoredEventKind::ContextCompacted {
                    summary: recursive_summary.clone(),
                    automatic: true,
                },
            ),
            StoredEvent::new(
                "thread",
                None,
                StoredEventKind::UserMessage {
                    message: user_message("怎么停了，继续".to_string(), Vec::new(), false).unwrap(),
                },
            ),
            StoredEvent::new(
                "thread",
                Some("turn-2".to_string()),
                StoredEventKind::ContextCompacted {
                    summary: recursive_summary,
                    automatic: true,
                },
            ),
        ];

        let history = provider_history(events, false);
        let summary = history
            .summary()
            .expect("compaction summary should be restored");
        let request = history.request_messages();
        let rendered = match request.first() {
            Some(ProviderMessage::Text { text, .. }) => text,
            other => panic!("expected rendered compaction summary, got {other:?}"),
        };

        assert!(summary.summary.is_empty());
        assert_eq!(summary.current_user_request, "怎么停了，继续");
        assert_eq!(
            summary.recent_user_messages,
            ["请设计 workflow 设置并对照参考实现", "怎么停了，继续"]
        );
        assert!(summary.user_constraints.is_empty());
        assert_eq!(rendered.matches("[Compacted context v3]").count(), 1);
        assert!(rendered.contains("请设计 workflow 设置并对照参考实现"));
        assert!(rendered.contains("怎么停了，继续"));
    }

    #[test]
    fn provider_history_repairs_incomplete_persisted_tool_groups() {
        let calls = vec![
            ToolCall {
                id: "call-a".to_string(),
                name: "read_file".to_string(),
                arguments: json!({ "path": "a.md" }),
                metadata: json!({}),
            },
            ToolCall {
                id: "call-b".to_string(),
                name: "read_file".to_string(),
                arguments: json!({ "path": "b.md" }),
                metadata: json!({}),
            },
        ];
        let history = provider_history(
            vec![
                StoredEvent::new(
                    "thread",
                    Some("interrupted-turn".to_string()),
                    StoredEventKind::AssistantToolCalls {
                        item_id: None,
                        text: "Inspecting files".to_string(),
                        calls,
                    },
                ),
                StoredEvent::new(
                    "thread",
                    Some("interrupted-turn".to_string()),
                    StoredEventKind::ToolResult {
                        call_id: "call-a".to_string(),
                        name: "read_file".to_string(),
                        result: ToolResult {
                            success: true,
                            output: "a".to_string(),
                            metadata: json!({}),
                        },
                    },
                ),
            ],
            false,
        )
        .request_messages();

        assert!(matches!(
            history.as_slice(),
            [ProviderMessage::Text { role: MessageRole::Assistant, text }]
                if text == "Inspecting files"
        ));
    }

    #[tokio::test]
    async fn recovered_turn_sends_only_complete_tool_groups_to_provider() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let interrupted_turn_id = "interrupted-turn".to_string();
        repository
            .append(StoredEvent::new(
                &thread_id,
                Some(interrupted_turn_id.clone()),
                StoredEventKind::AssistantToolCalls {
                    item_id: None,
                    text: "Inspecting files".to_string(),
                    calls: vec![
                        ToolCall {
                            id: "call-a".to_string(),
                            name: "read_file".to_string(),
                            arguments: json!({ "path": "a.md" }),
                            metadata: json!({}),
                        },
                        ToolCall {
                            id: "call-b".to_string(),
                            name: "read_file".to_string(),
                            arguments: json!({ "path": "b.md" }),
                            metadata: json!({}),
                        },
                    ],
                },
            ))
            .await
            .unwrap();
        repository
            .append(StoredEvent::new(
                &thread_id,
                Some(interrupted_turn_id),
                StoredEventKind::ToolResult {
                    call_id: "call-a".to_string(),
                    name: "read_file".to_string(),
                    result: ToolResult {
                        success: true,
                        output: "a".to_string(),
                        metadata: json!({}),
                    },
                },
            ))
            .await
            .unwrap();

        let provider = Arc::new(FakeProvider::text(&["Recovered"]));
        let outcome = runtime
            .run_turn(
                provider.clone(),
                "fake".to_string(),
                RunTurnRequest {
                    thread_id,
                    input: "continue".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert!(
            provider.requests()[0]
                .messages
                .iter()
                .all(|message| !matches!(
                    message,
                    ProviderMessage::AssistantToolCalls { .. } | ProviderMessage::ToolResult { .. }
                ))
        );
        assert!(matches!(
            provider.requests()[0].messages.as_slice(),
            [
                ProviderMessage::Text { role: MessageRole::Assistant, text },
                ProviderMessage::Text { role: MessageRole::User, text: user_text }
            ] if text == "Inspecting files" && user_text == "continue"
        ));
    }

    #[tokio::test]
    async fn fails_a_repeated_identical_tool_loop_with_a_typed_reason() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let response = || {
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: Uuid::new_v4().to_string(),
                        name: "list_directory".to_string(),
                        arguments: json!({ "path": "." }),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ]
        };
        let provider = Arc::new(FakeProvider::script(vec![
            response(),
            response(),
            response(),
        ]));
        let outcome = runtime
            .run_turn(
                provider,
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "loop".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();
        assert_eq!(outcome.state, TurnState::Failed);
        assert!(outcome.error.unwrap().contains("repeated_tool_call"));
        assert_eq!(
            repository
                .load(&thread_id)
                .await
                .unwrap()
                .iter()
                .filter(|event| matches!(event.kind, StoredEventKind::ToolResult { .. }))
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn allows_rechecking_identical_arguments_after_an_intervening_tool_call() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let list = |id: &str, limit: Option<usize>| {
            let mut arguments = json!({ "path": "." });
            if let Some(limit) = limit {
                arguments["limit"] = json!(limit);
            }
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: id.to_string(),
                        name: "list_directory".to_string(),
                        arguments,
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ]
        };
        let provider = Arc::new(FakeProvider::script(vec![
            list("initial-check", None),
            list("intervening-check", Some(1)),
            list("final-check", None),
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "verified".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));

        let outcome = runtime
            .run_turn(
                provider,
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "check, work, and check again".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(
            repository
                .load(&thread_id)
                .await
                .unwrap()
                .iter()
                .filter(|event| matches!(event.kind, StoredEventKind::ToolResult { .. }))
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn cancellation_is_persisted_and_published() {
        let (_directory, repository, runtime, thread_id) = runtime_fixture().await;
        let provider = Arc::new(FakeProvider::text(&["late"]).with_delay(Duration::from_secs(10)));
        let cancellation = CancellationToken::new();
        let cancel_from_test = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_from_test.cancel();
        });
        let publisher = Arc::new(RecordingPublisher::default());
        let result = runtime
            .run_turn(
                provider,
                "fake-model".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "wait".to_string(),
                    agent_mode: None,
                },
                cancellation,
                publisher.clone(),
            )
            .await
            .unwrap();
        assert_eq!(result.state, TurnState::Cancelled);
        let events = repository.load(&thread_id).await.unwrap();
        assert!(matches!(
            events.last().map(|event| &event.kind),
            Some(StoredEventKind::TurnCancelled)
        ));
        assert!(matches!(
            events.iter().find_map(|event| match &event.kind {
                StoredEventKind::ItemCompleted { status, .. } => Some(status),
                _ => None,
            }),
            Some(AgentItemStatus::Cancelled)
        ));
        assert!(
            publisher
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(
                    &event.event,
                    AgentEvent::ItemCompleted {
                        item_type: AgentItemType::AgentMessage,
                        status: AgentItemStatus::Cancelled,
                        ..
                    }
                ))
        );
    }

    #[tokio::test]
    async fn tool_cancellation_completes_every_persisted_call_result() {
        let directory = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(directory.path()).unwrap());
        let thread = repository.create_thread().await.unwrap();
        let tools = ToolRegistry::new(vec![Arc::new(SlowTool)]).unwrap();
        let runtime =
            AgentRuntime::with_tools(repository.clone(), tools, directory.path().to_path_buf());
        let calls = ["call-1", "call-2"]
            .into_iter()
            .map(|id| {
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: id.to_string(),
                        name: "slow_read".to_string(),
                        arguments: json!({}),
                        metadata: json!({}),
                    },
                })
            })
            .chain(std::iter::once(Ok(ProviderEvent::Completed)))
            .collect();
        let provider = Arc::new(FakeProvider::new(calls));
        let cancellation = CancellationToken::new();
        let publisher = Arc::new(CancellingPublisher {
            cancellation: cancellation.clone(),
            started_calls: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
        });

        let outcome = runtime
            .run_turn(
                provider,
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread.id.clone(),
                    input: "cancel tools".to_string(),
                    agent_mode: None,
                },
                cancellation,
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Cancelled);
        let events = repository.load(&thread.id).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind, StoredEventKind::ToolResult { .. }))
                .count(),
            2
        );
        let tool_lifecycle = events
            .iter()
            .filter_map(|event| match &event.kind {
                StoredEventKind::ItemStarted { item_id, item_type }
                    if *item_type == AgentItemType::Tool =>
                {
                    Some((item_id.clone(), None))
                }
                StoredEventKind::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                } if *item_type == AgentItemType::Tool => Some((item_id.clone(), Some(*status))),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tool_lifecycle,
            [
                ("call-1".to_string(), None),
                ("call-2".to_string(), None),
                ("call-1".to_string(), Some(AgentItemStatus::Cancelled)),
                ("call-2".to_string(), Some(AgentItemStatus::Cancelled)),
            ]
        );
        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert!(
            detail
                .tool_activities
                .iter()
                .all(|activity| activity.state == crate::storage::ToolActivityState::Cancelled)
        );
        assert_eq!(
            publisher
                .events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| matches!(
                    event.event,
                    AgentEvent::ItemCompleted {
                        item_type: AgentItemType::Tool,
                        status: AgentItemStatus::Cancelled,
                        ..
                    }
                ))
                .count(),
            2
        );
        assert_eq!(*publisher.started_calls.lock().unwrap(), ["call-1"]);
    }

    fn patch_call(patch: &str) -> ToolCall {
        patch_call_with_id("patch-call", patch)
    }

    fn patch_call_with_id(id: &str, patch: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "apply_patch".to_string(),
            arguments: json!({ "patch": patch }),
            metadata: json!({}),
        }
    }

    async fn editing_runtime(
        workspace: &Path,
        timeout: Duration,
    ) -> (
        Arc<JsonlThreadRepository>,
        AgentRuntime,
        Arc<ApprovalManager>,
        String,
    ) {
        let repository = Arc::new(JsonlThreadRepository::new(workspace.join("data")).unwrap());
        let thread = repository.create_thread().await.unwrap();
        let approvals = Arc::new(ApprovalManager::new(timeout));
        let service = crate::patch::PatchService::new();
        let runtime = AgentRuntime::with_tools_and_approvals(
            repository.clone(),
            ToolRegistry::workspace_tools(service),
            workspace.to_path_buf(),
            approvals.clone(),
        );
        (repository, runtime, approvals, thread.id)
    }

    #[tokio::test]
    async fn approved_patch_is_applied_audited_and_returned_to_the_model() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file.txt");
        std::fs::write(&file, "before\n").unwrap();
        let patch =
            "*** Begin Patch\n*** Update File: file.txt\n@@\n-before\n+after\n*** End Patch";
        let (repository, runtime, approvals, thread_id) =
            editing_runtime(directory.path(), Duration::from_secs(1)).await;
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: patch_call(patch),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "change applied".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(ResolvingPublisher {
            events: Mutex::new(Vec::new()),
            approvals: approvals.clone(),
            resolution: ApprovalResolution {
                action: ApprovalAction::Approved,
                patch: None,
                selected_paths: Vec::new(),
                expected_hashes: Vec::new(),
            },
            mutation: None,
        });

        let outcome = runtime
            .run_turn(
                provider.clone(),
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "edit file".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(std::fs::read_to_string(file).unwrap(), "after\n");
        assert!(matches!(
            provider.requests()[1].messages.last(),
            Some(ProviderMessage::ToolResult { success: true, .. })
        ));
        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert_eq!(detail.approvals.len(), 1);
        assert_eq!(detail.changes.len(), 1);
        assert_eq!(
            detail.changes[0].files[0].before_content.as_deref(),
            Some("before\n")
        );
        assert_eq!(approvals.pending_count().await, 0);
        let approval_id = detail.approvals[0].request.id.clone();
        let change_id = detail.changes[0].id.clone();
        let stored_lifecycle = repository
            .load(&thread_id)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.kind {
                StoredEventKind::ItemStarted { item_id, item_type }
                    if matches!(item_type, AgentItemType::Approval | AgentItemType::Change) =>
                {
                    Some((item_id, item_type, None))
                }
                StoredEventKind::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                } if matches!(item_type, AgentItemType::Approval | AgentItemType::Change) => {
                    Some((item_id, item_type, Some(status)))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stored_lifecycle,
            [
                (approval_id.clone(), AgentItemType::Approval, None),
                (
                    approval_id.clone(),
                    AgentItemType::Approval,
                    Some(AgentItemStatus::Completed),
                ),
                (change_id.clone(), AgentItemType::Change, None),
                (
                    change_id.clone(),
                    AgentItemType::Change,
                    Some(AgentItemStatus::Completed),
                ),
            ]
        );
        let published_sequence = publisher
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match &event.event {
                AgentEvent::ItemStarted {
                    item_id, item_type, ..
                } if matches!(item_type, AgentItemType::Approval | AgentItemType::Change) => {
                    Some(format!("item_started:{item_type:?}:{item_id}"))
                }
                AgentEvent::ApprovalRequested { request, .. } => {
                    Some(format!("approval_requested:{}", request.id))
                }
                AgentEvent::ApprovalResolved { request_id, .. } => {
                    Some(format!("approval_resolved:{request_id}"))
                }
                AgentEvent::ChangeApplied { change_set, .. } => {
                    Some(format!("change_applied:{}", change_set.id))
                }
                AgentEvent::ItemCompleted {
                    item_id,
                    item_type,
                    status,
                    ..
                } if matches!(item_type, AgentItemType::Approval | AgentItemType::Change) => {
                    Some(format!("item_completed:{item_type:?}:{item_id}:{status:?}"))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            published_sequence,
            [
                format!("item_started:Approval:{approval_id}"),
                format!("approval_requested:{approval_id}"),
                format!("approval_resolved:{approval_id}"),
                format!("item_completed:Approval:{approval_id}:Completed"),
                format!("item_started:Change:{change_id}"),
                format!("change_applied:{change_id}"),
                format!("item_completed:Change:{change_id}:Completed"),
            ]
        );
    }

    #[tokio::test]
    async fn full_access_auto_approves_patch_without_skipping_audit() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file.txt");
        std::fs::write(&file, "before\n").unwrap();
        let patch =
            "*** Begin Patch\n*** Update File: file.txt\n@@\n-before\n+after\n*** End Patch";
        let (repository, runtime, approvals, thread_id) =
            editing_runtime(directory.path(), Duration::from_secs(1)).await;
        let runtime = runtime.with_approval_mode(ApprovalMode::FullAccess);
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: patch_call(patch),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "change applied".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(RecordingPublisher::default());

        let outcome = runtime
            .run_turn(
                provider,
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "edit file".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(std::fs::read_to_string(file).unwrap(), "after\n");
        assert_eq!(approvals.pending_count().await, 0);
        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert_eq!(detail.approvals.len(), 1);
        assert!(detail.approvals[0].request.auto_approved);
        let serialized_request = serde_json::to_value(&detail.approvals[0].request).unwrap();
        assert_eq!(serialized_request["autoApproved"], true);
        assert!(serialized_request.get("auto_approved").is_none());
        assert_eq!(
            detail.approvals[0]
                .resolution
                .as_ref()
                .map(|resolution| resolution.action),
            Some(ApprovalAction::Approved)
        );
        assert!(
            detail.approvals[0]
                .request
                .reason
                .starts_with("full-access mode automatically approved:")
        );
        let events = publisher.events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(&event.event, AgentEvent::ApprovalRequested { .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(&event.event, AgentEvent::ApprovalResolved { .. }))
        );
        assert!(!events.iter().any(|event| matches!(
            &event.event,
            AgentEvent::ActivityStatusChanged {
                status: AgentActivityStatus::AwaitingApproval,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn repairs_a_failed_check_and_completes_the_same_turn() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file.txt");
        std::fs::write(&file, "before\n").unwrap();
        let repository =
            Arc::new(JsonlThreadRepository::new(directory.path().join("data")).unwrap());
        let thread = repository.create_thread().await.unwrap();
        let approvals = Arc::new(ApprovalManager::new(Duration::from_secs(1)));
        let tools = ToolRegistry::workspace_tools(crate::patch::PatchService::new())
            .with_additional_handlers(
                vec![Arc::new(AssertFileTool)],
                std::collections::HashMap::from([("assert_file".to_string(), ToolRisk::Read)]),
            )
            .unwrap();
        let runtime = AgentRuntime::with_tools_and_approvals(
            repository.clone(),
            tools,
            directory.path().to_path_buf(),
            approvals.clone(),
        );
        let first_patch =
            "*** Begin Patch\n*** Update File: file.txt\n@@\n-before\n+broken\n*** End Patch";
        let repair_patch =
            "*** Begin Patch\n*** Update File: file.txt\n@@\n-broken\n+fixed\n*** End Patch";
        let assert_call = |id: &str| ToolCall {
            id: id.to_string(),
            name: "assert_file".to_string(),
            arguments: json!({ "path": "file.txt", "expected": "fixed\n" }),
            metadata: json!({}),
        };
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "I will make the change.".to_string(),
                }),
                Ok(ProviderEvent::ToolCall {
                    call: patch_call_with_id("patch-broken", first_patch),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: assert_call("check-failed"),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "The check failed, so I will repair it.".to_string(),
                }),
                Ok(ProviderEvent::ToolCall {
                    call: patch_call_with_id("patch-fixed", repair_patch),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: assert_call("check-passed"),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "The repair is complete and verified.".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(ResolvingPublisher {
            events: Mutex::new(Vec::new()),
            approvals,
            resolution: ApprovalResolution {
                action: ApprovalAction::Approved,
                patch: None,
                selected_paths: Vec::new(),
                expected_hashes: Vec::new(),
            },
            mutation: None,
        });

        let outcome = runtime
            .run_turn(
                provider.clone(),
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread.id.clone(),
                    input: "change the file and verify it".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher,
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(std::fs::read_to_string(file).unwrap(), "fixed\n");
        assert_eq!(provider.requests().len(), 5);
        assert!(matches!(
            provider.requests()[2].messages.last(),
            Some(ProviderMessage::ToolResult { success: false, output, .. })
                if output.contains("file check failed")
        ));
        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.changes.len(), 2);
        assert_eq!(detail.tool_activities.len(), 4);
        assert!(detail.tool_activities.iter().any(|activity| {
            activity.call.id == "check-failed"
                && activity.state == crate::storage::ToolActivityState::Failed
        }));
        assert!(detail.tool_activities.iter().any(|activity| {
            activity.call.id == "check-passed"
                && activity.state == crate::storage::ToolActivityState::Completed
        }));
        let stored_tool_completions = repository
            .load(&thread.id)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.kind {
                StoredEventKind::ItemCompleted {
                    item_id,
                    item_type: AgentItemType::Tool,
                    status,
                } => Some((item_id, status)),
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(
            stored_tool_completions.get("check-failed"),
            Some(&AgentItemStatus::Failed)
        );
        assert_eq!(
            stored_tool_completions.get("check-passed"),
            Some(&AgentItemStatus::Completed)
        );
        assert_eq!(
            detail.messages.last().unwrap().text(),
            "The repair is complete and verified."
        );
    }

    #[tokio::test]
    async fn generic_external_approval_executes_once_without_patch_capabilities() {
        let directory = tempfile::tempdir().unwrap();
        let repository =
            Arc::new(JsonlThreadRepository::new(directory.path().join("data")).unwrap());
        let thread = repository.create_thread().await.unwrap();
        let approvals = Arc::new(ApprovalManager::new(Duration::from_secs(1)));
        let name = "mcp__fixture__write".to_string();
        let tools = ToolRegistry::read_only()
            .with_extensions(
                vec![Arc::new(ExternalTool)],
                HashMap::from([(name.clone(), ToolRisk::Write)]),
                None,
            )
            .unwrap();
        let runtime = AgentRuntime::with_tools_and_approvals(
            repository.clone(),
            tools,
            directory.path().into(),
            approvals.clone(),
        );
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: "external-call".into(),
                        name,
                        arguments: json!({}),
                        metadata: json!({}),
                    },
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "done".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(ResolvingPublisher {
            events: Mutex::new(Vec::new()),
            approvals: approvals.clone(),
            resolution: ApprovalResolution {
                action: ApprovalAction::Approved,
                patch: None,
                selected_paths: Vec::new(),
                expected_hashes: Vec::new(),
            },
            mutation: None,
        });
        let outcome = runtime
            .run_turn(
                provider,
                "fake".into(),
                RunTurnRequest {
                    thread_id: thread.id.clone(),
                    input: "use external tool".into(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher,
            )
            .await
            .unwrap();
        assert_eq!(outcome.state, TurnState::Completed);
        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.approvals.len(), 1);
        assert!(detail.approvals[0].request.preview.is_none());
        assert!(detail.tool_activities.iter().any(|activity| {
            activity
                .result
                .as_ref()
                .is_some_and(|result| result.output == "external completed")
        }));
    }

    #[tokio::test]
    async fn reviewed_edited_patch_replaces_the_model_proposal() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file.txt");
        std::fs::write(&file, "before\n").unwrap();
        let model_patch = "*** Begin Patch\n*** Update File: file.txt\n@@\n-before\n+model version\n*** End Patch";
        let edited_patch = "*** Begin Patch\n*** Update File: file.txt\n@@\n-before\n+reviewed version\n*** End Patch";
        let repository =
            Arc::new(JsonlThreadRepository::new(directory.path().join("data")).unwrap());
        let thread = repository.create_thread().await.unwrap();
        let approvals = Arc::new(ApprovalManager::new(Duration::from_secs(1)));
        let service = crate::patch::PatchService::new();
        let edited_preview = service
            .preview_patch(directory.path(), edited_patch)
            .unwrap();
        let runtime = AgentRuntime::with_tools_and_approvals(
            repository.clone(),
            ToolRegistry::workspace_tools(service),
            directory.path().to_path_buf(),
            approvals.clone(),
        );
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: patch_call(model_patch),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "reviewed change applied".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let publisher = Arc::new(ResolvingPublisher {
            events: Mutex::new(Vec::new()),
            approvals,
            resolution: ApprovalResolution {
                action: ApprovalAction::Approved,
                patch: Some(edited_patch.to_string()),
                selected_paths: vec!["file.txt".to_string()],
                expected_hashes: preview_hashes(&edited_preview),
            },
            mutation: None,
        });

        let outcome = runtime
            .run_turn(
                provider,
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread.id.clone(),
                    input: "edit the proposal".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher,
            )
            .await
            .unwrap();

        assert_eq!(outcome.state, TurnState::Completed);
        assert_eq!(std::fs::read_to_string(file).unwrap(), "reviewed version\n");
        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert_eq!(
            detail.changes[0].files[0].after_content.as_deref(),
            Some("reviewed version\n")
        );
    }

    #[tokio::test]
    async fn failed_change_audit_rolls_back_the_applied_patch() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file.txt");
        std::fs::write(&file, "before\n").unwrap();
        let inner = Arc::new(JsonlThreadRepository::new(directory.path().join("data")).unwrap());
        let thread = inner.create_thread().await.unwrap();
        let repository: Arc<dyn ThreadRepository> = Arc::new(RejectChangeAuditRepository {
            inner: inner.clone(),
        });
        let approvals = Arc::new(ApprovalManager::new(Duration::from_secs(1)));
        let service = crate::patch::PatchService::new();
        let runtime = AgentRuntime::with_tools_and_approvals(
            repository,
            ToolRegistry::workspace_tools(service),
            directory.path().to_path_buf(),
            approvals.clone(),
        );
        let patch =
            "*** Begin Patch\n*** Update File: file.txt\n@@\n-before\n+after\n*** End Patch";
        let provider = Arc::new(FakeProvider::script(vec![vec![
            Ok(ProviderEvent::ToolCall {
                call: patch_call(patch),
            }),
            Ok(ProviderEvent::Completed),
        ]]));
        let publisher = Arc::new(ResolvingPublisher {
            events: Mutex::new(Vec::new()),
            approvals,
            resolution: ApprovalResolution {
                action: ApprovalAction::Approved,
                patch: None,
                selected_paths: Vec::new(),
                expected_hashes: Vec::new(),
            },
            mutation: None,
        });

        let result = runtime
            .run_turn(
                provider,
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread.id.clone(),
                    input: "edit file".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher.clone(),
            )
            .await;

        assert!(matches!(result, Err(AgentRuntimeError::Storage(_))));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "before\n");
        let detail = inner.read_thread(&thread.id).await.unwrap();
        assert!(detail.changes.is_empty());
        assert!(matches!(
            detail.last_turn,
            Some(crate::storage::TurnSnapshot {
                state: TurnState::Failed,
                ..
            })
        ));
        assert!(inner.load(&thread.id).await.unwrap().iter().any(|event| {
            matches!(
                &event.kind,
                StoredEventKind::ItemCompleted {
                    item_type: AgentItemType::Change,
                    status: AgentItemStatus::Failed,
                    ..
                }
            )
        }));
        assert!(publisher.events.lock().unwrap().iter().any(|event| {
            matches!(
                &event.event,
                AgentEvent::ItemCompleted {
                    item_type: AgentItemType::Change,
                    status: AgentItemStatus::Failed,
                    ..
                }
            )
        }));
        assert!(publisher.events.lock().unwrap().iter().any(|event| {
            matches!(&event.event, AgentEvent::TurnFailed { message, .. } if message.contains("injected change audit failure"))
        }));
    }

    #[tokio::test]
    async fn rejected_and_timed_out_patches_do_not_change_files() {
        for (action, timeout) in [
            (Some(ApprovalAction::Rejected), Duration::from_secs(1)),
            (None, Duration::from_millis(5)),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let file = directory.path().join("file.txt");
            std::fs::write(&file, "before\n").unwrap();
            let patch =
                "*** Begin Patch\n*** Update File: file.txt\n@@\n-before\n+after\n*** End Patch";
            let (repository, runtime, approvals, thread_id) =
                editing_runtime(directory.path(), timeout).await;
            let provider = Arc::new(FakeProvider::script(vec![
                vec![
                    Ok(ProviderEvent::ToolCall {
                        call: patch_call(patch),
                    }),
                    Ok(ProviderEvent::Completed),
                ],
                vec![
                    Ok(ProviderEvent::TextDelta {
                        delta: "not changed".to_string(),
                    }),
                    Ok(ProviderEvent::Completed),
                ],
            ]));
            let publisher: Arc<dyn EventPublisher> = match action {
                Some(action) => Arc::new(ResolvingPublisher {
                    events: Mutex::new(Vec::new()),
                    approvals,
                    resolution: ApprovalResolution {
                        action,
                        patch: None,
                        selected_paths: Vec::new(),
                        expected_hashes: Vec::new(),
                    },
                    mutation: None,
                }),
                None => Arc::new(RecordingPublisher::default()),
            };
            runtime
                .run_turn(
                    provider,
                    "fake".to_string(),
                    RunTurnRequest {
                        thread_id: thread_id.clone(),
                        input: "edit file".to_string(),
                        agent_mode: None,
                    },
                    CancellationToken::new(),
                    publisher,
                )
                .await
                .unwrap();
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "before\n");
            assert!(
                repository
                    .read_thread(&thread_id)
                    .await
                    .unwrap()
                    .changes
                    .is_empty()
            );
            let events = repository.load(&thread_id).await.unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        &event.kind,
                        StoredEventKind::ItemStarted {
                            item_type: AgentItemType::Approval,
                            ..
                        }
                    ))
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        &event.kind,
                        StoredEventKind::ItemCompleted {
                            item_type: AgentItemType::Approval,
                            status: AgentItemStatus::Failed,
                            ..
                        }
                    ))
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn approved_patch_reports_conflict_when_file_changed_during_review() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file.txt");
        std::fs::write(&file, "before\n").unwrap();
        let patch =
            "*** Begin Patch\n*** Update File: file.txt\n@@\n-before\n+after\n*** End Patch";
        let (repository, runtime, approvals, thread_id) =
            editing_runtime(directory.path(), Duration::from_secs(1)).await;
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::ToolCall {
                    call: patch_call(patch),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "conflict".to_string(),
                }),
                Ok(ProviderEvent::Completed),
            ],
        ]));
        let file_for_mutation = file.clone();
        let publisher = Arc::new(ResolvingPublisher {
            events: Mutex::new(Vec::new()),
            approvals,
            resolution: ApprovalResolution {
                action: ApprovalAction::Approved,
                patch: None,
                selected_paths: Vec::new(),
                expected_hashes: Vec::new(),
            },
            mutation: Some(Box::new(move || {
                std::fs::write(&file_for_mutation, "newer\n").unwrap();
            })),
        });
        runtime
            .run_turn(
                provider.clone(),
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread_id.clone(),
                    input: "edit file".to_string(),
                    agent_mode: None,
                },
                CancellationToken::new(),
                publisher,
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), "newer\n");
        assert!(matches!(
            provider.requests()[1].messages.last(),
            Some(ProviderMessage::ToolResult { success: false, output, .. })
                if output.contains("conflict")
        ));
        assert!(
            repository
                .read_thread(&thread_id)
                .await
                .unwrap()
                .changes
                .is_empty()
        );
    }
}
