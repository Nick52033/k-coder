use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::context::{self, CompactionSummary, DEFAULT_CONTEXT_LIMIT};
use crate::policy::{ApprovalError, ApprovalManager, PolicyDecision};
use crate::protocol::{
    AgentEvent, AgentEventEnvelope, ApprovalAction, ApprovalRequest, ApprovalResolution, ChangeSet,
    ChatMessage, ContentBlock, ExpectedFileHash, MessageRole, PROTOCOL_VERSION, PatchPreview,
    TokenUsage, ToolCall, ToolResult, TurnState,
};
use crate::providers::{Provider, ProviderError, ProviderEvent, ProviderMessage, ProviderRequest};
use crate::storage::{StorageError, StoredEvent, StoredEventKind, ThreadRepository, now_ms};
use crate::tools::{ApprovedToolExecution, ToolContext, ToolError, ToolRegistry};

const MAX_INPUT_BYTES: usize = 100_000;
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOOL_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_TOOL_ITERATIONS: usize = 8;
const MAX_TOOL_CALLS: usize = 24;
const MAX_IDENTICAL_TOOL_CALLS: usize = 2;

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
        }
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

    pub async fn compact_thread(
        &self,
        thread_id: &str,
    ) -> Result<CompactionSummary, AgentRuntimeError> {
        let history = provider_history(self.repository.load(thread_id).await?);
        let (summary, _) = context::compact(&history, DEFAULT_CONTEXT_LIMIT);
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
            self.repository
                .append(StoredEvent::new(
                    &thread_id,
                    None,
                    StoredEventKind::UserMessage {
                        message: text_message(MessageRole::User, input),
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

        let mut total_usage = TokenUsage::default();
        let mut has_usage = false;
        let mut tool_call_count = 0usize;
        let mut repeated_calls = HashMap::<String, usize>::new();

        for iteration in 0..MAX_TOOL_ITERATIONS {
            let mut history = provider_history(self.repository.load(&thread_id).await?);
            if context::needs_compaction(&history, DEFAULT_CONTEXT_LIMIT) {
                let (summary, compacted) = context::compact(&history, DEFAULT_CONTEXT_LIMIT);
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
            let request = ProviderRequest {
                schema_version: PROTOCOL_VERSION,
                model: model.clone(),
                messages: history,
                tools: self.tools.definitions(),
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
            let mut calls = Vec::new();
            let mut iteration_usage = None;
            let completed = loop {
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
                                    format!("response_limit: provider response exceeds {MAX_RESPONSE_BYTES} bytes"),
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
                    Some(Ok(ProviderEvent::ToolCall { call })) => calls.push(call),
                    Some(Ok(ProviderEvent::ProviderContext { provider, item })) => {
                        self.repository
                            .append(StoredEvent::new(
                                &thread_id,
                                Some(turn_id.clone()),
                                StoredEventKind::ProviderContext { provider, item },
                            ))
                            .await?;
                    }
                    Some(Ok(ProviderEvent::Usage { usage })) => {
                        iteration_usage = Some(usage);
                        let aggregate = add_usage(total_usage, usage);
                        publisher.publish(AgentEventEnvelope::new(AgentEvent::UsageUpdated {
                            thread_id: thread_id.clone(),
                            turn_id: turn_id.clone(),
                            usage: aggregate,
                        }));
                    }
                    Some(Ok(ProviderEvent::Completed)) => break true,
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
                    None => break false,
                }
            };

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
            }

            if calls.is_empty() {
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

            if iteration + 1 >= MAX_TOOL_ITERATIONS {
                return self
                    .finish_failed(
                        &thread_id,
                        &turn_id,
                        format!("tool_iteration_limit: exceeded {MAX_TOOL_ITERATIONS} provider iterations"),
                        &publisher,
                    )
                    .await;
            }
            if tool_call_count.saturating_add(calls.len()) > MAX_TOOL_CALLS {
                return self
                    .finish_failed(
                        &thread_id,
                        &turn_id,
                        format!("tool_call_limit: exceeded {MAX_TOOL_CALLS} calls in one turn"),
                        &publisher,
                    )
                    .await;
            }
            tool_call_count += calls.len();
            self.repository
                .append(StoredEvent::new(
                    &thread_id,
                    Some(turn_id.clone()),
                    StoredEventKind::AssistantToolCalls {
                        calls: calls.clone(),
                    },
                ))
                .await?;

            let mut stop_reason = None;
            let mut cancelled_batch = false;
            for call in calls {
                publisher.publish(AgentEventEnvelope::new(AgentEvent::ToolStarted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    call: call.clone(),
                }));

                let result = if let Some(reason) = &stop_reason {
                    failure_result(format!("tool execution skipped: {reason}"))
                } else {
                    let signature = call_signature(&call);
                    let repeat = repeated_calls.entry(signature).or_default();
                    *repeat += 1;
                    if *repeat > MAX_IDENTICAL_TOOL_CALLS {
                        let reason = format!(
                            "repeated_tool_call: {} was requested with identical arguments more than {MAX_IDENTICAL_TOOL_CALLS} times",
                            call.name
                        );
                        stop_reason = Some(reason.clone());
                        failure_result(reason)
                    } else {
                        let context = ToolContext {
                            thread_id: thread_id.clone(),
                            turn_id: turn_id.clone(),
                            call_id: call.id.clone(),
                            workspace_root: self.workspace_root.clone(),
                            approval: None,
                        };
                        match self
                            .execute_tool_call(&context, &call, cancellation.clone(), &publisher)
                            .await?
                        {
                            Some(result) => bound_tool_result(result),
                            None => {
                                cancelled_batch = true;
                                stop_reason = Some("turn cancellation".to_string());
                                failure_result("tool execution was cancelled".to_string())
                            }
                        }
                    }
                };
                self.persist_tool_result(&thread_id, &turn_id, &call, &result, &publisher)
                    .await?;
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
        }

        self.finish_failed(
            &thread_id,
            &turn_id,
            format!("tool_iteration_limit: exceeded {MAX_TOOL_ITERATIONS} provider iterations"),
            &publisher,
        )
        .await
    }

    async fn execute_tool_call(
        &self,
        context: &ToolContext,
        call: &ToolCall,
        cancellation: CancellationToken,
        publisher: &Arc<dyn EventPublisher>,
    ) -> Result<Option<ToolResult>, AgentRuntimeError> {
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
                let Some(preview) = preview else {
                    return Ok(Some(failure_result(
                        "approval_required: tool did not provide a reviewable preview".to_string(),
                    )));
                };
                let request_id = Uuid::new_v4().to_string();
                let created_at_ms = now_ms();
                let request = ApprovalRequest {
                    id: request_id.clone(),
                    thread_id: context.thread_id.clone(),
                    turn_id: context.turn_id.clone(),
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    reason,
                    risk: authorization.risk,
                    arguments: call.arguments.clone(),
                    preview: Some(preview.clone()),
                    created_at_ms,
                    expires_at_ms: created_at_ms.saturating_add(self.approvals.timeout_ms()),
                };
                let receiver = match self.approvals.register(&request_id).await {
                    Ok(receiver) => receiver,
                    Err(error) => return Ok(Some(failure_result(error.to_string()))),
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
                    self.approvals.discard(&request_id).await;
                    return Err(error.into());
                }
                publisher.publish(AgentEventEnvelope::new(AgentEvent::ApprovalRequested {
                    thread_id: context.thread_id.clone(),
                    turn_id: context.turn_id.clone(),
                    request,
                }));

                let resolution = match self
                    .approvals
                    .wait(&request_id, receiver, cancellation.clone())
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

fn provider_history(events: Vec<StoredEvent>) -> Vec<ProviderMessage> {
    let mut history = Vec::new();
    for event in events {
        let message = match event.kind {
            StoredEventKind::UserMessage { message }
            | StoredEventKind::AssistantMessage { message } => {
                let text = message.text();
                Some(ProviderMessage::Text {
                    role: message.role,
                    text,
                })
            }
            StoredEventKind::AssistantToolCalls { calls } => {
                Some(ProviderMessage::AssistantToolCalls { calls })
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
                history.extend(summary.recent_tool_results);
                None
            }
            _ => None,
        };
        if let Some(message) = message {
            history.push(message);
        }
    }
    history
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
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;

    use super::*;
    use crate::protocol::ToolDefinition;
    use crate::providers::testing::FakeProvider;
    use crate::storage::JsonlThreadRepository;
    use crate::tools::ToolHandler;

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
    }

    impl EventPublisher for CancellingPublisher {
        fn publish(&self, event: AgentEventEnvelope) {
            if matches!(event.event, AgentEvent::ToolStarted { .. }) {
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
            .unwrap();
        let detail = repository.read_thread(&thread_id).await.unwrap();
        assert_eq!(result.state, TurnState::Completed);
        assert_eq!(detail.messages[1].text(), "hello world");
        assert_eq!(provider.requests()[0].messages.len(), 1);
        assert!(matches!(
            publisher
                .events
                .lock()
                .unwrap()
                .last()
                .map(|event| &event.event),
            Some(AgentEvent::TurnCompleted { .. })
        ));
    }

    #[tokio::test]
    async fn executes_a_native_tool_and_continues_until_final_text() {
        let (directory, repository, runtime, thread_id) = runtime_fixture().await;
        std::fs::write(directory.path().join("README.md"), "workspace docs").unwrap();
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "read_file".to_string(),
            arguments: json!({ "path": "README.md" }),
            metadata: json!({}),
        };
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
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
        assert_eq!(
            repository
                .read_thread(&thread_id)
                .await
                .unwrap()
                .tool_activities
                .len(),
            1
        );
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
        });

        let outcome = runtime
            .run_turn(
                provider,
                "fake".to_string(),
                RunTurnRequest {
                    thread_id: thread.id.clone(),
                    input: "cancel tools".to_string(),
                },
                cancellation,
                publisher,
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
    }

    fn patch_call(patch: &str) -> ToolCall {
        ToolCall {
            id: "patch-call".to_string(),
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
