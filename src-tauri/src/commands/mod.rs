use std::sync::{Arc, Mutex as StdMutex};

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::advanced::{
    BrowserArtifact, BrowserAuditEvent, BrowserSettings, CreateGoalRequest, DocumentContent,
    EvaluationReport, GoalTransitionRequest, GoalView, MemorySettings, MemoryUpsertRequest,
    MemoryView, MetricsSnapshot, PlanUpdateRequest, PlanView, RepositorySearchIndex, SearchResult,
    extract_document, run_recorded_evaluation,
};
use crate::agent::mailbox::{MailboxTurn, MailboxTurnKind, QueuedTurnSteerError};
use crate::agent::thread_operation::ThreadOperationGuard;
use crate::agent::{
    AgentRuntime, EventPublisher, RunTurnRequest, SoftTurnLimits, TurnOutcome, build_user_message,
};
use crate::app_state::{AppState, AppStateError};
use crate::execution::{
    CommandSessionView, OutputPage, PtyOutputPage, PtySessionView, StartCommandRequest,
    StartPtyRequest,
};
use crate::extensions::ExtensionOverview;
use crate::logging::{LogQuery, LogQueryResult};
use crate::multi_agent::{
    CreateSubagentRequest, MultiAgentError, SubagentEventPublisher, SubagentExecutionContext,
    SubagentView, delegation_tools,
};
use crate::ocr::{self, OcrResult};
use crate::persistence::ProjectRecord;
use crate::protocol::{
    AgentEvent, AgentEventEnvelope, AgentMode, ApprovalMode, ApprovalResolution, ChangeSet,
    ImageAttachment, MessageRole, PROTOCOL_VERSION, PatchPreview, QueuedTurnSteerRequest,
    ReasoningEffort, RuntimeStatus, ThreadForkRequest, ThreadHistorySnapshot, ThreadMailboxChanged,
    ThreadMailboxSnapshot, ThreadRollbackRequest, TokenUsage, TurnHandle, TurnState,
    TurnSteerRequest, TurnSteerResponse, UserInputResolution,
};
use crate::providers::{
    ProviderConfigView, ProviderEvent, ProviderMessage, ProviderRequest, SaveProviderConfigRequest,
};
use crate::storage::{StoredEvent, StoredEventKind, ThreadRepository, ThreadSummary};
use crate::workbench::{
    self, AttachmentContent, FileEntry, FilePreview, GitBranchView, GitStatusView,
    SaveWorkspaceFileRequest, WorkspaceState,
};

/// Plan 协作模式指令模板（借鉴 Codex 的 plan.md）。
const PLAN_MODE_INSTRUCTIONS: &str = include_str!("../../templates/plan_mode.md");

/// Ask 模式指令模板。
const ASK_MODE_INSTRUCTIONS: &str = include_str!("../../templates/ask_mode.md");

/// Craft（默认执行）模式指令模板（借鉴 Codex 的 default.md）。
const CRAFT_MODE_INSTRUCTIONS: &str = include_str!("../../templates/craft_mode.md");

const AGENT_EVENT_NAME: &str = "agent-event";
const SUBAGENT_EVENT_NAME: &str = "subagent-event";
const THREAD_MAILBOX_CHANGED_EVENT_NAME: &str = "thread-mailbox-changed";

fn ordinary_turn_soft_limits(has_active_goal: bool) -> Option<SoftTurnLimits> {
    (!has_active_goal).then(SoftTurnLimits::default)
}

pub(crate) mod threads;

async fn emit_mailbox_changed(app: &AppHandle, state: &AppState, thread_id: &str) {
    let revision = state.thread_mailbox().revision(thread_id).await;
    emit_mailbox_revision(app, thread_id, revision);
}

fn emit_mailbox_revision(app: &AppHandle, thread_id: &str, revision: u64) {
    let _ = app.emit(
        THREAD_MAILBOX_CHANGED_EVENT_NAME,
        ThreadMailboxChanged {
            schema_version: PROTOCOL_VERSION,
            thread_id: thread_id.to_string(),
            revision,
        },
    );
}

fn instructions_for_mode(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Plan => PLAN_MODE_INSTRUCTIONS,
        AgentMode::Ask => ASK_MODE_INSTRUCTIONS,
        AgentMode::Craft => CRAFT_MODE_INSTRUCTIONS,
    }
}

fn tools_for_mode(
    tools: crate::tools::ToolRegistry,
    mode: AgentMode,
) -> Result<crate::tools::ToolRegistry, String> {
    if !mode.is_read_only() {
        return Ok(tools);
    }
    let allowed = mode
        .allowed_tools()
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    tools
        .restricted_to(&allowed)
        .map_err(|error| error.to_string())
}

