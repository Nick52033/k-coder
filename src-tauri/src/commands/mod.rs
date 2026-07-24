use std::sync::Arc;

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::advanced::{
    BrowserArtifact, BrowserAuditEvent, BrowserSettings, CreateGoalRequest, DocumentContent,
    EvaluationReport, GoalTransitionRequest, GoalView, MemorySettings, MemoryUpsertRequest,
    MemoryView, MetricsSnapshot, PlanUpdateRequest, PlanView, RepositorySearchIndex, SearchResult,
    extract_document, run_recorded_evaluation,
};
use crate::agent::{AgentRuntime, EventPublisher, RunTurnRequest, TurnOutcome};
use crate::app_state::AppState;
use crate::context::CompactionSummary;
use crate::execution::{
    CommandSessionView, OutputPage, PtyOutputPage, PtySessionView, StartCommandRequest,
    StartPtyRequest,
};
use crate::extensions::ExtensionOverview;
use crate::multi_agent::{
    CreateSubagentRequest, MultiAgentError, SubagentEventPublisher, SubagentExecutionContext,
    SubagentView, delegation_tools,
};
use crate::persistence::{ProjectRecord, UsageSummary};
use crate::protocol::{
    AgentEvent, AgentEventEnvelope, ApprovalResolution, ChangeSet, ImageAttachment, MessageRole,
    PROTOCOL_VERSION, PatchPreview, RuntimeStatus, TokenUsage,
};
use crate::providers::{
    ProviderConfigView, ProviderEvent, ProviderMessage, ProviderRequest, SaveProviderConfigRequest,
};
use crate::storage::{StoredEventKind, ThreadDetail, ThreadSummary};
use crate::workbench::{
    self, AttachmentContent, FileEntry, FilePreview, GitBranchView, GitStatusView, WorkspaceState,
};

const AGENT_EVENT_NAME: &str = "agent-event";
const SUBAGENT_EVENT_NAME: &str = "subagent-event";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    code: &'static str,
    message: String,
}

impl CommandError {
    fn new(code: &'static str, error: impl std::fmt::Display) -> Self {
        Self {
            code,
            message: error.to_string(),
        }
    }
}

type CommandResult<T> = Result<T, CommandError>;

struct TauriEventPublisher {
    app: AppHandle,
}

impl EventPublisher for TauriEventPublisher {
    fn publish(&self, event: AgentEventEnvelope) {
        let _ = self.app.emit(AGENT_EVENT_NAME, event);
    }
}

struct TauriSubagentEventPublisher {
    app: AppHandle,
}

impl SubagentEventPublisher for TauriSubagentEventPublisher {
    fn publish(&self, view: SubagentView) {
        let _ = self.app.emit(SUBAGENT_EVENT_NAME, view);
    }
}

fn subagent_context(
    app: &AppHandle,
    state: &AppState,
    provider: Arc<dyn crate::providers::Provider>,
    model: String,
    context_limit: usize,
    tools: crate::tools::ToolRegistry,
) -> SubagentExecutionContext {
    SubagentExecutionContext {
        repository: state.repository(),
        provider,
        model,
        context_limit,
        tools,
        workspace_root: state.workspace_root(),
        approvals: state.approvals(),
        agent_events: Arc::new(TauriEventPublisher { app: app.clone() }),
        lifecycle_events: Arc::new(TauriSubagentEventPublisher { app: app.clone() }),
    }
}

fn append_runtime_instructions(base: String, memory: String) -> String {
    match (base.trim().is_empty(), memory.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => base,
        (true, false) => memory,
        (false, false) => format!("{base}\n\n{memory}"),
    }
}

async fn turn_tokens(state: &AppState, thread_id: &str, turn_id: &str) -> u64 {
    state
        .runtime_repository()
        .load(thread_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|event| event.turn_id.as_deref() == Some(turn_id))
        .filter_map(|event| match event.kind {
            StoredEventKind::ProviderCallUsage { usage, .. } => Some(usage.total_tokens),
            _ => None,
        })
        .sum()
}

