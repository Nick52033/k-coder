use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::advanced::{REQUEST_USER_INPUT_TOOL_NAME, RequestUserInputTool, RuntimeMetrics};
use crate::context::{self, CompactionSummary, DEFAULT_CONTEXT_LIMIT};
use crate::logging::StructuredLogger;
use crate::policy::{
    ApprovalError, ApprovalManager, PolicyDecision, UserInputError, UserInputManager,
};
use crate::protocol::{
    AgentActivityStatus, AgentEvent, AgentEventEnvelope, AgentItemStatus, AgentItemType, AgentMode,
    ApprovalAction, ApprovalMode, ApprovalRequest, ApprovalResolution, ChangeSet, ChatMessage,
    ContentBlock, ExpectedFileHash, ImageAttachment, MessageRole, PROTOCOL_VERSION, PatchPreview,
    ReasoningEffort, TokenUsage, ToolCall, ToolResult, TurnError, TurnState, UserInputAction,
    UserInputQuestion, UserInputRequest, UserInputRequestKind, UserInputResolution,
};
use crate::providers::{Provider, ProviderError, ProviderEvent, ProviderMessage, ProviderRequest};
use crate::storage::{StorageError, StoredEvent, StoredEventKind, ThreadRepository, now_ms};
use crate::tools::{
    ApprovedToolExecution, ToolContext, ToolError, ToolProgress, ToolRegistry,
    tool_progress_channel,
};

mod input;
pub mod mailbox;
mod provider_history;
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
const PROGRESS_CHECK_WINDOW: usize = 5;
const MAX_NO_PROGRESS_WINDOWS: usize = 3;
pub const DEFAULT_SOFT_TURN_PROVIDER_CALLS: u32 = 30;
pub const DEFAULT_SOFT_TURN_TOTAL_TOKENS: u64 = 1_000_000;
pub const DEFAULT_SOFT_TURN_DURATION_MS: u64 = 10 * 60 * 1_000;

const TURN_CONTINUATION_TOOL_CALL_ID: &str = "runtime-turn-continuation";
const TURN_CONTINUE: &str = "continue";
const TURN_COMPACT_AND_CONTINUE: &str = "compact_and_continue";
const TURN_STOP: &str = "stop";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoftTurnLimits {
    provider_calls: u32,
    total_tokens: u64,
    duration_ms: u64,
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
            duration_ms,
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
                || self.duration_ms >= limits.duration_ms)
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

        for event in events {
            let mut hasher = DefaultHasher::new();
            match &event.kind {
                StoredEventKind::ChangeApplied { change_set } => {
                    "change".hash(&mut hasher);
                    format!("{:?}", change_set.files).hash(&mut hasher);
                    progress_fingerprints.insert(hasher.finish());
                }
                StoredEventKind::ToolResult { name, result, .. } if result.success => {
                    "tool".hash(&mut hasher);
                    name.hash(&mut hasher);
                    result.output.hash(&mut hasher);
                    progress_fingerprints.insert(hasher.finish());
                }
                _ => {}
            }
        }

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
    #[error("change audit failed: {storage_error}; rollback also failed: {rollback_error}")]
    AuditCompensation {
        storage_error: String,
        rollback_error: String,
    },
}

pub trait EventPublisher: Send + Sync {
    fn publish(&self, event: AgentEventEnvelope);
}

