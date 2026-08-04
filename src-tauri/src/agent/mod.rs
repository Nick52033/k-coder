use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

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
    AgentActivityStatus, AgentEvent, AgentEventEnvelope, ApprovalAction, ApprovalMode,
    ApprovalRequest, ApprovalResolution, ChangeSet, ChatMessage, ContentBlock, ExpectedFileHash,
    ImageAttachment, MessageRole, PROTOCOL_VERSION, PatchPreview, ReasoningEffort, TokenUsage,
    ToolCall, ToolResult, TurnState, UserInputAction, UserInputRequest, UserInputResolution,
};
use crate::providers::{
    Provider, ProviderError, ProviderEvent, ProviderImage, ProviderMessage, ProviderRequest,
};
use crate::storage::{StorageError, StoredEvent, StoredEventKind, ThreadRepository, now_ms};
use crate::tools::{
    ApprovedToolExecution, ToolContext, ToolError, ToolProgress, ToolRegistry,
    tool_progress_channel,
};

const MAX_INPUT_BYTES: usize = 100_000;
const MAX_IMAGE_COUNT: usize = 4;
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_OCR_TEXT_BYTES: usize = 16 * 1024;
const MAX_TOTAL_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_REASONING_SUMMARY_BYTES: usize = 64 * 1024;
const MAX_PROVIDER_CONTEXT_BYTES: usize = 512 * 1024;
const MAX_TOOL_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_IDENTICAL_TOOL_CALLS: usize = 2;
const PROGRESS_CHECK_WINDOW: usize = 5;
const MAX_NO_PROGRESS_WINDOWS: usize = 3;

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
    context_limit: usize,
    metrics: Option<RuntimeMetrics>,
    reasoning_effort: ReasoningEffort,
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
            context_limit: DEFAULT_CONTEXT_LIMIT,
            metrics: None,
            reasoning_effort: ReasoningEffort::default(),
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

    pub fn with_context_limit(mut self, context_limit: usize) -> Self {
        self.context_limit = context_limit.max(1_024);
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
        let input = validate_input(&request.input, !attachments.is_empty())?;
        let message = user_message(input, attachments)?;
        self.run_turn_inner(
            provider,
            model,
            request.thread_id,
            Some(message),
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

    pub async fn compact_thread(
        &self,
        thread_id: &str,
    ) -> Result<CompactionSummary, AgentRuntimeError> {
        let history = provider_history(self.repository.load(thread_id).await?);
        let (summary, _) = context::compact(&history, self.context_limit);
        if summary.compacted_message_count > 0 {
            self.repository
                .append(StoredEvent::new(
                    thread_id,
                    None,
                    StoredEventKind::ContextCompacted {
                        summary: summary.clone(),
                        automatic: false,
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

        if let Some(message) = new_input {
            self.repository
                .append(StoredEvent::new(
                    &thread_id,
                    None,
                    StoredEventKind::UserMessage { message },
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

        let mut total_usage = TokenUsage::default();
        let mut has_usage = false;
        let mut provider_context_bytes = 0usize;
        let mut last_call_signature = None::<String>;
        let mut identical_call_streak = 0usize;
        let token_budget = self.max_total_tokens;

        // 进展检测变量
        let mut no_progress_count = 0usize;
        let mut last_snapshot: Option<ProgressSnapshot> = None;

        let mut iteration = 0usize;
        loop {
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
            let mut history = provider_history(events);
            if context::needs_compaction(&history, self.context_limit)
                || last_context_usage.is_some_and(|usage| {
                    context::needs_compaction_for_usage(usage.total_tokens, self.context_limit)
                })
            {
                let (summary, compacted) = context::compact(&history, self.context_limit);
                if summary.compacted_message_count > 0 {
                    self.repository
                        .append(StoredEvent::new(
                            &thread_id,
                            Some(turn_id.clone()),
                            StoredEventKind::ContextCompacted {
                                summary,
                                automatic: true,
                            },
                        ))
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
                tools: self.tools.definitions(),
            };
            publisher.publish(AgentEventEnvelope::new(AgentEvent::ActivityStatusChanged {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                status: AgentActivityStatus::Thinking,
            }));
            let provider_started = std::time::Instant::now();

            // 重试逻辑：对于不完整的工具调用错误，最多重试5次
            const MAX_RETRIES: u32 = 5;
            let mut retry_count = 0;
            let mut _last_error: Option<ProviderError> = None;

            // 声明需要在重试循环外部的变量
            let response: String;
            let pending_tool_calls: Vec<ToolCall>;
            let iteration_usage: Option<crate::protocol::TokenUsage>;
            let completed: bool;

            // 外层循环：支持整个请求的重试
            'retry_loop: loop {
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

                                // 发布重试通知
                                publisher.publish(AgentEventEnvelope::new(AgentEvent::TextDelta {
                                    thread_id: thread_id.clone(),
                                    turn_id: turn_id.clone(),
                                    delta: format!("\n[重试 {}/{}...]\n", retry_count, MAX_RETRIES),
                                }));

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
                let mut responding_published = false;
                let mut pending_tool_calls_inner = Vec::new(); // 暂存 ToolCall，等 Completed 后再启动
                let mut iteration_usage_inner = None;
                let completed_inner = loop {
                    let event = tokio::select! {
                        _ = cancellation.cancelled() => {
                            return self.finish_cancelled(&thread_id, &turn_id, &publisher).await;
                        }
                        event = stream.next() => event,
                    };

                    match event {
                        Some(Ok(ProviderEvent::TextDelta { delta })) => {
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
                                delta,
                            }));
                        }
                        Some(Ok(ProviderEvent::ReasoningSummaryDelta { item_id, delta })) => {
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
                                    item_id,
                                    summary,
                                },
                            ));
                        }
                        Some(Ok(ProviderEvent::ToolCall { call })) => {
                            // 先暂存，等 AI 完成后再启动执行
                            pending_tool_calls_inner.push(call);
                        }
                        Some(Ok(ProviderEvent::ProviderContext { provider, item })) => {
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
                                self.repository
                                    .append(StoredEvent::new(
                                        &thread_id,
                                        Some(turn_id.clone()),
                                        StoredEventKind::ProviderCallUsage {
                                            call_index: iteration as u32,
                                            usage,
                                        },
                                    ))
                                    .await?;
                                return self
                                    .finish_failed(
                                        &thread_id,
                                        &turn_id,
                                        format!(
                                            "token_budget_exceeded: used {} of {} tokens",
                                            aggregate.total_tokens, budget
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

                            if is_retriable && retry_count < MAX_RETRIES {
                                retry_count += 1;

                                // 等待一小段时间后重试（指数退避）
                                // 200ms, 500ms, 1000ms, 2000ms, 4000ms
                                let wait_ms = 200 * (1 << (retry_count - 1));
                                let wait_ms = wait_ms.min(4000); // 最多等待4秒
                                tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;

                                // 发布重试通知
                                publisher.publish(AgentEventEnvelope::new(AgentEvent::TextDelta {
                                    thread_id: thread_id.clone(),
                                    turn_id: turn_id.clone(),
                                    delta: format!(
                                        "\n[流式传输中断，重试 {}/{}...]\n",
                                        retry_count, MAX_RETRIES
                                    ),
                                }));

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
                            break false;
                        }
                    }
                };

                // 成功完成，跳出重试循环
                response = response_inner;
                pending_tool_calls = pending_tool_calls_inner;
                iteration_usage = iteration_usage_inner;
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
            if let Some(usage) = iteration_usage {
                total_usage = add_usage(total_usage, usage);
                has_usage = true;
                self.repository
                    .append(StoredEvent::new(
                        &thread_id,
                        Some(turn_id.clone()),
                        StoredEventKind::ProviderCallUsage {
                            call_index: iteration as u32,
                            usage,
                        },
                    ))
                    .await?;
                publisher.publish(AgentEventEnvelope::new(AgentEvent::UsageUpdated {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    usage: total_usage,
                }));
            }

            // AI 完成输出后的处理
            if pending_tool_calls.is_empty() {
                if response.is_empty() {
                    return self
                        .finish_failed(
                            &thread_id,
                            &turn_id,
                            "provider completed without text or a tool call".to_string(),
                            &publisher,
                        )
                        .await;
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
                        response,
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
                        text: response.clone(),
                        calls: pending_tool_calls.clone(),
                    },
                ))
                .await?;

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
                    self.persist_tool_result(&thread_id, &turn_id, &call, &result, &publisher)
                        .await?;
                    continue;
                }

                if let Some(reason) = &stop_reason {
                    let result = failure_result(format!("tool execution skipped: {reason}"));
                    if let Some(metrics) = &self.metrics {
                        metrics.tool(result.success);
                    }
                    self.persist_tool_result(&thread_id, &turn_id, &call, &result, &publisher)
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

                let result = match self
                    .execute_tool_with_progress(context, &call, cancellation.clone(), &publisher)
                    .await
                {
                    Ok(Some(tool_result)) => bound_tool_result(tool_result),
                    Ok(None) => {
                        cancelled_batch = true;
                        stop_reason = Some("turn cancellation".to_string());
                        failure_result("tool execution was cancelled".to_string())
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if fatal_error.is_none() {
                            cancellation.cancel();
                            stop_reason = Some(format!("tool batch aborted: {message}"));
                            fatal_error = Some(error);
                        }
                        failure_result(message)
                    }
                };

                if let Some(metrics) = &self.metrics {
                    metrics.tool(result.success);
                }
                self.persist_tool_result(&thread_id, &turn_id, &call, &result, &publisher)
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

    async fn execute_tool_with_progress(
        &self,
        mut context: ToolContext,
        call: &ToolCall,
        cancellation: CancellationToken,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<Option<ToolResult>, AgentRuntimeError> {
        let (progress_tx, mut progress_rx) = tool_progress_channel();
        context.progress = Some(progress_tx);
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
                            return Err(AgentRuntimeError::AuditCompensation {
                                storage_error,
                                rollback_error: rollback_error.to_string(),
                            });
                        }
                        return Err(error.into());
                    }
                    publisher.publish(AgentEventEnvelope::new(AgentEvent::ChangeApplied {
                        thread_id: context.thread_id.clone(),
                        turn_id: context.turn_id.clone(),
                        change_set,
                    }));
                }
                Ok(Some(result))
            }
        }
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
        let receiver = match self.user_inputs.register(&request_id).await {
            Ok(receiver) => receiver,
            Err(error) => return Ok(Some(failure_result(error.to_string()))),
        };
        if let Err(error) = self
            .repository
            .append(StoredEvent::new(
                &context.thread_id,
                Some(context.turn_id.clone()),
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
            thread_id: context.thread_id.clone(),
            turn_id: context.turn_id.clone(),
            request: request.clone(),
        }));
        let resolution = match self
            .user_inputs
            .wait(&request_id, receiver, cancellation.clone())
            .await
        {
            Ok(resolution) => resolution,
            Err(UserInputError::Cancelled) => {
                let resolution = UserInputResolution {
                    action: UserInputAction::Cancelled,
                    answers: Vec::new(),
                };
                self.persist_user_input_resolution(context, &request_id, &resolution, publisher)
                    .await?;
                return Ok(None);
            }
            Err(error) => {
                return Ok(Some(failure_result(error.to_string())));
            }
        };
        self.persist_user_input_resolution(context, &request_id, &resolution, publisher)
            .await?;
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
        Ok(())
    }

    async fn persist_user_input_resolution(
        &self,
        context: &ToolContext,
        request_id: &str,
        resolution: &crate::protocol::UserInputResolution,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<(), AgentRuntimeError> {
        self.repository
            .append(StoredEvent::new(
                &context.thread_id,
                Some(context.turn_id.clone()),
                StoredEventKind::UserInputResolved {
                    request_id: request_id.to_string(),
                    resolution: resolution.clone(),
                },
            ))
            .await?;
        publisher.publish(AgentEventEnvelope::new(AgentEvent::UserInputResolved {
            thread_id: context.thread_id.clone(),
            turn_id: context.turn_id.clone(),
            request_id: request_id.to_string(),
            resolution: resolution.clone(),
        }));
        Ok(())
    }

    async fn persist_tool_result(
        &self,
        thread_id: &str,
        turn_id: &str,
        call: &ToolCall,
        result: &ToolResult,
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
        Ok(())
    }

    async fn finish_completed(
        &self,
        thread_id: &str,
        turn_id: &str,
        text: String,
        usage: Option<TokenUsage>,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<TurnOutcome, AgentRuntimeError> {
        let message = text_message(MessageRole::Assistant, text);
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
        let timing = self
            .append_terminal_event(
                thread_id,
                turn_id,
                StoredEventKind::TurnFailed {
                    message: message.clone(),
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

fn provider_history(events: Vec<StoredEvent>) -> Vec<ProviderMessage> {
    let mut history = Vec::new();
    for event in events {
        let message = match event.kind {
            StoredEventKind::UserMessage { message }
            | StoredEventKind::AssistantMessage { message } => chat_to_provider(message),
            StoredEventKind::AssistantToolCalls { text, calls } => {
                Some(ProviderMessage::AssistantToolCalls { text, calls })
            }
            StoredEventKind::ToolResult {
                call_id,
                name,
                result,
            } => Some(ProviderMessage::ToolResult {
                call_id,
                name,
                success: result.success,
                output: result.output,
            }),
            StoredEventKind::ProviderContext { provider, item } => {
                Some(ProviderMessage::ProviderContext { provider, item })
            }
            StoredEventKind::ContextCompacted { summary, .. } => {
                history.clear();
                history.push(ProviderMessage::Text {
                    role: MessageRole::User,
                    text: context::render_summary(&summary),
                });
                None
            }
            _ => None,
        };
        if let Some(message) = message {
            history.push(message);
        }
    }
    context::repair_tool_history(history)
}

fn last_active_context_usage(events: &[StoredEvent]) -> Option<TokenUsage> {
    events.iter().fold(None, |usage, event| match &event.kind {
        StoredEventKind::ProviderCallUsage { usage, .. } => Some(*usage),
        StoredEventKind::ContextCompacted { .. } => None,
        _ => usage,
    })
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
    let mut end = MAX_TOOL_OUTPUT_BYTES;
    while end > 0 && !result.output.is_char_boundary(end) {
        end -= 1;
    }
    result.output.truncate(end);
    if !result.metadata.is_object() {
        result.metadata = json!({});
    }
    result.metadata["outputTruncated"] = Value::Bool(true);
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

fn text_message(role: MessageRole, text: String) -> ChatMessage {
    ChatMessage {
        schema_version: PROTOCOL_VERSION,
        id: Uuid::new_v4().to_string(),
        role,
        content: vec![ContentBlock::Text { text }],
        created_at_ms: now_ms(),
    }
}

fn user_message(
    text: String,
    attachments: Vec<ImageAttachment>,
) -> Result<ChatMessage, AgentRuntimeError> {
    if attachments.len() > MAX_IMAGE_COUNT {
        return Err(AgentRuntimeError::InvalidInput(format!(
            "at most {MAX_IMAGE_COUNT} images may be attached"
        )));
    }
    let mut total = 0usize;
    let mut content = if text.is_empty() {
        vec![ContentBlock::Context {
            text: "请分析用户提供的图片。".into(),
        }]
    } else {
        vec![ContentBlock::Text { text }]
    };
    for attachment in attachments {
        let name: String = attachment.name.chars().take(255).collect();
        let (_, encoded) = parse_image_data_url(&attachment.data_url)?;
        let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .map_err(|_| {
            AgentRuntimeError::InvalidInput("image data is not valid base64".into())
        })?;
        if decoded.len() > MAX_IMAGE_BYTES {
            return Err(AgentRuntimeError::InvalidInput(
                "an attached image exceeds the 4 MiB limit".into(),
            ));
        }
        total = total.saturating_add(decoded.len());
        if total > MAX_TOTAL_IMAGE_BYTES {
            return Err(AgentRuntimeError::InvalidInput(
                "attached images exceed the 8 MiB total limit".into(),
            ));
        }
        if let Some(ocr_text) = attachment
            .ocr_text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            let ocr_text = truncate_utf8(ocr_text, MAX_OCR_TEXT_BYTES);
            content.push(ContentBlock::Context {
                text: format!("\n\n[图片文字识别: {name}]\n{ocr_text}"),
            });
        }
        content.push(ContentBlock::Image {
            name,
            data_url: attachment.data_url,
        });
    }
    Ok(ChatMessage {
        schema_version: PROTOCOL_VERSION,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content,
        created_at_ms: now_ms(),
    })
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn parse_image_data_url(value: &str) -> Result<(&str, &str), AgentRuntimeError> {
    let (metadata, encoded) = value.split_once(',').ok_or_else(|| {
        AgentRuntimeError::InvalidInput("image attachment must be a data URL".into())
    })?;
    let media_type = metadata
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .ok_or_else(|| AgentRuntimeError::InvalidInput("image data URL must use base64".into()))?;
    if !matches!(
        media_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) {
        return Err(AgentRuntimeError::InvalidInput(
            "image type must be PNG, JPEG, GIF, or WebP".into(),
        ));
    }
    Ok((media_type, encoded))
}

fn chat_to_provider(message: ChatMessage) -> Option<ProviderMessage> {
    let text = message.text();
    let images = message
        .content
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Image { name, data_url } => Some(ProviderImage { name, data_url }),
            ContentBlock::Text { .. } | ContentBlock::Context { .. } => None,
        })
        .collect::<Vec<_>>();
    if message.role == MessageRole::User && !images.is_empty() {
        Some(ProviderMessage::UserContent { text, images })
    } else {
        Some(ProviderMessage::Text {
            role: message.role,
            text,
        })
    }
}

fn validate_input(input: &str, allow_empty: bool) -> Result<String, AgentRuntimeError> {
    let input = input.trim();
    if input.is_empty() && !allow_empty {
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
    use crate::protocol::{ToolDefinition, ToolRisk};
    use crate::providers::testing::FakeProvider;
    use crate::storage::{JsonlThreadRepository, TurnTimelineItem};
    use crate::tools::ToolHandler;

    #[test]
    fn validates_and_maps_bounded_image_attachments() {
        let message = user_message(
            "inspect this screenshot".into(),
            vec![ImageAttachment {
                name: "screen.png".into(),
                data_url: "data:image/png;base64,iVBORw0KGgo=".into(),
                ocr_text: Some("compiler error E0308".into()),
            }],
        )
        .unwrap();
        assert!(matches!(
            &message.content[1],
            ContentBlock::Context { text } if text.contains("compiler error E0308")
        ));
        assert!(matches!(
            chat_to_provider(message),
            Some(ProviderMessage::UserContent { text, images })
                if text.contains("inspect this screenshot")
                    && text.contains("compiler error E0308")
                    && images.len() == 1
        ));
        assert!(
            user_message(
                "bad image".into(),
                vec![ImageAttachment {
                    name: "bad.svg".into(),
                    data_url: "data:image/svg+xml;base64,PHN2Zy8+".into(),
                    ocr_text: None,
                }],
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
        )
        .unwrap();
        assert!(matches!(
            chat_to_provider(image_only),
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
    }

    impl EventPublisher for CancellingPublisher {
        fn publish(&self, event: AgentEventEnvelope) {
            if let AgentEvent::ToolStarted { call, .. } = event.event {
                self.started_calls.lock().unwrap().push(call.id);
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

        assert_eq!(runtime.with_context_limit(32_000).context_limit, 32_000);
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
        assert!(matches!(
            publisher
                .events
                .lock()
                .unwrap()
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
                publisher,
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

        let completed = runtime
            .retry_turn(
                Arc::new(FakeProvider::text(&["retry completed"])),
                "fake".to_string(),
                thread_id.clone(),
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();
        assert_eq!(completed.state, TurnState::Completed);

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
                Arc::new(RecordingPublisher::default()),
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
                .any(|event| matches!(
                    event.kind,
                    StoredEventKind::ContextCompacted {
                        automatic: true,
                        ..
                    }
                ))
        );
        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].messages.len() < requests[0].messages.len());
    }

    #[test]
    fn restored_compaction_renders_recent_tools_as_text() {
        let summary = CompactionSummary {
            contract_version: 1,
            summary: "repository inspected".to_string(),
            user_constraints: Vec::new(),
            recent_tool_results: vec![ProviderMessage::ToolResult {
                call_id: "orphaned-call".to_string(),
                name: "read_file".to_string(),
                success: true,
                output: "important result".to_string(),
            }],
            compacted_message_count: 10,
        };
        let history = provider_history(vec![StoredEvent::new(
            "thread",
            None,
            StoredEventKind::ContextCompacted {
                summary,
                automatic: true,
            },
        )]);

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
        let history = provider_history(vec![
            StoredEvent::new(
                "thread",
                Some("interrupted-turn".to_string()),
                StoredEventKind::AssistantToolCalls {
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
        ]);

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
                    agent_mode: None,
                },
                cancellation,
                Arc::new(RecordingPublisher::default()),
            )
            .await
            .unwrap();
        assert_eq!(result.state, TurnState::Cancelled);
        assert!(matches!(
            repository
                .load(&thread_id)
                .await
                .unwrap()
                .last()
                .map(|event| &event.kind),
            Some(StoredEventKind::TurnCancelled)
        ));
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
                publisher,
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
                publisher,
            )
            .await;

        assert!(matches!(result, Err(AgentRuntimeError::Storage(_))));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "before\n");
        assert!(
            inner
                .read_thread(&thread.id)
                .await
                .unwrap()
                .changes
                .is_empty()
        );
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