#[tauri::command]
pub fn runtime_status(state: State<'_, AppState>) -> RuntimeStatus {
    RuntimeStatus {
        ready: true,
        phase: "advanced-agent".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: state.uptime_seconds(),
        capabilities: vec![
            "streaming-chat".to_string(),
            "persistent-threads".to_string(),
            "cancellation".to_string(),
            "native-tool-calling".to_string(),
            "workspace-read-tools".to_string(),
            "workspace-write-tools".to_string(),
            "reviewable-patches".to_string(),
            "change-undo".to_string(),
            "command-sessions".to_string(),
            "bounded-command-output".to_string(),
            "process-tree-cancellation".to_string(),
            "command-risk-policy".to_string(),
            "pty-terminal".to_string(),
            "sqlite-projections".to_string(),
            "context-budgeting".to_string(),
            "context-compaction".to_string(),
            "crash-recovery".to_string(),
            "structured-logging".to_string(),
            "programming-workbench".to_string(),
            "runtime-instructions".to_string(),
            "skills".to_string(),
            "mcp-stdio".to_string(),
            "mcp-streamable-http".to_string(),
            "tool-hooks".to_string(),
            "extension-diagnostics".to_string(),
            "extension-audit".to_string(),
            "multi-agent-delegation".to_string(),
            "bounded-subagents".to_string(),
            "subagent-cancellation".to_string(),
            "subagent-persistence".to_string(),
            "persistent-plans".to_string(),
            "budgeted-goals".to_string(),
            "browser-automation".to_string(),
            "repository-search".to_string(),
            "opt-in-memory".to_string(),
            "bounded-document-extraction".to_string(),
            "runtime-metrics".to_string(),
        ],
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_plan(state: State<'_, AppState>, thread_id: String) -> CommandResult<Option<PlanView>> {
    state
        .advanced()
        .plans
        .get(&thread_id)
        .map_err(|error| CommandError::new("plan", error))
}

#[tauri::command]
pub fn update_plan(
    state: State<'_, AppState>,
    request: PlanUpdateRequest,
) -> CommandResult<PlanView> {
    state
        .advanced()
        .plans
        .update(request)
        .map_err(|error| CommandError::new("plan", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_goal(state: State<'_, AppState>, thread_id: String) -> CommandResult<Option<GoalView>> {
    state
        .advanced()
        .goals
        .current(&thread_id)
        .map_err(|error| CommandError::new("goal", error))
}

#[tauri::command]
pub fn create_goal(
    state: State<'_, AppState>,
    request: CreateGoalRequest,
) -> CommandResult<GoalView> {
    state
        .advanced()
        .goals
        .create(request)
        .map_err(|error| CommandError::new("goal", error))
}

#[tauri::command]
pub fn transition_goal(
    state: State<'_, AppState>,
    request: GoalTransitionRequest,
) -> CommandResult<GoalView> {
    state
        .advanced()
        .goals
        .transition(request)
        .map_err(|error| CommandError::new("goal", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn search_repository(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> CommandResult<Vec<SearchResult>> {
    RepositorySearchIndex::new(state.workspace_root())
        .search(&query, limit.unwrap_or(50))
        .map_err(|error| CommandError::new("repository_search", error))
}

#[tauri::command]
pub fn get_memory_settings(state: State<'_, AppState>) -> CommandResult<MemorySettings> {
    state
        .advanced()
        .memory
        .settings()
        .map_err(|error| CommandError::new("memory", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_memory_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> CommandResult<MemorySettings> {
    state
        .advanced()
        .memory
        .set_enabled(enabled)
        .map_err(|error| CommandError::new("memory", error))
}

#[tauri::command]
pub fn list_memories(state: State<'_, AppState>) -> CommandResult<Vec<MemoryView>> {
    state
        .advanced()
        .memory
        .list()
        .map_err(|error| CommandError::new("memory", error))
}

#[tauri::command]
pub fn upsert_memory(
    state: State<'_, AppState>,
    request: MemoryUpsertRequest,
) -> CommandResult<MemoryView> {
    state
        .advanced()
        .memory
        .upsert(request)
        .map_err(|error| CommandError::new("memory", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_memory(state: State<'_, AppState>, memory_id: String) -> CommandResult<MemoryView> {
    state
        .advanced()
        .memory
        .delete(&memory_id)
        .map_err(|error| CommandError::new("memory", error))
}

#[tauri::command]
pub async fn get_browser_settings(state: State<'_, AppState>) -> CommandResult<BrowserSettings> {
    Ok(state.advanced().browser.settings().await)
}

#[tauri::command]
pub async fn save_browser_settings(
    state: State<'_, AppState>,
    settings: BrowserSettings,
) -> CommandResult<BrowserSettings> {
    state
        .advanced()
        .browser
        .save_settings(settings)
        .await
        .map_err(|error| CommandError::new("browser", error))
}

#[tauri::command]
pub fn list_browser_audit(state: State<'_, AppState>) -> CommandResult<Vec<BrowserAuditEvent>> {
    state
        .advanced()
        .browser
        .audit_events()
        .map_err(|error| CommandError::new("browser", error))
}

#[tauri::command]
pub fn list_browser_artifacts(state: State<'_, AppState>) -> CommandResult<Vec<BrowserArtifact>> {
    state
        .advanced()
        .browser
        .artifacts()
        .map_err(|error| CommandError::new("browser", error))
}

#[tauri::command]
pub async fn close_browser_session(state: State<'_, AppState>) -> CommandResult<()> {
    state
        .advanced()
        .browser
        .close()
        .await
        .map_err(|error| CommandError::new("browser", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn extract_document_content(
    state: State<'_, AppState>,
    relative_path: String,
) -> CommandResult<DocumentContent> {
    extract_document(&state.workspace_root(), &relative_path)
        .map_err(|error| CommandError::new("document", error))
}

#[tauri::command]
pub fn advanced_metrics(state: State<'_, AppState>) -> CommandResult<MetricsSnapshot> {
    state
        .advanced()
        .metrics
        .snapshot()
        .map_err(|error| CommandError::new("metrics", error))
}

#[tauri::command]
pub fn run_regression_evaluation() -> CommandResult<EvaluationReport> {
    run_recorded_evaluation().map_err(|error| CommandError::new("evaluation", error))
}

#[tauri::command]
pub fn get_provider_config(
    state: State<'_, AppState>,
) -> CommandResult<Option<ProviderConfigView>> {
    state
        .provider_config()
        .map_err(|error| CommandError::new("provider_config", error))
}

#[tauri::command]
pub fn save_provider_config(
    state: State<'_, AppState>,
    request: SaveProviderConfigRequest,
) -> CommandResult<ProviderConfigView> {
    state
        .save_provider_config(request)
        .map_err(|error| CommandError::new("provider_config", error))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConnectionTest {
    connected: bool,
    latency_ms: u64,
    usage: Option<TokenUsage>,
}

#[tauri::command]
pub async fn test_provider_connection(
    state: State<'_, AppState>,
) -> CommandResult<ProviderConnectionTest> {
    let (provider, model, _) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let started = std::time::Instant::now();
    let request = ProviderRequest {
        schema_version: PROTOCOL_VERSION,
        model,
        messages: vec![ProviderMessage::Text {
            role: MessageRole::User,
            text: "Reply with OK.".into(),
        }],
        tools: vec![],
    };
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        provider.stream(request, CancellationToken::new()),
    )
    .await
    .map_err(|_| CommandError::new("provider_timeout", "connection test timed out"))?
    .map_err(|error| CommandError::new("provider", error))?;
    let mut usage = None;
    while let Some(event) = tokio::time::timeout(std::time::Duration::from_secs(20), stream.next())
        .await
        .map_err(|_| CommandError::new("provider_timeout", "connection test stream timed out"))?
    {
        match event.map_err(|error| CommandError::new("provider", error))? {
            ProviderEvent::Usage { usage: value } => usage = Some(value),
            ProviderEvent::Completed => break,
            _ => {}
        }
    }
    Ok(ProviderConnectionTest {
        connected: true,
        latency_ms: started.elapsed().as_millis() as u64,
        usage,
    })
}

#[tauri::command]
pub fn delete_provider_api_key(state: State<'_, AppState>) -> CommandResult<()> {
    state
        .delete_provider_api_key()
        .map_err(|error| CommandError::new("credential_store", error))
}

#[tauri::command]
pub async fn create_thread(state: State<'_, AppState>) -> CommandResult<ThreadSummary> {
    state
        .repository()
        .create_thread()
        .await
        .map_err(|error| CommandError::new("storage", error))
}

#[tauri::command]
pub async fn list_threads(state: State<'_, AppState>) -> CommandResult<Vec<ThreadSummary>> {
    state
        .repository()
        .list_threads()
        .await
        .map_err(|error| CommandError::new("storage", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn search_threads(
    state: State<'_, AppState>,
    query: String,
) -> CommandResult<Vec<ThreadSummary>> {
    state
        .repository()
        .search_threads(&query)
        .await
        .map_err(|error| CommandError::new("storage", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn rename_thread(
    state: State<'_, AppState>,
    thread_id: String,
    title: String,
) -> CommandResult<ThreadSummary> {
    state
        .repository()
        .rename_thread(&thread_id, title)
        .await
        .map_err(|error| CommandError::new("storage", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_thread(state: State<'_, AppState>, thread_id: String) -> CommandResult<()> {
    if state.is_turn_active(&thread_id).await {
        return Err(CommandError::new(
            "turn_active",
            "stop the active turn before deleting",
        ));
    }
    state
        .repository()
        .delete_thread(&thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))
}

#[tauri::command]
pub fn usage_summary(state: State<'_, AppState>) -> CommandResult<UsageSummary> {
    state
        .repository()
        .projection()
        .usage_summary()
        .map_err(|error| CommandError::new("projection", error))
}

#[tauri::command]
pub async fn extension_overview(
    state: State<'_, AppState>,
    refresh: bool,
) -> CommandResult<ExtensionOverview> {
    let result = state.prepare_extensions(refresh).await;
    let mut overview = state.extension_overview();
    if let Err(error) = result {
        overview.error = Some(error.to_string());
    }
    Ok(overview)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_extension_enabled(
    state: State<'_, AppState>,
    kind: String,
    id: String,
    enabled: bool,
) -> CommandResult<ExtensionOverview> {
    state
        .set_extension_enabled(&kind, &id, enabled)
        .await
        .map_err(|error| CommandError::new("extensions", error))?;
    Ok(state.extension_overview())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn save_mcp_secret(
    state: State<'_, AppState>,
    server: String,
    name: String,
    value: String,
) -> CommandResult<ExtensionOverview> {
    state
        .save_mcp_secret(&server, &name, &value)
        .await
        .map_err(|error| CommandError::new("extensions", error))?;
    Ok(state.extension_overview())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_mcp_secret(
    state: State<'_, AppState>,
    server: String,
    name: String,
) -> CommandResult<ExtensionOverview> {
    state
        .delete_mcp_secret(&server, &name)
        .await
        .map_err(|error| CommandError::new("extensions", error))?;
    Ok(state.extension_overview())
}

#[tauri::command]
pub fn workspace_state(state: State<'_, AppState>) -> CommandResult<WorkspaceState> {
    workbench::workspace_state(&state.repository().projection(), &state.workspace_root())
        .map_err(|error| CommandError::new("workspace", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn switch_workspace(
    state: State<'_, AppState>,
    path: String,
    trusted: bool,
) -> CommandResult<ProjectRecord> {
    let project = workbench::register_project(
        &state.repository().projection(),
        std::path::Path::new(&path),
        trusted,
    )
    .map_err(|error| CommandError::new("workspace", error))?;
    if !project.trusted {
        return Err(CommandError::new(
            "workspace_trust_required",
            "confirm trust before opening this workspace",
        ));
    }
    state
        .switch_workspace(&project.path)
        .await
        .map_err(|error| CommandError::new("workspace", error))?;
    Ok(project)
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_workspace_directory(
    state: State<'_, AppState>,
    path: String,
) -> CommandResult<Vec<FileEntry>> {
    workbench::list_directory(&state.workspace_root(), &path)
        .map_err(|error| CommandError::new("file_tree", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn preview_workspace_file(
    state: State<'_, AppState>,
    path: String,
) -> CommandResult<FilePreview> {
    workbench::preview_file(&state.workspace_root(), &path)
        .map_err(|error| CommandError::new("file_preview", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn extract_attachment(
    state: State<'_, AppState>,
    path: String,
) -> CommandResult<AttachmentContent> {
    let extension = std::path::Path::new(&path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    ) {
        let document = extract_document(&state.workspace_root(), &path)
            .map_err(|error| CommandError::new("attachment", error))?;
        return Ok(AttachmentContent {
            path: document.path,
            name: document.name,
            kind: "document".into(),
            content: document.content,
            size: document.source_bytes,
            truncated: document.truncated,
        });
    }
    workbench::extract_attachment(&state.workspace_root(), &path)
        .map_err(|error| CommandError::new("attachment", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_workspace_file(state: State<'_, AppState>, path: String) -> CommandResult<()> {
    workbench::open_external(&state.workspace_root(), &path, false)
        .map_err(|error| CommandError::new("file_open", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn reveal_workspace_file(state: State<'_, AppState>, path: String) -> CommandResult<()> {
    workbench::open_external(&state.workspace_root(), &path, true)
        .map_err(|error| CommandError::new("file_reveal", error))
}

#[tauri::command]
pub fn git_status(state: State<'_, AppState>) -> CommandResult<GitStatusView> {
    workbench::git_status(&state.workspace_root()).map_err(|error| CommandError::new("git", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_diff(
    state: State<'_, AppState>,
    path: Option<String>,
    staged: bool,
) -> CommandResult<String> {
    workbench::git_diff(&state.workspace_root(), path.as_deref(), staged)
        .map_err(|error| CommandError::new("git", error))
}

#[tauri::command]
pub fn git_branches(state: State<'_, AppState>) -> CommandResult<GitBranchView> {
    workbench::git_branches(&state.workspace_root())
        .map_err(|error| CommandError::new("git", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_switch_branch(
    state: State<'_, AppState>,
    branch: String,
    create: bool,
    confirmed: bool,
) -> CommandResult<String> {
    workbench::git_switch_branch(&state.workspace_root(), &branch, create, confirmed)
        .map_err(|error| CommandError::new("git", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn git_action(
    state: State<'_, AppState>,
    action: String,
    paths: Vec<String>,
    message: Option<String>,
    confirmed: bool,
) -> CommandResult<String> {
    workbench::git_action(
        &state.workspace_root(),
        &action,
        &paths,
        message.as_deref(),
        confirmed,
    )
    .map_err(|error| CommandError::new("git", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn read_thread(
    state: State<'_, AppState>,
    thread_id: String,
) -> CommandResult<ThreadDetail> {
    state
        .read_thread(&thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn archive_thread(state: State<'_, AppState>, thread_id: String) -> CommandResult<()> {
    if state.is_turn_active(&thread_id).await {
        return Err(CommandError::new(
            "turn_active",
            "stop the active turn before archiving this thread",
        ));
    }
    state
        .repository()
        .archive_thread(&thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn compact_thread(
    state: State<'_, AppState>,
    thread_id: String,
) -> CommandResult<CompactionSummary> {
    if state.is_turn_active(&thread_id).await {
        return Err(CommandError::new(
            "turn_active",
            "stop the active turn before compacting",
        ));
    }
    let context_limit = state
        .provider_context_limit()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        state.tool_registry(),
        state.workspace_root(),
        state.approvals(),
    )
    .with_context_limit(context_limit);
    runtime
        .compact_thread(&thread_id)
        .await
        .map_err(|error| CommandError::new("context_compaction", error))
}

#[tauri::command]
pub fn rebuild_session_projection(state: State<'_, AppState>) -> CommandResult<()> {
    state
        .repository()
        .rebuild_projection()
        .map_err(|error| CommandError::new("projection_rebuild", error))
}

#[tauri::command]
pub async fn run_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RunTurnRequest,
    attachments: Vec<ImageAttachment>,
) -> CommandResult<TurnOutcome> {
    let thread_id = request.thread_id.clone();
    state
        .prepare_extensions(false)
        .await
        .map_err(|error| CommandError::new("extensions", error))?;
    let advanced = state.advanced();
    let runtime_instructions = state
        .extension_instructions(&request.input)
        .map_err(|error| CommandError::new("extensions", error))?;
    let advanced_instructions = advanced
        .runtime_instructions(&thread_id)
        .map_err(|error| CommandError::new("advanced_runtime", error))?;
    let runtime_instructions =
        append_runtime_instructions(runtime_instructions, advanced_instructions);
    let memory_instructions = advanced
        .memory
        .context()
        .map_err(|error| CommandError::new("memory", error))?;
    let runtime_instructions =
        append_runtime_instructions(runtime_instructions, memory_instructions);
    let goal_budget = advanced
        .goals
        .turn_budget(&thread_id)
        .map_err(|error| CommandError::new("goal", error))?;
    let goal_timeout_ms = advanced
        .goals
        .current(&thread_id)
        .map_err(|error| CommandError::new("goal", error))?
        .filter(|goal| goal.state == crate::advanced::GoalState::Active)
        .map(|goal| goal.time_budget_ms.saturating_sub(goal.elapsed_ms));
    let _ = state.logger().log(
        "info",
        "turn_requested",
        serde_json::json!({"threadId": thread_id}),
    );
    let (provider, model, context_limit) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let cancellation = state
        .begin_turn(&thread_id)
        .await
        .map_err(|error| CommandError::new("turn_active", error))?;
    let goal_timeout = goal_timeout_ms.map(|timeout_ms| {
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)).await;
            cancellation.cancel();
        })
    });
    let base_tools = state.tool_registry();
    let child_context = subagent_context(
        &app,
        &state,
        provider.clone(),
        model.clone(),
        context_limit,
        base_tools.clone(),
    );
    let (agent_handlers, agent_risks) = delegation_tools(
        state.subagents(),
        child_context,
        thread_id.clone(),
        cancellation.child_token(),
    );
    let tools = base_tools
        .with_additional_handlers(agent_handlers, agent_risks)
        .map_err(|error| CommandError::new("multi_agent", error))?;
    let mut runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        tools,
        state.workspace_root(),
        state.approvals(),
    )
    .with_runtime_instructions(runtime_instructions)
    .with_context_limit(context_limit)
    .with_metrics(advanced.metrics.clone());
    if let Some((_, remaining_tokens)) = &goal_budget {
        runtime = runtime.with_token_budget(*remaining_tokens);
    }
    let publisher: Arc<dyn EventPublisher> = Arc::new(TauriEventPublisher { app });
    let started = std::time::Instant::now();
    let result = runtime
        .run_turn_with_attachments(
            provider,
            model,
            request,
            attachments,
            cancellation,
            publisher,
        )
        .await;
    if let Some(timeout) = goal_timeout {
        timeout.abort();
    }
    if let Some((goal_id, _)) = goal_budget {
        let tokens = match &result {
            Ok(outcome) => turn_tokens(&state, &thread_id, &outcome.turn_id).await,
            Err(_) => 0,
        };
        let _ = advanced
            .goals
            .record_turn(&goal_id, tokens, started.elapsed().as_millis() as u64);
    }
    state.finish_turn(&thread_id).await;
    let _ = state.logger().log(
        if result.is_ok() { "info" } else { "error" },
        "turn_finished",
        serde_json::json!({"threadId": thread_id, "success": result.is_ok()}),
    );
    result.map_err(|error| CommandError::new("agent_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn retry_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    thread_id: String,
) -> CommandResult<TurnOutcome> {
    state
        .prepare_extensions(false)
        .await
        .map_err(|error| CommandError::new("extensions", error))?;
    let retry_input = state
        .repository()
        .read_thread(&thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))?
        .messages
        .into_iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .map(|message| message.text())
        .unwrap_or_default();
    let advanced = state.advanced();
    let runtime_instructions = state
        .extension_instructions(&retry_input)
        .map_err(|error| CommandError::new("extensions", error))?;
    let advanced_instructions = advanced
        .runtime_instructions(&thread_id)
        .map_err(|error| CommandError::new("advanced_runtime", error))?;
    let runtime_instructions =
        append_runtime_instructions(runtime_instructions, advanced_instructions);
    let memory_instructions = advanced
        .memory
        .context()
        .map_err(|error| CommandError::new("memory", error))?;
    let runtime_instructions =
        append_runtime_instructions(runtime_instructions, memory_instructions);
    let goal_budget = advanced
        .goals
        .turn_budget(&thread_id)
        .map_err(|error| CommandError::new("goal", error))?;
    let goal_timeout_ms = advanced
        .goals
        .current(&thread_id)
        .map_err(|error| CommandError::new("goal", error))?
        .filter(|goal| goal.state == crate::advanced::GoalState::Active)
        .map(|goal| goal.time_budget_ms.saturating_sub(goal.elapsed_ms));
    let (provider, model, context_limit) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let cancellation = state
        .begin_turn(&thread_id)
        .await
        .map_err(|error| CommandError::new("turn_active", error))?;
    let goal_timeout = goal_timeout_ms.map(|timeout_ms| {
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)).await;
            cancellation.cancel();
        })
    });
    let mut runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        state.tool_registry(),
        state.workspace_root(),
        state.approvals(),
    )
    .with_runtime_instructions(runtime_instructions)
    .with_context_limit(context_limit)
    .with_metrics(advanced.metrics.clone());
    if let Some((_, remaining_tokens)) = &goal_budget {
        runtime = runtime.with_token_budget(*remaining_tokens);
    }
    let publisher: Arc<dyn EventPublisher> = Arc::new(TauriEventPublisher { app });
    let started = std::time::Instant::now();
    let result = runtime
        .retry_turn(provider, model, thread_id.clone(), cancellation, publisher)
        .await;
    if let Some(timeout) = goal_timeout {
        timeout.abort();
    }
    if let Some((goal_id, _)) = goal_budget {
        let tokens = match &result {
            Ok(outcome) => turn_tokens(&state, &thread_id, &outcome.turn_id).await,
            Err(_) => 0,
        };
        let _ = advanced
            .goals
            .record_turn(&goal_id, tokens, started.elapsed().as_millis() as u64);
    }
    state.finish_turn(&thread_id).await;
    result.map_err(|error| CommandError::new("agent_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn cancel_turn(state: State<'_, AppState>, thread_id: String) -> CommandResult<bool> {
    Ok(state.cancel_turn(&thread_id).await)
}

#[tauri::command]
pub async fn create_subagent(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CreateSubagentRequest,
) -> CommandResult<SubagentView> {
    state
        .repository()
        .read_thread(&request.parent_thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))?;
    state
        .prepare_extensions(false)
        .await
        .map_err(|error| CommandError::new("extensions", error))?;
    let (provider, model, context_limit) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let context = subagent_context(
        &app,
        &state,
        provider,
        model,
        context_limit,
        state.tool_registry(),
    );
    state
        .subagents()
        .create(request, None, context, CancellationToken::new())
        .await
        .map_err(multi_agent_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_subagents(
    state: State<'_, AppState>,
    parent_thread_id: Option<String>,
) -> Vec<SubagentView> {
    state.subagents().list(parent_thread_id.as_deref())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn wait_subagent(
    state: State<'_, AppState>,
    agent_id: String,
    timeout_ms: u64,
) -> CommandResult<SubagentView> {
    state
        .subagents()
        .wait(&agent_id, timeout_ms, CancellationToken::new())
        .await
        .map_err(multi_agent_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn send_subagent_message(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: String,
    message: String,
) -> CommandResult<SubagentView> {
    let (provider, model, context_limit) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let context = subagent_context(
        &app,
        &state,
        provider,
        model,
        context_limit,
        state.tool_registry(),
    );
    state
        .subagents()
        .send_message(&agent_id, message, context)
        .await
        .map_err(multi_agent_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn resume_subagent(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: String,
    message: Option<String>,
) -> CommandResult<SubagentView> {
    let (provider, model, context_limit) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let context = subagent_context(
        &app,
        &state,
        provider,
        model,
        context_limit,
        state.tool_registry(),
    );
    state
        .subagents()
        .resume(&agent_id, message, context)
        .await
        .map_err(multi_agent_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn close_subagent(state: State<'_, AppState>, agent_id: String) -> CommandResult<SubagentView> {
    state
        .subagents()
        .close(&agent_id)
        .map_err(multi_agent_command_error)
}

fn multi_agent_command_error(error: MultiAgentError) -> CommandError {
    CommandError::new("multi_agent", error)
}

#[tauri::command]
pub fn preview_patch(state: State<'_, AppState>, patch: String) -> CommandResult<PatchPreview> {
    state
        .patch_service()
        .preview_patch(&state.workspace_root(), &patch)
        .map_err(|error| CommandError::new("patch_preview", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn resolve_approval(
    state: State<'_, AppState>,
    request_id: String,
    resolution: ApprovalResolution,
) -> CommandResult<()> {
    state
        .approvals()
        .resolve(&request_id, resolution)
        .await
        .map_err(|error| CommandError::new("approval", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn undo_change(
    app: AppHandle,
    state: State<'_, AppState>,
    thread_id: String,
    change_id: String,
) -> CommandResult<ChangeSet> {
    if state.is_turn_active(&thread_id).await {
        return Err(CommandError::new(
            "turn_active",
            "stop the active turn before undoing a change",
        ));
    }
    let change = state
        .undo_change(&thread_id, &change_id)
        .await
        .map_err(|error| CommandError::new("change_undo", error))?;
    let _ = app.emit(
        AGENT_EVENT_NAME,
        AgentEventEnvelope::new(AgentEvent::ChangeUndone {
            thread_id,
            turn_id: change.turn_id.clone(),
            change_id,
        }),
    );
    Ok(change)
}

#[tauri::command]
pub async fn start_command(
    state: State<'_, AppState>,
    request: StartCommandRequest,
) -> CommandResult<CommandSessionView> {
    let runtime = state.command_runtime();
    let assessment = runtime.assess(&request);
    if assessment.requires_approval {
        return Err(CommandError::new(
            "command_approval_required",
            assessment.reason,
        ));
    }
    runtime
        .start(request)
        .await
        .map_err(|error| CommandError::new("command_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn command_status(
    state: State<'_, AppState>,
    session_id: String,
) -> CommandResult<CommandSessionView> {
    state
        .command_runtime()
        .status(&session_id)
        .await
        .map_err(|error| CommandError::new("command_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn read_command_output(
    state: State<'_, AppState>,
    session_id: String,
    cursor: u64,
    limit: usize,
) -> CommandResult<OutputPage> {
    state
        .command_runtime()
        .read(&session_id, cursor, limit)
        .await
        .map_err(|error| CommandError::new("command_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn wait_command(
    state: State<'_, AppState>,
    session_id: String,
) -> CommandResult<CommandSessionView> {
    state
        .command_runtime()
        .wait(&session_id)
        .await
        .map_err(|error| CommandError::new("command_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn write_command_stdin(
    state: State<'_, AppState>,
    session_id: String,
    input: String,
) -> CommandResult<()> {
    state
        .command_runtime()
        .write_stdin(&session_id, &input)
        .await
        .map_err(|error| CommandError::new("command_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn cancel_command(state: State<'_, AppState>, session_id: String) -> CommandResult<bool> {
    state
        .command_runtime()
        .cancel(&session_id)
        .await
        .map_err(|error| CommandError::new("command_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn close_command(state: State<'_, AppState>, session_id: String) -> CommandResult<()> {
    state
        .command_runtime()
        .close(&session_id)
        .await
        .map_err(|error| CommandError::new("command_runtime", error))
}

#[tauri::command]
pub async fn start_pty(
    state: State<'_, AppState>,
    request: StartPtyRequest,
) -> CommandResult<PtySessionView> {
    state
        .pty_runtime()
        .start(request)
        .await
        .map_err(|error| CommandError::new("pty_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn pty_status(
    state: State<'_, AppState>,
    session_id: String,
) -> CommandResult<PtySessionView> {
    state
        .pty_runtime()
        .status(&session_id)
        .await
        .map_err(|error| CommandError::new("pty_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn read_pty_output(
    state: State<'_, AppState>,
    session_id: String,
    cursor: u64,
    limit: usize,
) -> CommandResult<PtyOutputPage> {
    state
        .pty_runtime()
        .read(&session_id, cursor, limit)
        .await
        .map_err(|error| CommandError::new("pty_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn write_pty(
    state: State<'_, AppState>,
    session_id: String,
    input: String,
) -> CommandResult<()> {
    state
        .pty_runtime()
        .write(&session_id, &input)
        .await
        .map_err(|error| CommandError::new("pty_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn resize_pty(
    state: State<'_, AppState>,
    session_id: String,
    rows: u16,
    cols: u16,
) -> CommandResult<()> {
    state
        .pty_runtime()
        .resize(&session_id, rows, cols)
        .await
        .map_err(|error| CommandError::new("pty_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn wait_pty(
    state: State<'_, AppState>,
    session_id: String,
) -> CommandResult<PtySessionView> {
    state
        .pty_runtime()
        .wait(&session_id)
        .await
        .map_err(|error| CommandError::new("pty_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn close_pty(state: State<'_, AppState>, session_id: String) -> CommandResult<()> {
    state
        .pty_runtime()
        .close(&session_id)
        .await
        .map_err(|error| CommandError::new("pty_runtime", error))
}