fn retry_mode(events: &[StoredEvent]) -> AgentMode {
    let retryable_turn_id = events
        .iter()
        .rev()
        .find(|event| {
            matches!(
                event.kind,
                StoredEventKind::TurnFailed { .. }
                    | StoredEventKind::TurnCancelled
                    | StoredEventKind::TurnCompleted { .. }
            )
        })
        .and_then(|event| match &event.kind {
            StoredEventKind::TurnFailed { .. } | StoredEventKind::TurnCancelled => {
                event.turn_id.as_deref()
            }
            _ => None,
        });
    let Some(turn_id) = retryable_turn_id else {
        return AgentMode::Craft;
    };
    events
        .iter()
        .rev()
        .find_map(|event| {
            (event.turn_id.as_deref() == Some(turn_id))
                .then_some(&event.kind)
                .and_then(|kind| match kind {
                    StoredEventKind::TurnModeSelected { mode } => Some(*mode),
                    _ => None,
                })
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
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

    fn internal(error: impl std::fmt::Display) -> Self {
        Self::new("internal_error", error)
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

struct TurnStartPublisher {
    delegate: Arc<dyn EventPublisher>,
    thread_id: String,
    turn_id: String,
    signal: StdMutex<Option<oneshot::Sender<Result<(), String>>>>,
}

impl TurnStartPublisher {
    fn new(
        delegate: Arc<dyn EventPublisher>,
        thread_id: String,
        turn_id: String,
        signal: oneshot::Sender<Result<(), String>>,
    ) -> Self {
        Self {
            delegate,
            thread_id,
            turn_id,
            signal: StdMutex::new(Some(signal)),
        }
    }

    fn report_error(&self, error: CommandError) {
        if let Some(signal) = self.signal.lock().unwrap().take() {
            let message = error.message;
            if signal.send(Err(message.clone())).is_err() {
                self.delegate
                    .publish(AgentEventEnvelope::new(AgentEvent::TurnRejected {
                        thread_id: self.thread_id.clone(),
                        turn_id: self.turn_id.clone(),
                        message,
                    }));
            }
        }
    }
}

impl EventPublisher for TurnStartPublisher {
    fn publish(&self, event: AgentEventEnvelope) {
        let started = matches!(&event.event, AgentEvent::TurnStarted { .. });
        self.delegate.publish(event);
        if started {
            if let Some(signal) = self.signal.lock().unwrap().take() {
                let _ = signal.send(Ok(()));
            }
        }
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
        approval_mode: state.approval_mode(),
        reasoning_effort: state.reasoning_effort(),
        agent_events: Arc::new(TauriEventPublisher { app: app.clone() }),
        lifecycle_events: Arc::new(TauriSubagentEventPublisher { app: app.clone() }),
        logger: Some(state.logger()),
    }
}

/// 分层 system prompt 构建器（借鉴 Codex 的分层 system message 架构）。
/// 把 prompt 按 `<identity>`/`<workspace>`/`<collaboration_mode>`/`<tools>`/`<memory>`/`<extension_prompts>` 分块，
/// 让模型能清晰区分不同层级的指令。
fn build_system_prompt(
    workspace_root: &std::path::Path,
    extension_instructions: &str,
    advanced_instructions: &str,
    memory_context: &str,
    mode_instructions: &str,
    tool_names: &[String],
) -> String {
    let mut sections = Vec::<String>::new();

    // 1. identity — 固定的身份指令
    sections.push("<identity>\n你是 k-Coder，一个专业的 AI 编码助手。你运行在用户的桌面环境中，可以读写文件、执行命令、搜索代码库。\n\n**重要**：请始终用中文回复用户。执行多步骤任务时，把简短、具体的进度说明自然穿插在工具调用之间：第一次调用工具前说明当前目标；完成一组探索、修改或验证工具后，在开始下一组工具前说明刚确认的事实和下一步。不要让长任务退化为连续多轮“模型调用 + 工具调用”而没有用户可见的阶段沟通，也不要为了凑频率逐条复述每个命令。进度说明只包含动作、已确认事实和下一步，不是隐藏推理过程；不要输出私有思维链或逐步内心推演。\n\n**思考摘要语言**：推理摘要（reasoning summary）和思考过程对用户可见，必须始终使用中文输出；即使内部推理使用其他语言，也要把摘要内容翻译成中文后再输出，与界面语言保持一致。\n</identity>".to_string());

    // 2. workspace — 工作区信息
    // 移除 Windows 扩展路径前缀 \\?\ 避免 JSON 转义问题
    let workspace_path = workspace_root
        .display()
        .to_string()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/");
    let project_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    sections.push(format!(
        "<workspace>\n工作区路径（仅用于识别，不是工具参数）：{workspace_path}\n项目名称：{project_name}\n工具路径规则：所有工作区路径参数必须是相对工作区根目录的路径。工作区根目录使用 `.`；例如使用 `docs/开发路线图.md` 或 `src-tauri/src`。不得把上面的绝对路径传给工具，也不得使用 `..` 或其他父目录遍历。\n</workspace>"
    ));

    // 3. collaboration mode — 当前协作模式指令
    if !mode_instructions.trim().is_empty() {
        sections.push(format!(
            "<collaboration_mode>\n{}\n</collaboration_mode>",
            mode_instructions.trim()
        ));
    }

    // 4. tools — 可用工具列表
    if !tool_names.is_empty() {
        let tools_list = tool_names
            .iter()
            .map(|name| format!("- {name}"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "<available_tools>\n{tools_list}\n</available_tools>"
        ));
    }

    // 5. memory — 相关记忆
    if !memory_context.trim().is_empty() {
        sections.push(format!("<memory>\n{}\n</memory>", memory_context.trim()));
    }

    // 6. advanced — advanced 模块的运行时指令（goals/plans/metrics 等）
    if !advanced_instructions.trim().is_empty() {
        sections.push(format!(
            "<runtime_context>\n{}\n</runtime_context>",
            advanced_instructions.trim()
        ));
    }

    // 7. extension — 扩展注入的指令
    if !extension_instructions.trim().is_empty() {
        sections.push(format!(
            "<extension_prompts>\n{}\n</extension_prompts>",
            extension_instructions.trim()
        ));
    }

    sections.join("\n\n")
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
            "plan-mode".to_string(),
            "user-input-tool".to_string(),
            "budgeted-goals".to_string(),
            "browser-automation".to_string(),
            "repository-search".to_string(),
            "opt-in-memory".to_string(),
            "bounded-document-extraction".to_string(),
            "runtime-metrics".to_string(),
        ],
    }
}

#[tauri::command]
pub fn read_logs(
    state: State<'_, AppState>,
    limit: Option<usize>,
    level: Option<String>,
    event: Option<String>,
    after_timestamp_ms: Option<u64>,
) -> Result<LogQueryResult, CommandError> {
    let query = LogQuery {
        limit,
        level,
        event,
        after_timestamp_ms,
    };
    state
        .logger()
        .read_logs(query)
        .map_err(|error| CommandError::internal(error.to_string()))
}

#[tauri::command]
pub fn get_approval_mode(state: State<'_, AppState>) -> ApprovalMode {
    state.approval_mode()
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_approval_mode(
    state: State<'_, AppState>,
    mode: ApprovalMode,
) -> CommandResult<ApprovalMode> {
    state
        .set_approval_mode(mode)
        .await
        .map_err(|error| CommandError::new("approval_mode", error))
}

#[tauri::command]
pub fn get_reasoning_effort(state: State<'_, AppState>) -> ReasoningEffort {
    state.reasoning_effort()
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_reasoning_effort(
    state: State<'_, AppState>,
    effort: ReasoningEffort,
) -> CommandResult<ReasoningEffort> {
    state
        .set_reasoning_effort(effort)
        .await
        .map_err(|error| CommandError::new("reasoning_effort", error))
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
pub fn get_provider_catalog(
    state: State<'_, AppState>,
) -> CommandResult<crate::providers::ProviderCatalogView> {
    state
        .provider_catalog()
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

#[tauri::command]
pub fn activate_provider(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<crate::providers::ProviderCatalogView> {
    state
        .activate_provider(&provider_id)
        .map_err(|error| CommandError::new("provider_config", error))
}

#[tauri::command]
pub fn delete_provider(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<crate::providers::ProviderCatalogView> {
    state
        .delete_provider(&provider_id)
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
    provider_id: Option<String>,
) -> CommandResult<ProviderConnectionTest> {
    let (provider, model, _) = state
        .build_provider_for(provider_id.as_deref())
        .map_err(|error| CommandError::new("provider_config", error))?;
    let started = std::time::Instant::now();
    let request = ProviderRequest {
        schema_version: PROTOCOL_VERSION,
        model,
        reasoning_effort: ReasoningEffort::Off,
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
pub fn delete_provider_api_key(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<()> {
    state
        .delete_provider_api_key(&provider_id)
        .map_err(|error| CommandError::new("credential_store", error))
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
pub fn search_workspace_files(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> CommandResult<Vec<FileEntry>> {
    workbench::search_files(&state.workspace_root(), &query, limit.unwrap_or(50))
        .map_err(|error| CommandError::new("file_search", error))
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
pub async fn save_workspace_file(
    state: State<'_, AppState>,
    request: SaveWorkspaceFileRequest,
) -> CommandResult<FilePreview> {
    let patch_service = state.patch_service();
    let _edit_guard = patch_service.acquire_edit_lock().await;
    workbench::save_file(&state.workspace_root(), request).map_err(|error| {
        let code = if matches!(error, workbench::WorkbenchError::Conflict(_)) {
            "file_conflict"
        } else {
            "file_save"
        };
        CommandError::new(code, error)
    })
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

#[tauri::command]
pub async fn run_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RunTurnRequest,
    attachments: Vec<ImageAttachment>,
) -> CommandResult<TurnOutcome> {
    let publisher: Arc<dyn EventPublisher> = Arc::new(TauriEventPublisher { app: app.clone() });
    execute_turn(
        app,
        state.inner(),
        request,
        attachments,
        None,
        None,
        publisher,
    )
    .await
}

#[tauri::command]
pub async fn turn_start(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RunTurnRequest,
    attachments: Vec<ImageAttachment>,
) -> CommandResult<TurnHandle> {
    let turn_id = Uuid::new_v4().to_string();
    let thread_id = request.thread_id.clone();
    let (signal, started) = oneshot::channel();
    let handle = TurnHandle {
        schema_version: PROTOCOL_VERSION,
        thread_id: thread_id.clone(),
        turn_id: turn_id.clone(),
        state: TurnState::Queued,
    };
    let should_start = state
        .enqueue_thread_turn(MailboxTurn {
            handle: handle.clone(),
            kind: MailboxTurnKind::Message {
                request,
                attachments,
            },
            started: Some(signal),
        })
        .await;
    emit_mailbox_changed(&app, state.inner(), &thread_id).await;

    if !should_start {
        return Ok(handle);
    }

    tauri::async_runtime::spawn(drain_thread_mailbox(app, thread_id));
    started
        .await
        .map_err(|_| CommandError::internal("turn start task ended before initialization"))?
        .map_err(|error| CommandError::new("turn_start", error))?;

    Ok(TurnHandle {
        state: TurnState::Streaming,
        ..handle
    })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn turn_retry(
    app: AppHandle,
    state: State<'_, AppState>,
    thread_id: String,
) -> CommandResult<TurnHandle> {
    let turn_id = Uuid::new_v4().to_string();
    let (signal, started) = oneshot::channel();
    let handle = TurnHandle {
        schema_version: PROTOCOL_VERSION,
        thread_id: thread_id.clone(),
        turn_id,
        state: TurnState::Queued,
    };
    let should_start = state
        .enqueue_thread_turn(MailboxTurn {
            handle: handle.clone(),
            kind: MailboxTurnKind::Retry,
            started: Some(signal),
        })
        .await;
    emit_mailbox_changed(&app, state.inner(), &thread_id).await;

    if !should_start {
        return Ok(handle);
    }

    tauri::async_runtime::spawn(drain_thread_mailbox(app, thread_id));
    started
        .await
        .map_err(|_| CommandError::internal("turn retry task ended before initialization"))?
        .map_err(|error| CommandError::new("turn_retry", error))?;

    Ok(TurnHandle {
        state: TurnState::Streaming,
        ..handle
    })
}

async fn drain_thread_mailbox(app: AppHandle, thread_id: String) {
    loop {
        let (item, revision) = {
            let state = app.state::<AppState>();
            let item = state.next_thread_turn(&thread_id).await;
            let revision = state.thread_mailbox().revision(&thread_id).await;
            (item, revision)
        };
        emit_mailbox_revision(&app, &thread_id, revision);
        let Some((item, operation_guard)) = item else {
            return;
        };
        let MailboxTurn {
            handle,
            kind,
            started,
        } = item;
        let delegate: Arc<dyn EventPublisher> = Arc::new(TauriEventPublisher { app: app.clone() });
        let publisher = Arc::new(match started {
            Some(signal) => TurnStartPublisher::new(
                delegate,
                handle.thread_id.clone(),
                handle.turn_id.clone(),
                signal,
            ),
            None => unreachable!("mailbox turns always carry a start signal"),
        });
        let state = app.state::<AppState>();
        let result = match kind {
            MailboxTurnKind::Message {
                request,
                attachments,
            } => {
                execute_turn(
                    app.clone(),
                    state.inner(),
                    request,
                    attachments,
                    Some(handle.turn_id),
                    Some(operation_guard),
                    publisher.clone(),
                )
                .await
            }
            MailboxTurnKind::Retry => {
                execute_retry(
                    state.inner(),
                    handle.thread_id,
                    Some(handle.turn_id),
                    Some(operation_guard),
                    publisher.clone(),
                )
                .await
            }
        };
        match result {
            Ok(_) => publisher.report_error(CommandError::internal(
                "turn completed before publishing turn_started",
            )),
            Err(error) => publisher.report_error(error),
        }
    }
}

#[tauri::command(rename_all = "camelCase")]
pub async fn read_thread_mailbox(
    state: State<'_, AppState>,
    thread_id: String,
) -> CommandResult<ThreadMailboxSnapshot> {
    state
        .repository()
        .read_thread(&thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))?;
    let active_turn_id = state.active_turn_id(&thread_id).await;
    Ok(state
        .thread_mailbox()
        .snapshot(&thread_id, active_turn_id)
        .await)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn remove_queued_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    thread_id: String,
    turn_id: String,
) -> CommandResult<bool> {
    let removed = state.remove_queued_turn(&thread_id, &turn_id).await;
    if removed {
        emit_mailbox_changed(&app, state.inner(), &thread_id).await;
    }
    Ok(removed)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn clear_thread_mailbox(
    app: AppHandle,
    state: State<'_, AppState>,
    thread_id: String,
) -> CommandResult<usize> {
    let removed = state.clear_thread_mailbox(&thread_id).await;
    if removed > 0 {
        emit_mailbox_changed(&app, state.inner(), &thread_id).await;
    }
    Ok(removed)
}

#[tauri::command]
pub async fn turn_steer(
    app: AppHandle,
    state: State<'_, AppState>,
    request: TurnSteerRequest,
) -> CommandResult<TurnSteerResponse> {
    if request.expected_turn_id.trim().is_empty() {
        return Err(CommandError::new(
            "invalid_request",
            "expectedTurnId must not be empty",
        ));
    }
    let active_turn_id = state
        .active_turn_id(&request.thread_id)
        .await
        .ok_or_else(|| CommandError::new("no_active_turn", "no active turn to steer"))?;
    if active_turn_id != request.expected_turn_id {
        return Err(CommandError::new(
            "turn_mismatch",
            format!(
                "expected active turn id {}, but found {}",
                request.expected_turn_id, active_turn_id
            ),
        ));
    }

    let message =
        prepare_steer_message(&app, state.inner(), &request.input, request.attachments).await?;
    let turn_id = state
        .steer_turn(&request.thread_id, &request.expected_turn_id, message)
        .await
        .map_err(|error| CommandError::new("turn_steer", error))?;
    Ok(TurnSteerResponse {
        schema_version: PROTOCOL_VERSION,
        thread_id: request.thread_id,
        turn_id,
    })
}

#[tauri::command]
pub async fn turn_steer_queued(
    app: AppHandle,
    state: State<'_, AppState>,
    request: QueuedTurnSteerRequest,
) -> CommandResult<TurnSteerResponse> {
    if request.expected_turn_id.trim().is_empty() || request.queued_turn_id.trim().is_empty() {
        return Err(CommandError::new(
            "invalid_request",
            "expectedTurnId and queuedTurnId must not be empty",
        ));
    }
    let active_turn_id = state
        .active_turn_id(&request.thread_id)
        .await
        .ok_or_else(|| CommandError::new("no_active_turn", "no active turn to steer"))?;
    if active_turn_id != request.expected_turn_id {
        return Err(CommandError::new(
            "turn_mismatch",
            format!(
                "expected active turn id {}, but found {}",
                request.expected_turn_id, active_turn_id
            ),
        ));
    }
    let pending = state
        .thread_mailbox()
        .pending_message(&request.thread_id, &request.queued_turn_id)
        .await
        .map_err(|error| match error {
            QueuedTurnSteerError::NotFound => CommandError::new(
                "queued_turn_not_found",
                format!("queued turn {} was not found", request.queued_turn_id),
            ),
            QueuedTurnSteerError::NotMessage => CommandError::new(
                "queued_turn_not_message",
                format!("queued turn {} is not a message", request.queued_turn_id),
            ),
            QueuedTurnSteerError::TurnClosed => {
                CommandError::new("no_active_turn", "active turn no longer accepts input")
            }
        })?;
    let message = prepare_steer_message(
        &app,
        state.inner(),
        &pending.request.input,
        pending.attachments,
    )
    .await?;
    let turn_id = state
        .steer_queued_message(
            &request.thread_id,
            &request.expected_turn_id,
            &request.queued_turn_id,
            message,
        )
        .await
        .map_err(map_queued_steer_error)?;
    emit_mailbox_changed(&app, state.inner(), &request.thread_id).await;
    Ok(TurnSteerResponse {
        schema_version: PROTOCOL_VERSION,
        thread_id: request.thread_id,
        turn_id,
    })
}

async fn prepare_steer_message(
    app: &AppHandle,
    state: &AppState,
    input: &str,
    mut attachments: Vec<ImageAttachment>,
) -> CommandResult<crate::protocol::ChatMessage> {
    let supports_vision = state
        .active_model_supports_vision()
        .map_err(|error| CommandError::new("provider_config", error))?;
    if supports_vision {
        for attachment in &mut attachments {
            attachment.ocr_text = None;
        }
    } else if !attachments.is_empty() {
        enrich_image_attachments(app, &mut attachments).await?;
    }
    build_user_message(input, attachments, supports_vision)
        .map_err(|error| CommandError::new("invalid_request", error))
}

fn map_queued_steer_error(error: AppStateError) -> CommandError {
    let code = match &error {
        AppStateError::NoActiveTurn(_) => "no_active_turn",
        AppStateError::ExpectedTurnMismatch { .. } => "turn_mismatch",
        AppStateError::QueuedTurnNotFound { .. } => "queued_turn_not_found",
        AppStateError::QueuedTurnNotMessage { .. } => "queued_turn_not_message",
        _ => "turn_steer_queued",
    };
    CommandError::new(code, error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn turn_interrupt(
    state: State<'_, AppState>,
    thread_id: String,
    turn_id: String,
) -> CommandResult<()> {
    if turn_id.trim().is_empty() {
        return Err(CommandError::new(
            "invalid_request",
            "turnId must not be empty",
        ));
    }
    state
        .interrupt_turn(&thread_id, &turn_id)
        .await
        .map_err(|error| CommandError::new("turn_interrupt", error))
}

#[tauri::command]
pub async fn thread_fork(
    state: State<'_, AppState>,
    request: ThreadForkRequest,
) -> CommandResult<ThreadSummary> {
    state
        .fork_thread(&request.thread_id, request.last_turn_id.as_deref())
        .await
        .map_err(map_thread_operation_error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn thread_resume(
    state: State<'_, AppState>,
    thread_id: String,
) -> CommandResult<ThreadHistorySnapshot> {
    state
        .resume_thread_history(&thread_id)
        .await
        .map_err(|error| CommandError::new("thread_resume", error))
}

#[tauri::command]
pub async fn thread_rollback(
    state: State<'_, AppState>,
    request: ThreadRollbackRequest,
) -> CommandResult<ThreadHistorySnapshot> {
    state
        .rollback_thread(&request.thread_id, request.num_turns)
        .await
        .map_err(map_thread_operation_error)
}

fn map_thread_operation_error(error: AppStateError) -> CommandError {
    let code = match &error {
        AppStateError::ThreadOperationBusy(_) => "turn_active",
        AppStateError::ThreadMailboxNotEmpty(_) => "mailbox_not_empty",
        _ => "thread_operation",
    };
    CommandError::new(code, error)
}

async fn execute_turn(
    app: AppHandle,
    state: &AppState,
    request: RunTurnRequest,
    attachments: Vec<ImageAttachment>,
    assigned_turn_id: Option<String>,
    operation_guard: Option<ThreadOperationGuard>,
    publisher: Arc<dyn EventPublisher>,
) -> CommandResult<TurnOutcome> {
    let mut attachments = attachments;
    let has_image_attachments = !attachments.is_empty();
    let thread_id = request.thread_id.clone();
    let workspace_root = state
        .ensure_thread_workspace(&thread_id)
        .await
        .map_err(|error| CommandError::new("workspace_mismatch", error))?;
    let history_has_images = state
        .repository()
        .read_thread(&thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))?
        .messages
        .iter()
        .any(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, crate::protocol::ContentBlock::Image { .. }))
        });
    state
        .prepare_extensions(false)
        .await
        .map_err(|error| CommandError::new("extensions", error))?;
    let advanced = state.advanced();
    let extension_instructions = state
        .extension_instructions(&request.input)
        .map_err(|error| CommandError::new("extensions", error))?;
    let advanced_instructions = advanced
        .runtime_instructions(&thread_id)
        .map_err(|error| CommandError::new("advanced_runtime", error))?;
    let memory_instructions = advanced
        .memory
        .context()
        .map_err(|error| CommandError::new("memory", error))?;

    // 根据协作模式注入指令并限制可用工具
    let agent_mode = request
        .agent_mode
        .as_deref()
        .map(AgentMode::from_str)
        .unwrap_or_default();
    let mode_instructions = instructions_for_mode(agent_mode).to_string();

    // Plan/Ask 模式下把工具限制为只读子集（借鉴 Codex 的 plan_mask）。
    let base_tools = tools_for_mode(state.tool_registry(), agent_mode)
        .map_err(|error| CommandError::new("agent_mode", error))?;

    // 分层拼接 system prompt（identity/workspace/mode/tools/memory/context/extension）
    let tool_names = base_tools.definition_names();
    let runtime_instructions = build_system_prompt(
        &workspace_root,
        &extension_instructions,
        &advanced_instructions,
        &memory_instructions,
        &mode_instructions,
        &tool_names,
    );

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
    let supports_vision = state
        .active_model_supports_vision()
        .map_err(|error| CommandError::new("provider_config", error))?;
    if supports_vision {
        for attachment in &mut attachments {
            attachment.ocr_text = None;
        }
    } else if has_image_attachments {
        enrich_image_attachments(&app, &mut attachments).await?;
    }
    let (provider, model, context_limit) =
        if supports_vision && (has_image_attachments || history_has_images) {
            state.build_vision_provider()
        } else {
            state.build_provider()
        }
        .map_err(|error| CommandError::new("provider_config", error))?;
    let turn_id = assigned_turn_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let begin_result = match operation_guard.as_ref() {
        Some(operation_guard) => {
            state
                .begin_turn_with_id_in_workspace_locked(
                    &thread_id,
                    &turn_id,
                    &workspace_root,
                    operation_guard,
                )
                .await
        }
        None => {
            state
                .begin_turn_with_id_in_workspace(&thread_id, &turn_id, &workspace_root)
                .await
        }
    };
    let (cancellation, control) =
        begin_result.map_err(|error| CommandError::new("turn_active", error))?;
    drop(operation_guard);
    let goal_timeout = goal_timeout_ms.map(|timeout_ms| {
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)).await;
            cancellation.cancel();
        })
    });
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
    let tools = match base_tools.with_additional_handlers(agent_handlers, agent_risks) {
        Ok(tools) => tools,
        Err(error) => {
            if let Some(timeout) = goal_timeout {
                timeout.abort();
            }
            state.finish_turn(&thread_id).await;
            return Err(CommandError::new("multi_agent", error));
        }
    };
    let mut runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        tools,
        workspace_root,
        state.approvals(),
    )
    .with_approval_mode(state.approval_mode())
    .with_runtime_instructions(runtime_instructions)
    .with_context_limit(context_limit)
    .with_metrics(advanced.metrics.clone())
    .with_reasoning_effort(state.reasoning_effort())
    .with_vision_support(supports_vision)
    .with_user_inputs(state.user_inputs())
    .with_logger(state.logger());
    if let Some(limits) = ordinary_turn_soft_limits(goal_budget.is_some()) {
        runtime = runtime.with_soft_turn_limits(limits);
    }
    if let Some((_, Some(remaining_tokens))) = &goal_budget {
        runtime = runtime.with_token_budget(*remaining_tokens);
    }
    let started = std::time::Instant::now();
    let result = runtime
        .run_turn_with_attachments_id_and_control(
            provider,
            model,
            request,
            attachments,
            turn_id,
            cancellation,
            control,
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
        serde_json::json!({
            "threadId": thread_id,
            "success": result.is_ok(),
            "error": result.as_ref().err().map(|e| e.to_string()),
        }),
    );
    result.map_err(|error| CommandError::new("agent_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn retry_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    thread_id: String,
) -> CommandResult<TurnOutcome> {
    let publisher: Arc<dyn EventPublisher> = Arc::new(TauriEventPublisher { app: app.clone() });
    execute_retry(state.inner(), thread_id, None, None, publisher).await
}

async fn execute_retry(
    state: &AppState,
    thread_id: String,
    assigned_turn_id: Option<String>,
    operation_guard: Option<ThreadOperationGuard>,
    publisher: Arc<dyn EventPublisher>,
) -> CommandResult<TurnOutcome> {
    let workspace_root = state
        .ensure_thread_workspace(&thread_id)
        .await
        .map_err(|error| CommandError::new("workspace_mismatch", error))?;
    state
        .prepare_extensions(false)
        .await
        .map_err(|error| CommandError::new("extensions", error))?;
    let repository = state.repository();
    let events = repository
        .load(&thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))?;
    let agent_mode = retry_mode(&events);
    let thread_detail = repository
        .read_thread(&thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))?;
    let history_has_images = thread_detail.messages.iter().any(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, crate::protocol::ContentBlock::Image { .. }))
    });
    let retry_message = thread_detail
        .messages
        .into_iter()
        .rev()
        .find(|message| message.role == MessageRole::User);
    let retry_input = retry_message
        .as_ref()
        .map(|message| message.text())
        .unwrap_or_default();
    let retry_has_images = retry_message.as_ref().is_some_and(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, crate::protocol::ContentBlock::Image { .. }))
    });
    let advanced = state.advanced();
    let extension_instructions = state
        .extension_instructions(&retry_input)
        .map_err(|error| CommandError::new("extensions", error))?;
    let advanced_instructions = advanced
        .runtime_instructions(&thread_id)
        .map_err(|error| CommandError::new("advanced_runtime", error))?;
    let memory_instructions = advanced
        .memory
        .context()
        .map_err(|error| CommandError::new("memory", error))?;
    let mode_instructions = instructions_for_mode(agent_mode).to_string();
    let base_tools = tools_for_mode(state.tool_registry(), agent_mode)
        .map_err(|error| CommandError::new("agent_mode", error))?;
    let tool_names = base_tools.definition_names();
    let runtime_instructions = build_system_prompt(
        &workspace_root,
        &extension_instructions,
        &advanced_instructions,
        &memory_instructions,
        &mode_instructions,
        &tool_names,
    );
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
    let supports_vision = state
        .active_model_supports_vision()
        .map_err(|error| CommandError::new("provider_config", error))?;
    if retry_has_images
        && !supports_vision
        && retry_message.as_ref().is_some_and(|message| {
            !message.content.iter().any(|block| {
                matches!(
                    block,
                    crate::protocol::ContentBlock::Context { text }
                        if text.contains("[图片文字识别:")
                )
            })
        })
    {
        return Err(CommandError::new(
            "ocr",
            "当前模型不支持图片，且原消息没有可用的本地 OCR 结果",
        ));
    }
    let (provider, model, context_limit) = if supports_vision && history_has_images {
        state.build_vision_provider()
    } else {
        state.build_provider()
    }
    .map_err(|error| CommandError::new("provider_config", error))?;
    let turn_id = assigned_turn_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let begin_result = match operation_guard.as_ref() {
        Some(operation_guard) => {
            state
                .begin_turn_with_id_in_workspace_locked(
                    &thread_id,
                    &turn_id,
                    &workspace_root,
                    operation_guard,
                )
                .await
        }
        None => {
            state
                .begin_turn_with_id_in_workspace(&thread_id, &turn_id, &workspace_root)
                .await
        }
    };
    let (cancellation, control) =
        begin_result.map_err(|error| CommandError::new("turn_active", error))?;
    drop(operation_guard);
    let goal_timeout = goal_timeout_ms.map(|timeout_ms| {
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)).await;
            cancellation.cancel();
        })
    });
    let mut runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        base_tools,
        workspace_root,
        state.approvals(),
    )
    .with_approval_mode(state.approval_mode())
    .with_runtime_instructions(runtime_instructions)
    .with_context_limit(context_limit)
    .with_metrics(advanced.metrics.clone())
    .with_reasoning_effort(state.reasoning_effort())
    .with_vision_support(supports_vision)
    .with_user_inputs(state.user_inputs())
    .with_logger(state.logger());
    if let Some(limits) = ordinary_turn_soft_limits(goal_budget.is_some()) {
        runtime = runtime.with_soft_turn_limits(limits);
    }
    if let Some((_, Some(remaining_tokens))) = &goal_budget {
        runtime = runtime.with_token_budget(*remaining_tokens);
    }
    let started = std::time::Instant::now();
    let result = runtime
        .retry_turn_with_id_and_control(
            provider,
            model,
            thread_id.clone(),
            agent_mode,
            turn_id,
            cancellation,
            control,
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
        serde_json::json!({
            "threadId": thread_id,
            "success": result.is_ok(),
            "error": result.as_ref().err().map(|e| e.to_string()),
        }),
    );
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
pub async fn resolve_user_input(
    state: State<'_, AppState>,
    request_id: String,
    resolution: UserInputResolution,
) -> CommandResult<()> {
    state
        .user_inputs()
        .resolve(&request_id, resolution)
        .await
        .map_err(|error| CommandError::new("user_input", error))
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

/// 在本地使用随应用打包的 PP-OCRv5 模型识别图片文字。
#[tauri::command(rename_all = "camelCase")]
pub async fn recognize_image(app: AppHandle, data_url: String) -> CommandResult<OcrResult> {
    let resource_dir = ocr_resource_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || ocr::recognize_data_url(&data_url, &resource_dir))
        .await
        .map_err(|error| CommandError::new("ocr", error.to_string()))?
        .map_err(|error| CommandError::new("ocr", error))
}