pub struct AgentRuntime {
    repository: Arc<dyn ThreadRepository>,
    tools: ToolRegistry,
    workspace_root: PathBuf,
    approvals: Arc<ApprovalManager>,
    approval_mode: ApprovalMode,
    user_inputs: Arc<UserInputManager>,
    runtime_instructions: String,
    max_total_tokens: Option<u64>,
    soft_turn_limits: Option<SoftTurnLimits>,
    context_limit: usize,
    working_context_limit: usize,
    metrics: Option<RuntimeMetrics>,
    reasoning_effort: ReasoningEffort,
    supports_vision: bool,
    logger: Option<StructuredLogger>,
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
            user_inputs: Arc::new(UserInputManager::new(std::time::Duration::from_secs(
                10 * 60,
            ))),
            runtime_instructions: String::new(),
            max_total_tokens: None,
            soft_turn_limits: None,
            context_limit: DEFAULT_CONTEXT_LIMIT,
            working_context_limit: context::default_working_context_limit(DEFAULT_CONTEXT_LIMIT),
            metrics: None,
            reasoning_effort: ReasoningEffort::default(),
            supports_vision: false,
            logger: None,
        }
    }

    pub fn with_runtime_instructions(mut self, instructions: String) -> Self {
        self.runtime_instructions = instructions;
        self
    }

    pub fn with_logger(mut self, logger: StructuredLogger) -> Self {
        self.logger = Some(logger);
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
        let (summary, _) =
            context::compact(&history, self.working_context_limit.min(self.context_limit));
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
        turn_id: String,
        cancellation: CancellationToken,
        control: Option<Arc<TurnControl>>,
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
            user_message: started_user_message,
        }));

        let result = async {
        if cancellation.is_cancelled() {
            return self
                .finish_cancelled(&thread_id, &turn_id, &publisher)
                .await;
        }

        let mut total_usage = TokenUsage::default();
        let mut has_usage = false;
        let mut provider_call_index = 0u32;
        let mut provider_context_bytes = 0usize;
        let mut last_call_signature = None::<String>;
        let mut identical_call_streak = 0usize;
        let token_budget = self.max_total_tokens;
        let mut soft_turn_segment = self
            .soft_turn_limits
            .map(|_| SoftTurnSegment::new(provider_call_index, total_usage.total_tokens));
        let mut force_compaction = false;
        let tool_definitions = self.tools.provider_definitions();

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

            let events = self.repository.load(&thread_id).await?;
            let last_context_usage = last_active_context_usage(&events);
            let mut history = provider_history(events, self.supports_vision);
            if force_compaction
                || context::needs_compaction_for_request(
                    &history,
                    &self.runtime_instructions,
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
                    &history,
                    self.working_context_limit.min(self.context_limit),
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
                    history = compacted;
                }
            }
            if !self.runtime_instructions.trim().is_empty() {
                history.insert(
                    0,
                    ProviderMessage::Text {
                        role: MessageRole::System,
                        text: self.runtime_instructions.clone(),
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
            // 重试逻辑：对于不完整的工具调用错误，最多重试5次
            const MAX_RETRIES: u32 = 5;
            let mut retry_count = 0;
            let mut _last_error: Option<ProviderError> = None;

            // 声明需要在重试循环外部的变量
            let response: String;
            let response_images: Vec<ContentBlock>;
            let pending_tool_calls: Vec<ToolCall>;
            let completed: bool;
            let assistant_item_id = Uuid::new_v4().to_string();
            self.start_item(
                &thread_id,
                &turn_id,
                &assistant_item_id,
                AgentItemType::AgentMessage,
                &publisher,
            )
            .await?;

            // 外层循环：支持整个请求的重试
            'retry_loop: loop {
                let call_index = provider_call_index;
                provider_call_index = provider_call_index.saturating_add(1);
                let provider_started = std::time::Instant::now();
                let mut stream = loop {
                    match provider.stream(request.clone(), cancellation.clone()).await {
                        Ok(stream) => break stream,
                        Err(ProviderError::Cancelled) => {
                            self.record_provider_metric(provider_started, false, None);
                            return self
                                .finish_cancelled(&thread_id, &turn_id, &publisher)
                                .await;
                        }
                        Err(error) => {
                            // 检查是否是不完整工具调用错误且可以重试
                            let is_incomplete_tool_call =
                                error.to_string().contains("incomplete tool call");

                            if is_incomplete_tool_call && retry_count < MAX_RETRIES {
                                retry_count += 1;
                                _last_error = Some(error);

                                // 等待一小段时间后重试（指数退避）
                                // 200ms, 500ms, 1000ms, 2000ms, 4000ms
                                let wait_ms = 200 * (1 << (retry_count - 1));
                                let wait_ms = wait_ms.min(4000); // 最多等待4秒
                                tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;

                                continue;
                            }

                            self.record_provider_metric(provider_started, false, None);

                            // 如果已经重试过，在错误信息中包含重试次数
                            let error_msg = if retry_count > 0 {
                                format!("{} (已重试 {} 次)", error, retry_count)
                            } else {
                                error.to_string()
                            };

                            return self
                                .finish_failed(&thread_id, &turn_id, error_msg, &publisher)
                                .await;
                        }
                    }
                };

                let mut response_inner = String::new();
                let mut reasoning_summary_bytes = HashMap::<String, usize>::new();
                let mut reasoning_items_completed = HashSet::<String>::new();
                let mut responding_published = false;
                let mut pending_tool_calls_inner = Vec::new(); // 暂存 ToolCall，等 Completed 后再启动
                let mut response_images_inner = Vec::new();
                let mut iteration_usage_inner = None;
                let mut attempt_had_output = false;
                let completed_inner = loop {
                    let event = tokio::select! {
                        _ = cancellation.cancelled() => {
                            return self.finish_cancelled(&thread_id, &turn_id, &publisher).await;
                        }
                        event = stream.next() => event,
                    };

                    match event {
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
                            publisher.publish(AgentEventEnvelope::new(AgentEvent::TextDelta {
                                thread_id: thread_id.clone(),
                                turn_id: turn_id.clone(),
                                item_id: assistant_item_id.clone(),
                                delta,
                            }));
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
                        Some(Ok(ProviderEvent::Usage { usage })) => {
                            iteration_usage_inner = Some(usage);
                            let aggregate = add_usage(total_usage, usage);
                            if let Some(budget) =
                                token_budget.filter(|budget| aggregate.total_tokens > *budget)
                            {
                                self.persist_provider_usage(
                                    &thread_id,
                                    &turn_id,
                                    call_index,
                                    usage,
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
                            break true;
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
                                    &mut total_usage,
                                    &mut has_usage,
                                    &publisher,
                                )
                                .await?;
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

                            // 检查是否是可重试的错误（不完整工具调用或 JSON 解析失败）
                            let error_str = error.to_string();
                            let is_retriable = error_str.contains("incomplete tool call")
                                || error_str.contains("invalid JSON arguments")
                                || error_str.contains("returned invalid JSON");

                            if let Some(usage) = iteration_usage_inner {
                                self.persist_provider_usage(
                                    &thread_id,
                                    &turn_id,
                                    call_index,
                                    usage,
                                    &mut total_usage,
                                    &mut has_usage,
                                    &publisher,
                                )
                                .await?;
                            }

                            if is_retriable && !attempt_had_output && retry_count < MAX_RETRIES {
                                retry_count += 1;

                                // 等待一小段时间后重试（指数退避）
                                // 200ms, 500ms, 1000ms, 2000ms, 4000ms
                                let wait_ms = 200 * (1 << (retry_count - 1));
                                let wait_ms = wait_ms.min(4000); // 最多等待4秒
                                tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;

                                // 跳出内层循环，重新开始整个请求
                                continue 'retry_loop;
                            }

                            return self
                                .finish_failed(&thread_id, &turn_id, error.to_string(), &publisher)
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
                                    &mut total_usage,
                                    &mut has_usage,
                                    &publisher,
                                )
                                .await?;
                                iteration_usage_inner = None;
                            }
                            break false;
                        }
                    }
                };

                if let Some(usage) = iteration_usage_inner {
                    self.persist_provider_usage(
                        &thread_id,
                        &turn_id,
                        call_index,
                        usage,
                        &mut total_usage,
                        &mut has_usage,
                        &publisher,
                    )
                    .await?;
                }
                response = response_inner;
                response_images = response_images_inner;
                pending_tool_calls = pending_tool_calls_inner;
                completed = completed_inner;
                break;
            } // 'retry_loop 结束

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
                        text: response.clone(),
                        calls: pending_tool_calls.clone(),
                    },
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
            for call in &pending_tool_calls {
                self.start_item(
                    &thread_id,
                    &turn_id,
                    &call.id,
                    AgentItemType::Tool,
                    &publisher,
                )
                .await?;
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

                if identical_call_streak > MAX_IDENTICAL_TOOL_CALLS {
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
                };

                let (result, item_status) = match self
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
        let request = UserInputRequest {
            id: request_id,
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            tool_call_id: TURN_CONTINUATION_TOOL_CALL_ID.to_string(),
            kind: UserInputRequestKind::TurnContinuation,
            questions: vec![UserInputQuestion {
                question: format!(
                    "当前执行段已调用模型 {} 次、累计消耗 {} tokens、运行 {} 秒。继续后会获得新一段额度（{} 次调用 / {} tokens / {} 秒）。",
                    usage.provider_calls,
                    usage.total_tokens,
                    usage.duration_ms.div_ceil(1_000),
                    limits.provider_calls,
                    limits.total_tokens,
                    limits.duration_ms.div_ceil(1_000),
                ),
                options: vec![
                    TURN_CONTINUE.to_string(),
                    TURN_COMPACT_AND_CONTINUE.to_string(),
                    TURN_STOP.to_string(),
                ],
            }],
            created_at_ms,
            expires_at_ms: created_at_ms.saturating_add(self.user_inputs.timeout_ms()),
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
            expires_at_ms: created_at_ms.saturating_add(self.user_inputs.timeout_ms()),
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

    async fn finish_failed(
        &self,
        thread_id: &str,
        turn_id: &str,
        message: String,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
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
                    error: Some(TurnError::classify(message.clone())),
                },
            )
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnFailed {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            message: message.clone(),
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
                StoredEventKind::ProviderCallUsage { call_index, usage },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::UsageUpdated {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            usage: *total_usage,
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
                ocr_text: None,
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

        let ocr_message = user_message(
            "inspect this screenshot".into(),
            vec![ImageAttachment {
                name: "screen.png".into(),
                data_url: "data:image/png;base64,iVBORw0KGgo=".into(),
                ocr_text: Some("compiler error E0308".into()),
            }],
            false,
        )
        .unwrap();
        assert!(matches!(
            &ocr_message.content[1],
            ContentBlock::Context { text } if text.contains("compiler error E0308")
        ));
        assert!(matches!(
            &ocr_message.content[2],
            ContentBlock::Image { name, .. } if name == "screen.png"
        ));
        assert!(matches!(
            chat_to_provider(ocr_message, false),
            Some(ProviderMessage::Text { role: MessageRole::User, text })
                if text.contains("inspect this screenshot")
                    && text.contains("compiler error E0308")
        ));
        assert!(
            user_message(
                "missing OCR".into(),
                vec![ImageAttachment {
                    name: "screen.png".into(),
                    data_url: "data:image/png;base64,iVBORw0KGgo=".into(),
                    ocr_text: None,
                }],
                false,
            )
            .is_err()
        );
        assert!(
            user_message(
                "bad image".into(),
                vec![ImageAttachment {
                    name: "bad.svg".into(),
                    data_url: "data:image/svg+xml;base64,PHN2Zy8+".into(),
                    ocr_text: None,
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
                ocr_text: None,
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

    #[tokio::test]
    async fn uses_the_caller_assigned_turn_id_for_events_and_persistence() {
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
                        delta: "first answer".into(),
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
            .with_delay(Duration::from_millis(20)),
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
            while provider.requests().is_empty() {
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
        assert!(publisher.events.lock().unwrap().iter().any(|event| {
            matches!(
                &event.event,
                AgentEvent::TurnSteered { turn_id, message, .. }
                    if turn_id == "turn-steered" && message.visible_text() == "adjust it"
            )
        }));
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
            Some(ProviderMessage::ToolResult { output, .. }) if output == "workspace docs"
        ));
        let events = repository.load(&thread_id).await.unwrap();
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
                AgentEvent::ToolStarted { call, .. } => Some(format!("started:{}", call.id)),
                AgentEvent::ToolCompleted { call_id, .. } => Some(format!("completed:{call_id}")),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lifecycle,
            [
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
                Err(ProviderError::InvalidResponse(
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
                StoredEventKind::ProviderCallUsage { call_index, usage } => {
                    Some((call_index, usage.total_tokens))
                }
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
            Err(ProviderError::InvalidResponse(
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
        );

        assert!(matches!(
            history.as_slice(),
            [ProviderMessage::Text { text, .. }]
                if text.contains("tool read_file (true): important result")
        ));
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
        );

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
