use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::{AgentRuntime, EventPublisher, RunTurnRequest};
use crate::policy::ApprovalManager;
use crate::protocol::{
    AgentEvent, AgentEventEnvelope, ApprovalMode, ReasoningEffort, TokenUsage, ToolDefinition,
    ToolResult, ToolRisk, TurnState,
};
use crate::providers::Provider;
use crate::storage::{JsonlThreadRepository, StoredEventKind, ThreadRepository, now_ms};
use crate::tools::{ToolContext, ToolError, ToolHandler, ToolRegistry};

pub const MAX_SUBAGENT_DEPTH: u8 = 1;
pub const MAX_ACTIVE_SUBAGENTS: usize = 4;
pub const MAX_SUBAGENT_RUNTIME_MS: u64 = 30 * 60 * 1_000;
const DEFAULT_SUBAGENT_RUNTIME_MS: u64 = 10 * 60 * 1_000;
const MAX_TASK_BYTES: usize = 100_000;
const MAX_MESSAGE_BYTES: usize = 100_000;
const MAX_SUMMARY_BYTES: usize = 32 * 1024;
const STORE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubagentState {
    Queued,
    Running,
    Blocked,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl SubagentState {
    fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Blocked)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateSubagentRequest {
    pub parent_thread_id: String,
    pub task: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    DEFAULT_SUBAGENT_RUNTIME_MS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentView {
    pub schema_version: u32,
    pub id: String,
    pub parent_agent_id: Option<String>,
    pub parent_thread_id: String,
    pub thread_id: String,
    pub label: String,
    pub task: String,
    pub state: SubagentState,
    pub depth: u8,
    pub workspace_root: String,
    pub capabilities: Vec<String>,
    pub token_budget: Option<u64>,
    pub tokens_used: u64,
    pub timeout_ms: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub summary: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSubagentSnapshot {
    schema_version: u32,
    snapshot: SubagentView,
}

pub trait SubagentEventPublisher: Send + Sync {
    fn publish(&self, view: SubagentView);
}

#[derive(Default)]
pub struct NoopSubagentPublisher;

impl SubagentEventPublisher for NoopSubagentPublisher {
    fn publish(&self, _view: SubagentView) {}
}

#[derive(Clone)]
pub struct SubagentExecutionContext {
    pub repository: Arc<JsonlThreadRepository>,
    pub provider: Arc<dyn Provider>,
    pub model: String,
    pub context_limit: usize,
    pub tools: ToolRegistry,
    pub workspace_root: PathBuf,
    pub approvals: Arc<ApprovalManager>,
    pub approval_mode: ApprovalMode,
    pub reasoning_effort: ReasoningEffort,
    pub agent_events: Arc<dyn EventPublisher>,
    pub lifecycle_events: Arc<dyn SubagentEventPublisher>,
}

struct ActiveSubagent {
    cancellation: CancellationToken,
    notify: Arc<Notify>,
    _permit: OwnedSemaphorePermit,
}

struct SubagentStore {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl SubagentStore {
    fn open(data_root: &Path) -> Result<(Self, HashMap<String, SubagentView>), MultiAgentError> {
        fs::create_dir_all(data_root)
            .map_err(|error| MultiAgentError::Storage(error.to_string()))?;
        let path = data_root.join("subagents.jsonl");
        let records = if path.exists() {
            load_snapshots(&path)?
        } else {
            HashMap::new()
        };
        Ok((
            Self {
                path,
                write_lock: Mutex::new(()),
            },
            records,
        ))
    }

    fn append(&self, snapshot: &SubagentView) -> Result<(), MultiAgentError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| MultiAgentError::Storage("subagent store lock poisoned".into()))?;
        let payload = StoredSubagentSnapshot {
            schema_version: STORE_SCHEMA_VERSION,
            snapshot: snapshot.clone(),
        };
        let bytes = serde_json::to_vec(&payload)
            .map_err(|error| MultiAgentError::Storage(error.to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| MultiAgentError::Storage(error.to_string()))?;
        file.write_all(&bytes)
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.sync_data())
            .map_err(|error| MultiAgentError::Storage(error.to_string()))
    }
}

fn load_snapshots(path: &Path) -> Result<HashMap<String, SubagentView>, MultiAgentError> {
    let bytes = fs::read(path).map_err(|error| MultiAgentError::Storage(error.to_string()))?;
    let complete = bytes.ends_with(b"\n");
    let lines = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let mut records = HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        let parsed = serde_json::from_slice::<StoredSubagentSnapshot>(line);
        let event = match parsed {
            Ok(event) => event,
            Err(_) if index + 1 == lines.len() && !complete => break,
            Err(error) => return Err(MultiAgentError::Storage(error.to_string())),
        };
        if event.schema_version != STORE_SCHEMA_VERSION {
            return Err(MultiAgentError::Storage(format!(
                "unsupported subagent schema version {}",
                event.schema_version
            )));
        }
        records.insert(event.snapshot.id.clone(), event.snapshot);
    }
    Ok(records)
}

struct CoordinatorInner {
    store: SubagentStore,
    records: Mutex<HashMap<String, SubagentView>>,
    active: Mutex<HashMap<String, ActiveSubagent>>,
    permits: Arc<Semaphore>,
}

#[derive(Clone)]
pub struct MultiAgentCoordinator {
    inner: Arc<CoordinatorInner>,
}

impl MultiAgentCoordinator {
    pub fn new(data_root: impl AsRef<Path>) -> Result<Self, MultiAgentError> {
        let (store, mut records) = SubagentStore::open(data_root.as_ref())?;
        let interrupted = records
            .values()
            .filter(|record| record.state.is_active())
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        for id in interrupted {
            if let Some(record) = records.get_mut(&id) {
                record.state = SubagentState::Failed;
                record.error = Some("subagent was interrupted by an application restart".into());
                record.updated_at_ms = now_ms();
                store.append(record)?;
            }
        }
        Ok(Self {
            inner: Arc::new(CoordinatorInner {
                store,
                records: Mutex::new(records),
                active: Mutex::new(HashMap::new()),
                permits: Arc::new(Semaphore::new(MAX_ACTIVE_SUBAGENTS)),
            }),
        })
    }

    pub fn list(&self, parent_thread_id: Option<&str>) -> Vec<SubagentView> {
        let mut records = self
            .inner
            .records
            .lock()
            .expect("subagent record lock poisoned")
            .values()
            .filter(|record| {
                parent_thread_id.is_none_or(|parent| record.parent_thread_id == parent)
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
        records
    }

    pub fn has_active(&self) -> bool {
        self.inner
            .active
            .lock()
            .map(|active| !active.is_empty())
            .unwrap_or(true)
    }

    pub fn get(&self, id: &str) -> Result<SubagentView, MultiAgentError> {
        self.inner
            .records
            .lock()
            .map_err(|_| MultiAgentError::Storage("subagent record lock poisoned".into()))?
            .get(id)
            .cloned()
            .ok_or_else(|| MultiAgentError::NotFound(id.into()))
    }

    pub async fn create(
        &self,
        request: CreateSubagentRequest,
        parent_agent_id: Option<String>,
        context: SubagentExecutionContext,
        parent_cancellation: CancellationToken,
    ) -> Result<SubagentView, MultiAgentError> {
        validate_request(&request, &context.tools, parent_agent_id.as_deref(), self)?;
        let permit = self.acquire_permit()?;
        let thread = context
            .repository
            .create_thread()
            .await
            .map_err(|error| MultiAgentError::Runtime(error.to_string()))?;
        let depth = parent_agent_id
            .as_deref()
            .map(|id| self.get(id).map(|parent| parent.depth + 1))
            .transpose()?
            .unwrap_or(1);
        let id = Uuid::new_v4().to_string();
        let timestamp = now_ms();
        let capabilities = normalized_capabilities(&request.capabilities);
        let record = SubagentView {
            schema_version: STORE_SCHEMA_VERSION,
            id: id.clone(),
            parent_agent_id,
            parent_thread_id: request.parent_thread_id,
            thread_id: thread.id,
            label: request
                .label
                .filter(|label| !label.trim().is_empty())
                .unwrap_or_else(|| task_label(&request.task)),
            task: request.task.trim().to_string(),
            state: SubagentState::Queued,
            depth,
            workspace_root: context.workspace_root.to_string_lossy().into_owned(),
            capabilities,
            token_budget: request.token_budget,
            tokens_used: 0,
            timeout_ms: request.timeout_ms,
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
            summary: None,
            error: None,
        };
        self.store_record(record.clone(), &context.lifecycle_events)?;
        self.launch(
            id,
            Some(record.task.clone()),
            false,
            context,
            parent_cancellation,
            permit,
        )
        .await?;
        self.get(&record.id)
    }

    pub async fn send_message(
        &self,
        id: &str,
        message: String,
        context: SubagentExecutionContext,
    ) -> Result<SubagentView, MultiAgentError> {
        if message.trim().is_empty() || message.len() > MAX_MESSAGE_BYTES {
            return Err(MultiAgentError::Invalid(
                "message must contain 1 to 100000 bytes".into(),
            ));
        }
        self.ensure_inactive(id)?;
        let permit = self.acquire_permit()?;
        self.launch(
            id.to_string(),
            Some(message.trim().into()),
            false,
            context,
            CancellationToken::new(),
            permit,
        )
        .await?;
        self.get(id)
    }

    pub async fn resume(
        &self,
        id: &str,
        message: Option<String>,
        context: SubagentExecutionContext,
    ) -> Result<SubagentView, MultiAgentError> {
        self.ensure_inactive(id)?;
        let permit = self.acquire_permit()?;
        let record = self.get(id)?;
        let input = message
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if input.is_none()
            && !matches!(
                record.state,
                SubagentState::Failed | SubagentState::Cancelled | SubagentState::TimedOut
            )
        {
            return Err(MultiAgentError::Invalid(
                "a completed subagent requires a new message".into(),
            ));
        }
        let retry = input.is_none();
        self.launch(
            id.to_string(),
            input,
            retry,
            context,
            CancellationToken::new(),
            permit,
        )
        .await?;
        self.get(id)
    }

    async fn launch(
        &self,
        id: String,
        input: Option<String>,
        retry: bool,
        context: SubagentExecutionContext,
        parent_cancellation: CancellationToken,
        permit: OwnedSemaphorePermit,
    ) -> Result<(), MultiAgentError> {
        let record = self.get(&id)?;
        let allowed_tools = context.tools.restricted_to(&record.capabilities)?;
        let cancellation = parent_cancellation.child_token();
        let notify = Arc::new(Notify::new());
        self.inner
            .active
            .lock()
            .map_err(|_| MultiAgentError::Storage("subagent active lock poisoned".into()))?
            .insert(
                id.clone(),
                ActiveSubagent {
                    cancellation: cancellation.clone(),
                    notify: notify.clone(),
                    _permit: permit,
                },
            );
        self.transition(
            &id,
            SubagentState::Running,
            None,
            None,
            None,
            &context.lifecycle_events,
        )?;
        let manager = self.clone();
        tokio::spawn(async move {
            let remaining_tokens = record
                .token_budget
                .map(|budget| budget.saturating_sub(record.tokens_used));
            let mut runtime = AgentRuntime::with_tools_and_approvals(
                context.repository.clone(), allowed_tools, context.workspace_root.clone(), context.approvals.clone(),
            ).with_approval_mode(context.approval_mode).with_runtime_instructions(
                "You are a bounded subagent. Complete only the delegated task. Return a concise result for the parent agent; do not claim access outside your provided tools or workspace.".into(),
            )
            .with_context_limit(context.context_limit)
            .with_reasoning_effort(context.reasoning_effort);
            if let Some(remaining_tokens) = remaining_tokens {
                runtime = runtime.with_token_budget(remaining_tokens);
            }
            let publisher: Arc<dyn EventPublisher> = Arc::new(ChildEventPublisher {
                agent_id: id.clone(),
                manager: manager.clone(),
                downstream: context.agent_events.clone(),
                lifecycle: context.lifecycle_events.clone(),
            });
            let future = async {
                if retry {
                    runtime
                        .retry_turn(
                            context.provider.clone(),
                            context.model.clone(),
                            record.thread_id.clone(),
                            cancellation.clone(),
                            publisher,
                        )
                        .await
                } else {
                    runtime
                        .run_turn(
                            context.provider.clone(),
                            context.model.clone(),
                            RunTurnRequest {
                                thread_id: record.thread_id.clone(),
                                input: input.unwrap_or_default(),
                                agent_mode: None,
                            },
                            cancellation.clone(),
                            publisher,
                        )
                        .await
                }
            };
            tokio::pin!(future);
            let mut timed_out = false;
            let result = tokio::select! {
                result = &mut future => Some(result),
                _ = tokio::time::sleep(Duration::from_millis(record.timeout_ms)) => {
                    timed_out = true;
                    cancellation.cancel();
                    tokio::time::timeout(Duration::from_secs(5), &mut future).await.ok()
                }
            };
            let (state, error, turn_id) = if timed_out {
                (
                    SubagentState::TimedOut,
                    Some(format!("subagent exceeded {} ms", record.timeout_ms)),
                    None,
                )
            } else {
                match result {
                    Some(Ok(outcome)) => (
                        map_turn_state(outcome.state),
                        outcome.error,
                        Some(outcome.turn_id),
                    ),
                    Some(Err(error)) => (SubagentState::Failed, Some(error.to_string()), None),
                    None => (
                        SubagentState::Failed,
                        Some("subagent did not stop after cancellation".into()),
                        None,
                    ),
                }
            };
            let (summary, usage) =
                summarize_thread(&context.repository, &record.thread_id, turn_id.as_deref()).await;
            let _ = manager.transition(
                &id,
                state,
                summary,
                error,
                Some(record.tokens_used.saturating_add(usage.total_tokens)),
                &context.lifecycle_events,
            );
            if let Ok(mut active) = manager.inner.active.lock() {
                active.remove(&id);
            }
            notify.notify_waiters();
        });
        Ok(())
    }

    pub async fn wait(
        &self,
        id: &str,
        timeout_ms: u64,
        cancellation: CancellationToken,
    ) -> Result<SubagentView, MultiAgentError> {
        loop {
            let record = self.get(id)?;
            if !record.state.is_active() {
                return Ok(record);
            }
            let notify = self
                .inner
                .active
                .lock()
                .map_err(|_| MultiAgentError::Storage("subagent active lock poisoned".into()))?
                .get(id)
                .map(|active| active.notify.clone())
                .ok_or_else(|| {
                    MultiAgentError::Runtime("active subagent has no runtime handle".into())
                })?;
            tokio::select! {
                _ = notify.notified() => {},
                _ = cancellation.cancelled() => return Err(MultiAgentError::Cancelled),
                _ = tokio::time::sleep(Duration::from_millis(timeout_ms.min(MAX_SUBAGENT_RUNTIME_MS))) => return Ok(self.get(id)?),
            }
        }
    }

    pub fn close(&self, id: &str) -> Result<SubagentView, MultiAgentError> {
        if let Some(active) = self
            .inner
            .active
            .lock()
            .map_err(|_| MultiAgentError::Storage("subagent active lock poisoned".into()))?
            .get(id)
        {
            active.cancellation.cancel();
        }
        self.get(id)
    }

    pub fn cancel_for_parent(&self, parent_thread_id: &str) {
        let ids = self
            .list(Some(parent_thread_id))
            .into_iter()
            .filter(|record| record.state.is_active())
            .map(|record| record.id)
            .collect::<HashSet<_>>();
        if let Ok(active) = self.inner.active.lock() {
            for id in ids {
                if let Some(agent) = active.get(&id) {
                    agent.cancellation.cancel();
                }
            }
        }
    }

    fn ensure_inactive(&self, id: &str) -> Result<(), MultiAgentError> {
        let record = self.get(id)?;
        if record.state.is_active() {
            Err(MultiAgentError::AlreadyActive(id.into()))
        } else if record
            .token_budget
            .is_some_and(|budget| record.tokens_used >= budget)
        {
            Err(MultiAgentError::Limit(
                "subagent token budget is exhausted".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn acquire_permit(&self) -> Result<OwnedSemaphorePermit, MultiAgentError> {
        self.inner.permits.clone().try_acquire_owned().map_err(|_| {
            MultiAgentError::Limit(format!(
                "at most {MAX_ACTIVE_SUBAGENTS} subagents may run concurrently"
            ))
        })
    }

    fn store_record(
        &self,
        record: SubagentView,
        publisher: &Arc<dyn SubagentEventPublisher>,
    ) -> Result<(), MultiAgentError> {
        self.inner.store.append(&record)?;
        self.inner
            .records
            .lock()
            .map_err(|_| MultiAgentError::Storage("subagent record lock poisoned".into()))?
            .insert(record.id.clone(), record.clone());
        publisher.publish(record);
        Ok(())
    }

    fn transition(
        &self,
        id: &str,
        state: SubagentState,
        summary: Option<String>,
        error: Option<String>,
        tokens_used: Option<u64>,
        publisher: &Arc<dyn SubagentEventPublisher>,
    ) -> Result<(), MultiAgentError> {
        let record = {
            let mut records =
                self.inner.records.lock().map_err(|_| {
                    MultiAgentError::Storage("subagent record lock poisoned".into())
                })?;
            let record = records
                .get_mut(id)
                .ok_or_else(|| MultiAgentError::NotFound(id.into()))?;
            let previous_state = record.state;
            record.state = state;
            record.updated_at_ms = now_ms();
            if summary.is_some()
                || !state.is_active()
                || (state == SubagentState::Running && !previous_state.is_active())
            {
                record.summary = summary;
            }
            record.error = error;
            if let Some(tokens) = tokens_used {
                record.tokens_used = tokens;
            }
            record.clone()
        };
        self.inner.store.append(&record)?;
        publisher.publish(record);
        Ok(())
    }
}

fn validate_request(
    request: &CreateSubagentRequest,
    tools: &ToolRegistry,
    parent_agent_id: Option<&str>,
    manager: &MultiAgentCoordinator,
) -> Result<(), MultiAgentError> {
    let task = request.task.trim();
    if task.is_empty() || task.len() > MAX_TASK_BYTES {
        return Err(MultiAgentError::Invalid(
            "task must contain 1 to 100000 bytes".into(),
        ));
    }
    if request.token_budget == Some(0) {
        return Err(MultiAgentError::Limit(
            "token budget must be greater than zero when provided".into(),
        ));
    }
    if request.timeout_ms == 0 || request.timeout_ms > MAX_SUBAGENT_RUNTIME_MS {
        return Err(MultiAgentError::Limit(format!(
            "timeout must be between 1 and {MAX_SUBAGENT_RUNTIME_MS} ms"
        )));
    }
    if parent_agent_id.is_some_and(|id| {
        manager
            .get(id)
            .map_or(true, |parent| parent.depth >= MAX_SUBAGENT_DEPTH)
    }) {
        return Err(MultiAgentError::Limit(format!(
            "maximum subagent depth is {MAX_SUBAGENT_DEPTH}"
        )));
    }
    tools.restricted_to(&normalized_capabilities(&request.capabilities))?;
    Ok(())
}

fn normalized_capabilities(capabilities: &[String]) -> Vec<String> {
    let mut values = if capabilities.is_empty() {
        vec!["list_directory".into(), "read_file".into()]
    } else {
        capabilities.to_vec()
    };
    values.sort();
    values.dedup();
    values
}

fn task_label(task: &str) -> String {
    let label = task.trim().chars().take(36).collect::<String>();
    if label.is_empty() {
        "子任务".into()
    } else {
        label
    }
}

fn map_turn_state(state: TurnState) -> SubagentState {
    match state {
        TurnState::Completed => SubagentState::Completed,
        TurnState::Cancelled => SubagentState::Cancelled,
        TurnState::Failed => SubagentState::Failed,
        TurnState::AwaitingApproval => SubagentState::Blocked,
        _ => SubagentState::Running,
    }
}

async fn summarize_thread(
    repository: &JsonlThreadRepository,
    thread_id: &str,
    turn_id: Option<&str>,
) -> (Option<String>, TokenUsage) {
    let events = repository.load(thread_id).await.unwrap_or_default();
    let summary = events.iter().rev().find_map(|event| {
        if turn_id.is_some_and(|id| event.turn_id.as_deref() != Some(id)) {
            return None;
        }
        match &event.kind {
            StoredEventKind::AssistantMessage { message } => Some(bounded_summary(&message.text())),
            _ => None,
        }
    });
    let usage = events
        .into_iter()
        .fold(TokenUsage::default(), |mut total, event| {
            if turn_id.is_some_and(|id| event.turn_id.as_deref() != Some(id)) {
                return total;
            }
            if let StoredEventKind::ProviderCallUsage { usage, .. } = event.kind {
                total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
                total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
                total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
            }
            total
        });
    (summary, usage)
}

fn bounded_summary(value: &str) -> String {
    if value.len() <= MAX_SUMMARY_BYTES {
        return value.to_string();
    }
    let mut end = MAX_SUMMARY_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[摘要已截断]", &value[..end])
}

struct ChildEventPublisher {
    agent_id: String,
    manager: MultiAgentCoordinator,
    downstream: Arc<dyn EventPublisher>,
    lifecycle: Arc<dyn SubagentEventPublisher>,
}

impl EventPublisher for ChildEventPublisher {
    fn publish(&self, event: AgentEventEnvelope) {
        let state = match &event.event {
            AgentEvent::ApprovalRequested { .. } => Some(SubagentState::Blocked),
            AgentEvent::ApprovalResolved { .. } | AgentEvent::ToolStarted { .. } => {
                Some(SubagentState::Running)
            }
            _ => None,
        };
        if let Some(state) = state {
            let _ =
                self.manager
                    .transition(&self.agent_id, state, None, None, None, &self.lifecycle);
        }
        self.downstream.publish(event);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MultiAgentError {
    #[error("subagent input is invalid: {0}")]
    Invalid(String),
    #[error("subagent limit exceeded: {0}")]
    Limit(String),
    #[error("subagent was not found: {0}")]
    NotFound(String),
    #[error("subagent is already active: {0}")]
    AlreadyActive(String),
    #[error("subagent storage failed: {0}")]
    Storage(String),
    #[error("subagent runtime failed: {0}")]
    Runtime(String),
    #[error("subagent operation was cancelled")]
    Cancelled,
    #[error(transparent)]
    Tool(#[from] ToolError),
}

#[derive(Clone)]
struct AgentToolHandler {
    operation: AgentToolOperation,
    manager: MultiAgentCoordinator,
    context: SubagentExecutionContext,
    parent_thread_id: String,
    parent_cancellation: CancellationToken,
}

#[derive(Clone, Copy)]
enum AgentToolOperation {
    Create,
    Wait,
    Send,
    Resume,
    List,
    Close,
}

pub fn delegation_tools(
    manager: MultiAgentCoordinator,
    context: SubagentExecutionContext,
    parent_thread_id: String,
    parent_cancellation: CancellationToken,
) -> (Vec<Arc<dyn ToolHandler>>, HashMap<String, ToolRisk>) {
    let operations = [
        AgentToolOperation::Create,
        AgentToolOperation::Wait,
        AgentToolOperation::Send,
        AgentToolOperation::Resume,
        AgentToolOperation::List,
        AgentToolOperation::Close,
    ];
    let mut handlers = Vec::new();
    let mut risks = HashMap::new();
    for operation in operations {
        let handler = AgentToolHandler {
            operation,
            manager: manager.clone(),
            context: context.clone(),
            parent_thread_id: parent_thread_id.clone(),
            parent_cancellation: parent_cancellation.clone(),
        };
        let definition = handler.definition();
        risks.insert(
            definition.name.clone(),
            match operation {
                AgentToolOperation::Wait | AgentToolOperation::List => ToolRisk::Read,
                _ => ToolRisk::External,
            },
        );
        handlers.push(Arc::new(handler) as Arc<dyn ToolHandler>);
    }
    (handlers, risks)
}

#[async_trait]
impl ToolHandler for AgentToolHandler {
    fn definition(&self) -> ToolDefinition {
        let (name, description, properties, required) = match self.operation {
            AgentToolOperation::Create => (
                "create_agent",
                "Create a bounded subagent for an independent task.",
                json!({"task":{"type":"string"},"label":{"type":"string"},"capabilities":{"type":"array","items":{"type":"string"}},"tokenBudget":{"type":"integer","minimum":1},"timeoutMs":{"type":"integer","minimum":1}}),
                vec!["task"],
            ),
            AgentToolOperation::Wait => (
                "wait_agent",
                "Wait for a subagent and return its structured status and summary.",
                json!({"agentId":{"type":"string"},"timeoutMs":{"type":"integer","minimum":1}}),
                vec!["agentId"],
            ),
            AgentToolOperation::Send => (
                "send_agent_message",
                "Send a follow-up message to an inactive subagent.",
                json!({"agentId":{"type":"string"},"message":{"type":"string"}}),
                vec!["agentId", "message"],
            ),
            AgentToolOperation::Resume => (
                "resume_agent",
                "Resume a failed or cancelled subagent, optionally with a new message.",
                json!({"agentId":{"type":"string"},"message":{"type":"string"}}),
                vec!["agentId"],
            ),
            AgentToolOperation::List => (
                "list_agents",
                "List subagents owned by the current parent thread.",
                json!({}),
                vec![],
            ),
            AgentToolOperation::Close => (
                "close_agent",
                "Cancel and close an active subagent.",
                json!({"agentId":{"type":"string"}}),
                vec!["agentId"],
            ),
        };
        ToolDefinition {
            name: name.into(),
            description: description.into(),
            input_schema: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        }
    }

    async fn execute(
        &self,
        _tool_context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let result = match self.operation {
            AgentToolOperation::Create => {
                let task = string_arg(&arguments, "task")?;
                let capabilities = arguments
                    .get("capabilities")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                self.manager
                    .create(
                        CreateSubagentRequest {
                            parent_thread_id: self.parent_thread_id.clone(),
                            task,
                            label: optional_string_arg(&arguments, "label"),
                            capabilities,
                            token_budget: arguments.get("tokenBudget").and_then(Value::as_u64),
                            timeout_ms: arguments
                                .get("timeoutMs")
                                .and_then(Value::as_u64)
                                .unwrap_or(DEFAULT_SUBAGENT_RUNTIME_MS),
                        },
                        None,
                        self.context.clone(),
                        self.parent_cancellation.child_token(),
                    )
                    .await
            }
            AgentToolOperation::Wait => {
                let agent_id = self.owned_agent_id(&arguments)?;
                self.manager
                    .wait(
                        &agent_id,
                        arguments
                            .get("timeoutMs")
                            .and_then(Value::as_u64)
                            .unwrap_or(30_000),
                        cancellation,
                    )
                    .await
            }
            AgentToolOperation::Send => {
                let agent_id = self.owned_agent_id(&arguments)?;
                self.manager
                    .send_message(
                        &agent_id,
                        string_arg(&arguments, "message")?,
                        self.context.clone(),
                    )
                    .await
            }
            AgentToolOperation::Resume => {
                let agent_id = self.owned_agent_id(&arguments)?;
                self.manager
                    .resume(
                        &agent_id,
                        optional_string_arg(&arguments, "message"),
                        self.context.clone(),
                    )
                    .await
            }
            AgentToolOperation::List => {
                return tool_json(&self.manager.list(Some(&self.parent_thread_id)));
            }
            AgentToolOperation::Close => {
                let agent_id = self.owned_agent_id(&arguments)?;
                self.manager.close(&agent_id)
            }
        }
        .map_err(|error| ToolError::Execution(error.to_string()))?;
        if result.parent_thread_id != self.parent_thread_id {
            return Err(ToolError::Denied(
                "subagent belongs to another parent thread".into(),
            ));
        }
        tool_json(&result)
    }
}

impl AgentToolHandler {
    fn owned_agent_id(&self, arguments: &Value) -> Result<String, ToolError> {
        let id = string_arg(arguments, "agentId")?;
        let record = self
            .manager
            .get(&id)
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        if record.parent_thread_id != self.parent_thread_id {
            return Err(ToolError::Denied(
                "subagent belongs to another parent thread".into(),
            ));
        }
        Ok(id)
    }
}

fn string_arg(arguments: &Value, name: &str) -> Result<String, ToolError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ToolError::InvalidArguments(format!("{name} must be a string")))
}
fn optional_string_arg(arguments: &Value, name: &str) -> Option<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}
fn tool_json(value: &impl Serialize) -> Result<ToolResult, ToolError> {
    Ok(ToolResult {
        success: true,
        output: serde_json::to_string(value)
            .map_err(|error| ToolError::Execution(error.to_string()))?,
        metadata: json!({"structured":true}),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::time::Instant;

    use crate::patch::{PatchError, PatchService};
    use crate::protocol::ExpectedFileHash;
    use crate::providers::{ProviderEvent, testing::FakeProvider};

    use super::*;

    struct NoopAgentPublisher;
    impl EventPublisher for NoopAgentPublisher {
        fn publish(&self, _event: AgentEventEnvelope) {}
    }

    #[derive(Default)]
    struct RecordingLifecycle {
        states: StdMutex<Vec<(String, SubagentState)>>,
    }

    impl SubagentEventPublisher for RecordingLifecycle {
        fn publish(&self, view: SubagentView) {
            self.states.lock().unwrap().push((view.id, view.state));
        }
    }

    fn request(parent: &str, task: &str) -> CreateSubagentRequest {
        CreateSubagentRequest {
            parent_thread_id: parent.into(),
            task: task.into(),
            label: None,
            capabilities: Vec::new(),
            token_budget: None,
            timeout_ms: DEFAULT_SUBAGENT_RUNTIME_MS,
        }
    }

    #[test]
    fn omitted_subagent_token_budget_defaults_to_unlimited() {
        let request: CreateSubagentRequest = serde_json::from_value(json!({
            "parentThreadId": "parent",
            "task": "inspect the workspace"
        }))
        .unwrap();

        assert_eq!(request.token_budget, None);
    }

    fn context(
        repository: Arc<JsonlThreadRepository>,
        workspace: &Path,
        provider: Arc<dyn Provider>,
        lifecycle: Arc<dyn SubagentEventPublisher>,
    ) -> SubagentExecutionContext {
        SubagentExecutionContext {
            repository,
            provider,
            model: "fixture".into(),
            context_limit: crate::context::DEFAULT_CONTEXT_LIMIT,
            tools: ToolRegistry::read_only(),
            workspace_root: workspace.to_path_buf(),
            approvals: Arc::new(ApprovalManager::new(Duration::from_secs(1))),
            approval_mode: ApprovalMode::Ask,
            reasoning_effort: ReasoningEffort::default(),
            agent_events: Arc::new(NoopAgentPublisher),
            lifecycle_events: lifecycle,
        }
    }

    #[tokio::test]
    async fn runs_independent_subagents_concurrently_and_returns_bounded_summaries() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(data.path()).unwrap());
        let provider = Arc::new(
            FakeProvider::text(&["parallel result"]).with_delay(Duration::from_millis(100)),
        );
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let manager = MultiAgentCoordinator::new(data.path()).unwrap();
        let parent = CancellationToken::new();
        let started = Instant::now();

        let first = manager
            .create(
                request("parent", "inspect backend"),
                None,
                context(
                    repository.clone(),
                    workspace.path(),
                    provider.clone(),
                    lifecycle.clone(),
                ),
                parent.child_token(),
            )
            .await
            .unwrap();
        let second = manager
            .create(
                request("parent", "inspect frontend"),
                None,
                context(repository, workspace.path(), provider, lifecycle),
                parent.child_token(),
            )
            .await
            .unwrap();
        let (first, second) = tokio::join!(
            manager.wait(&first.id, 2_000, CancellationToken::new()),
            manager.wait(&second.id, 2_000, CancellationToken::new())
        );

        assert_eq!(first.unwrap().state, SubagentState::Completed);
        assert_eq!(second.unwrap().summary.as_deref(), Some("parallel result"));
        assert!(started.elapsed() < Duration::from_millis(350));
    }

    #[tokio::test]
    async fn parent_cancellation_stops_the_child_provider_request() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(data.path()).unwrap());
        let provider = Arc::new(FakeProvider::text(&["late"]).with_delay(Duration::from_secs(10)));
        let manager = MultiAgentCoordinator::new(data.path()).unwrap();
        let parent = CancellationToken::new();
        let agent = manager
            .create(
                request("parent", "slow task"),
                None,
                context(
                    repository,
                    workspace.path(),
                    provider,
                    Arc::new(NoopSubagentPublisher),
                ),
                parent.child_token(),
            )
            .await
            .unwrap();

        parent.cancel();
        let stopped = manager
            .wait(&agent.id, 2_000, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(stopped.state, SubagentState::Cancelled);
    }

    #[tokio::test]
    async fn timeout_and_capability_limits_fail_closed() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(data.path()).unwrap());
        let provider = Arc::new(FakeProvider::text(&["late"]).with_delay(Duration::from_secs(10)));
        let manager = MultiAgentCoordinator::new(data.path()).unwrap();
        let runtime_context = context(
            repository,
            workspace.path(),
            provider,
            Arc::new(NoopSubagentPublisher),
        );
        let mut denied = request("parent", "write without capability");
        denied.capabilities = vec!["apply_patch".into()];
        assert!(matches!(
            manager
                .create(
                    denied,
                    None,
                    runtime_context.clone(),
                    CancellationToken::new()
                )
                .await,
            Err(MultiAgentError::Tool(ToolError::InvalidArguments(_)))
        ));

        let mut timed = request("parent", "bounded task");
        timed.timeout_ms = 20;
        let agent = manager
            .create(timed, None, runtime_context, CancellationToken::new())
            .await
            .unwrap();
        let result = manager
            .wait(&agent.id, 2_000, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.state, SubagentState::TimedOut);
    }

    #[tokio::test]
    async fn active_count_and_delegation_depth_are_bounded() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(data.path()).unwrap());
        let provider = Arc::new(FakeProvider::text(&["late"]).with_delay(Duration::from_secs(10)));
        let lifecycle: Arc<dyn SubagentEventPublisher> = Arc::new(NoopSubagentPublisher);
        let manager = MultiAgentCoordinator::new(data.path()).unwrap();
        let parent_cancel = CancellationToken::new();
        let runtime_context = context(repository, workspace.path(), provider, lifecycle);
        let mut active = Vec::new();
        for index in 0..MAX_ACTIVE_SUBAGENTS {
            active.push(
                manager
                    .create(
                        request("parent", &format!("task {index}")),
                        None,
                        runtime_context.clone(),
                        parent_cancel.child_token(),
                    )
                    .await
                    .unwrap(),
            );
        }
        assert!(matches!(
            manager
                .create(
                    request("parent", "one too many"),
                    None,
                    runtime_context.clone(),
                    parent_cancel.child_token(),
                )
                .await,
            Err(MultiAgentError::Limit(_))
        ));
        assert!(matches!(
            manager
                .create(
                    request("parent", "nested"),
                    Some(active[0].id.clone()),
                    runtime_context,
                    parent_cancel.child_token(),
                )
                .await,
            Err(MultiAgentError::Limit(_))
        ));
        parent_cancel.cancel();
        for agent in active {
            let stopped = manager
                .wait(&agent.id, 2_000, CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(stopped.state, SubagentState::Cancelled);
        }
    }

    #[tokio::test]
    async fn lifecycle_is_restored_without_exposing_hidden_messages() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(data.path()).unwrap());
        let manager = MultiAgentCoordinator::new(data.path()).unwrap();
        let agent = manager
            .create(
                request("parent", "persist me"),
                None,
                context(
                    repository,
                    workspace.path(),
                    Arc::new(FakeProvider::text(&["structured summary"])),
                    Arc::new(NoopSubagentPublisher),
                ),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let completed = manager
            .wait(&agent.id, 2_000, CancellationToken::new())
            .await
            .unwrap();
        drop(manager);

        let recovered = MultiAgentCoordinator::new(data.path())
            .unwrap()
            .get(&agent.id)
            .unwrap();
        assert_eq!(recovered, completed);
        let serialized = serde_json::to_value(recovered).unwrap();
        assert!(serialized.get("messages").is_none());
        assert_eq!(serialized["summary"], "structured summary");
    }

    #[tokio::test]
    async fn failed_followup_does_not_reuse_a_previous_turn_summary() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(data.path()).unwrap());
        let provider = Arc::new(FakeProvider::script(vec![
            vec![
                Ok(ProviderEvent::TextDelta {
                    delta: "first result".into(),
                }),
                Ok(ProviderEvent::Completed),
            ],
            vec![Err(crate::providers::ProviderError::Request(
                "follow-up failed".into(),
            ))],
        ]));
        let manager = MultiAgentCoordinator::new(data.path()).unwrap();
        let runtime_context = context(
            repository,
            workspace.path(),
            provider,
            Arc::new(NoopSubagentPublisher),
        );
        let agent = manager
            .create(
                request("parent", "first turn"),
                None,
                runtime_context.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let completed = manager
            .wait(&agent.id, 2_000, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(completed.summary.as_deref(), Some("first result"));

        manager
            .send_message(&agent.id, "follow up".into(), runtime_context)
            .await
            .unwrap();
        let failed = manager
            .wait(&agent.id, 2_000, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(failed.state, SubagentState::Failed);
        assert!(failed.summary.is_none());
    }

    #[tokio::test]
    async fn concurrent_writes_to_one_path_report_a_hash_conflict() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("shared.txt"), "before\n").unwrap();
        let service = PatchService::new();
        let first_patch =
            "*** Begin Patch\n*** Update File: shared.txt\n@@\n-before\n+first\n*** End Patch\n";
        let second_patch =
            "*** Begin Patch\n*** Update File: shared.txt\n@@\n-before\n+second\n*** End Patch\n";
        let preview = service
            .preview_patch(workspace.path(), first_patch)
            .unwrap();
        let expected = vec![ExpectedFileHash {
            path: "shared.txt".into(),
            before_hash: preview.files[0].before_hash.clone(),
        }];
        let first = service.apply_patch(
            workspace.path().to_path_buf(),
            "a".into(),
            "turn-a".into(),
            "call-a".into(),
            first_patch.into(),
            vec!["shared.txt".into()],
            expected.clone(),
        );
        let second = service.apply_patch(
            workspace.path().to_path_buf(),
            "b".into(),
            "turn-b".into(),
            "call-b".into(),
            second_patch.into(),
            vec!["shared.txt".into()],
            expected,
        );
        let (first, second) = tokio::join!(first, second);
        assert!(first.is_ok());
        assert!(matches!(second, Err(PatchError::Conflict(_))));
    }

    #[tokio::test]
    async fn token_budget_stops_a_subagent_turn() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(data.path()).unwrap());
        let provider = Arc::new(FakeProvider::new(vec![
            Ok(ProviderEvent::Usage {
                usage: TokenUsage {
                    input_tokens: 6,
                    output_tokens: 6,
                    total_tokens: 12,
                },
            }),
            Ok(ProviderEvent::Completed),
        ]));
        let manager = MultiAgentCoordinator::new(data.path()).unwrap();
        let mut bounded = request("parent", "stay within budget");
        bounded.token_budget = Some(10);
        let agent = manager
            .create(
                bounded,
                None,
                context(
                    repository,
                    workspace.path(),
                    provider,
                    Arc::new(NoopSubagentPublisher),
                ),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let result = manager
            .wait(&agent.id, 2_000, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.state, SubagentState::Failed);
        assert!(result.error.unwrap().contains("token_budget_exceeded"));
    }
}