fn ocr_resource_dir(app: &AppHandle) -> CommandResult<std::path::PathBuf> {
    let bundled_dir = app
        .path()
        .resource_dir()
        .map_err(|error| CommandError::new("ocr_resources", error))?
        .join("ocr");
    let development_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/resources/ocr");
    Ok(if bundled_dir.join("onnxruntime.dll").is_file() {
        bundled_dir
    } else {
        development_dir
    })
}

async fn enrich_image_attachments(
    app: &AppHandle,
    attachments: &mut [ImageAttachment],
) -> CommandResult<()> {
    let resource_dir = ocr_resource_dir(app)?;
    for attachment in attachments.iter_mut() {
        attachment.ocr_text = None;
        let data_url = attachment.data_url.clone();
        let resource_dir = resource_dir.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            ocr::recognize_data_url(&data_url, &resource_dir)
        })
        .await
        .map_err(|error| CommandError::new("ocr", error.to_string()))?
        .map_err(|error| CommandError::new("ocr", error))?;
        if result.text.trim().is_empty() {
            return Err(CommandError::new(
                "ocr",
                format!(
                    "当前模型不支持图片，本地 OCR 未能从 {} 识别出文字",
                    attachment.name
                ),
            ));
        }
        attachment.ocr_text = Some(result.text);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use super::{
        CRAFT_MODE_INSTRUCTIONS, CommandError, TurnStartPublisher, build_system_prompt,
        ordinary_turn_soft_limits, retry_mode,
    };
    use crate::agent::EventPublisher;
    use crate::protocol::{AgentEvent, AgentEventEnvelope, AgentMode};
    use crate::storage::{StoredEvent, StoredEventKind};

    #[derive(Default)]
    struct RecordingPublisher {
        events: Mutex<Vec<AgentEventEnvelope>>,
    }

    impl EventPublisher for RecordingPublisher {
        fn publish(&self, event: AgentEventEnvelope) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[test]
    fn soft_turn_limits_apply_only_without_an_active_goal() {
        assert!(ordinary_turn_soft_limits(false).is_some());
        assert!(ordinary_turn_soft_limits(true).is_none());
    }

    #[tokio::test]
    async fn async_start_acknowledges_only_after_turn_started_is_published() {
        let delegate = Arc::new(RecordingPublisher::default());
        let (signal, mut started) = tokio::sync::oneshot::channel();
        let publisher =
            TurnStartPublisher::new(delegate.clone(), "thread-1".into(), "turn-1".into(), signal);
        publisher.publish(AgentEventEnvelope::new(AgentEvent::ActivityStatusChanged {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            status: crate::protocol::AgentActivityStatus::Thinking,
        }));
        assert!(started.try_recv().is_err());

        publisher.publish(AgentEventEnvelope::new(AgentEvent::TurnStarted {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            user_message: None,
        }));

        assert!(started.await.unwrap().is_ok());
        assert_eq!(delegate.events.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn async_start_returns_pre_start_errors_through_the_handshake() {
        let delegate = Arc::new(RecordingPublisher::default());
        let (signal, started) = tokio::sync::oneshot::channel();
        let publisher =
            TurnStartPublisher::new(delegate, "thread-1".into(), "turn-1".into(), signal);

        publisher.report_error(CommandError::new("turn_active", "already running"));

        let error = started.await.unwrap().unwrap_err();
        assert_eq!(error, "already running");
    }

    #[test]
    fn queued_start_failure_is_published_when_the_caller_no_longer_waits() {
        let delegate = Arc::new(RecordingPublisher::default());
        let (signal, started) = tokio::sync::oneshot::channel();
        drop(started);
        let publisher = TurnStartPublisher::new(
            delegate.clone(),
            "thread-1".into(),
            "turn-queued".into(),
            signal,
        );

        publisher.report_error(CommandError::new("provider_config", "missing provider"));

        assert!(matches!(
            delegate.events.lock().unwrap().as_slice(),
            [AgentEventEnvelope {
                event: AgentEvent::TurnRejected { turn_id, message, .. },
                ..
            }] if turn_id == "turn-queued" && message == "missing provider"
        ));
    }

    #[test]
    fn craft_mode_can_proactively_clarify_ambiguous_behavior() {
        assert!(CRAFT_MODE_INSTRUCTIONS.contains("request_user_input"));
        assert!(CRAFT_MODE_INSTRUCTIONS.contains("暂停当前 Turn"));
        assert!(CRAFT_MODE_INSTRUCTIONS.contains("破坏性操作"));
        assert!(CRAFT_MODE_INSTRUCTIONS.contains("apply_patch"));
        assert!(CRAFT_MODE_INSTRUCTIONS.contains("不得用普通助手正文"));
        assert!(CRAFT_MODE_INSTRUCTIONS.contains("unified diff"));
    }

    #[test]
    fn workspace_prompt_requires_workspace_relative_tool_paths() {
        let prompt = build_system_prompt(Path::new(r"D:\code\k-coder"), "", "", "", "", &[]);

        assert!(prompt.contains("仅用于识别，不是工具参数"));
        assert!(prompt.contains("必须是相对工作区根目录的路径"));
        assert!(prompt.contains("工作区根目录使用 `.`"));
        assert!(prompt.contains("不得把上面的绝对路径传给工具"));
        assert!(prompt.contains("不得使用 `..`"));
    }

    #[test]
    fn system_prompt_requires_interleaved_progress_without_private_reasoning() {
        let prompt = build_system_prompt(Path::new(r"D:\code\k-coder"), "", "", "", "", &[]);

        assert!(prompt.contains("自然穿插在工具调用之间"));
        assert!(prompt.contains("刚确认的事实和下一步"));
        assert!(prompt.contains("不要输出私有思维链"));
    }

    #[test]
    fn system_prompt_requires_chinese_reasoning_summaries() {
        let prompt = build_system_prompt(Path::new(r"D:\code\k-coder"), "", "", "", "", &[]);

        assert!(prompt.contains("思考摘要语言"));
        assert!(prompt.contains("推理摘要（reasoning summary）"));
        assert!(prompt.contains("必须始终使用中文输出"));
        assert!(prompt.contains("翻译成中文"));
    }

    #[test]
    fn retry_restores_the_failed_turn_mode_from_persisted_events() {
        let events = vec![
            StoredEvent::new(
                "thread",
                Some("turn-plan".into()),
                StoredEventKind::TurnModeSelected {
                    mode: AgentMode::Plan,
                },
            ),
            StoredEvent::new(
                "thread",
                Some("turn-plan".into()),
                StoredEventKind::TurnStarted,
            ),
            StoredEvent::new(
                "thread",
                Some("turn-plan".into()),
                StoredEventKind::TurnFailed {
                    message: "failed".into(),
                    error: None,
                },
            ),
        ];

        assert_eq!(retry_mode(&events), AgentMode::Plan);
    }

    #[test]
    fn retry_uses_legacy_craft_mode_when_no_mode_event_exists() {
        let events = vec![StoredEvent::new(
            "thread",
            Some("legacy-turn".into()),
            StoredEventKind::TurnCancelled,
        )];

        assert_eq!(retry_mode(&events), AgentMode::Craft);
    }
}
