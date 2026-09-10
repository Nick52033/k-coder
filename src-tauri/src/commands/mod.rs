use std::sync::{Arc, Mutex as StdMutex};

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::advanced::{
    BrowserArtifact, BrowserAuditEvent, BrowserSettings, CancelWorkflowRunRequest,
    CreateGoalRequest, DocumentContent, EvaluationReport, GoalTransitionRequest, GoalView,
    MemorySettings, MemoryUpsertRequest, MemoryView, MetricsSnapshot, PlanUpdateRequest, PlanView,
    RepositorySearchIndex, SearchResult, WorkflowDefinitionView, WorkflowRunState, WorkflowRunView,
    WorkflowSkillReadinessView, extract_document, extract_document_data_url,
    run_recorded_evaluation,
};
use crate::agent::mailbox::{MailboxTurn, MailboxTurnKind, QueuedTurnSteerError};
use crate::agent::thread_operation::ThreadOperationGuard;
use crate::agent::{
    AgentRuntime, EventPublisher, RunTurnRequest, RuntimeInstructionProvider, SoftTurnLimits,
    TurnOutcome, build_user_message,
};
use crate::app_state::{AppState, AppStateError};
use crate::execution::{
    CommandSessionView, OutputPage, PtyOutputPage, PtySessionView, StartCommandRequest,
    StartPtyRequest,
};
use crate::extensions::{ExtensionOverview, McpConfigView, SaveUserRuleRequest, UserRulesView};
use crate::knowledge::{
    AddSourceRequest, EmbeddingConnectionTest, EmbeddingSettings, KNOWLEDGE_PROGRESS_EVENT_NAME,
    KnowledgeCollection, KnowledgeError, KnowledgeIndexJob, KnowledgeIndexMetrics,
    KnowledgeIndexProgress, KnowledgeProgressSink, KnowledgeSearchResponse, KnowledgeSettings,
    KnowledgeSource, SetEmbeddingSettingsRequest, UpsertCollectionRequest,
};
use crate::logging::{LogQuery, LogQueryResult};
use crate::multi_agent::{
    CreateSubagentRequest, MultiAgentCoordinator, MultiAgentError, SubagentEventPublisher,
    SubagentExecutionContext, SubagentView, delegation_tools,
};
use crate::ocr::{self, OcrResult};
use crate::persistence::ProjectRecord;
use crate::protocol::{
    AgentEvent, AgentEventEnvelope, AgentMode, ApprovalMode, ApprovalResolution, ChangeSet,
    ImageAttachment, MessageRole, PROTOCOL_VERSION, PatchPreview, PluginOverview,
    QueuedTurnSteerRequest, ReasoningEffort, RuntimeStatus, ThreadForkRequest,
    ThreadHistorySnapshot, ThreadMailboxChanged, ThreadMailboxSnapshot, ThreadRollbackRequest,
    TokenUsage, TurnHandle, TurnState, TurnSteerRequest, TurnSteerResponse, UserInputResolution,
};
use crate::providers::{
    ProviderConfigView, ProviderEvent, ProviderMessage, ProviderRequest, SaveProviderConfigRequest,
};
use crate::scheduled_tasks::{ScheduledTaskError, ScheduledTaskView, UpsertScheduledTaskRequest};
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
    let registered = tools.definition_names();
    let allowed = mode
        .allowed_tools()
        .iter()
        .filter(|name| {
            registered
                .iter()
                .any(|registered| registered.as_str() == **name)
        })
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    tools
        .restricted_to(&allowed)
        .map_err(|error| error.to_string())
}

const PROJECT_FREE_TOOL_NAMES: &[&str] = &[
    "browser_click",
    "browser_close",
    "browser_navigate",
    "browser_screenshot",
    "browser_snapshot",
    "browser_type",
    "recall_memory",
    "remember",
    "request_user_input",
    "todo_write",
    "update_goal",
    "update_plan",
];

fn tools_without_project(
    tools: crate::tools::ToolRegistry,
) -> Result<crate::tools::ToolRegistry, String> {
    let allowed = tools
        .definition_names()
        .into_iter()
        .filter(|name| PROJECT_FREE_TOOL_NAMES.contains(&name.as_str()))
        .collect::<Vec<_>>();
    tools
        .restricted_to(&allowed)
        .map_err(|error| error.to_string())
}

fn require_project_thread_for_subagent(summary: &ThreadSummary) -> CommandResult<()> {
    if summary.in_project {
        Ok(())
    } else {
        Err(CommandError::new(
            "standalone_thread",
            "subagents require a project workspace",
        ))
    }
}

fn require_project_thread_for_workflow(summary: &ThreadSummary) -> CommandResult<()> {
    if summary.in_project {
        Ok(())
    } else {
        Err(CommandError::new(
            "standalone_thread",
            "built-in workflows require a project thread",
        ))
    }
}

fn validate_workflow_turn_context(
    has_project: bool,
    agent_mode: AgentMode,
    workflow_requested: bool,
    workflow_active: bool,
) -> CommandResult<()> {
    if !(workflow_requested || workflow_active) {
        return Ok(());
    }
    if !has_project {
        return Err(CommandError::new(
            "standalone_thread",
            "built-in workflows require a project thread",
        ));
    }
    if agent_mode != AgentMode::Craft {
        return Err(CommandError::new(
            "workflow_mode",
            "built-in workflows require Craft mode",
        ));
    }
    Ok(())
}

fn normalize_workflow_id(workflow_id: Option<&str>) -> CommandResult<Option<&str>> {
    let normalized = workflow_id.map(str::trim).filter(|value| !value.is_empty());
    if workflow_id.is_some() && normalized.is_none() {
        return Err(CommandError::new(
            "workflow",
            "workflowId must not be empty when provided",
        ));
    }
    Ok(normalized)
}

async fn require_workflow_skill_preflight(
    state: &AppState,
    workflow_id: &str,
) -> CommandResult<()> {
    let readiness = state
        .get_workflow_skill_readiness(workflow_id)
        .await
        .map_err(|error| {
            CommandError::new("workflow_skill_preflight_failed", error).with_details(
                serde_json::json!({
                    "workflowId": workflow_id,
                    "ready": false,
                }),
            )
        })?;
    if readiness.ready {
        return Ok(());
    }
    Err(CommandError::new(
        "workflow_skill_preflight_failed",
        format!(
            "workflow `{workflow_id}` is not ready: {} skill blocker(s); resolve every blocker before starting or resuming it",
            readiness.blocker_count
        ),
    )
    .with_details(serde_json::json!(readiness)))
}

async fn preflight_requested_or_active_workflow(
    state: &AppState,
    thread_id: &str,
    requested_workflow_id: Option<&str>,
) -> CommandResult<()> {
    let requested_workflow_id = normalize_workflow_id(requested_workflow_id)?;
    let current_workflow = state
        .advanced()
        .workflows
        .current(thread_id)
        .map_err(|error| CommandError::new("workflow", error))?;
    let workflow_id = requested_workflow_id.map(str::to_owned).or_else(|| {
        current_workflow
            .filter(|run| run.state == WorkflowRunState::Active)
            .map(|run| run.workflow_id)
    });
    if let Some(workflow_id) = workflow_id {
        require_workflow_skill_preflight(state, &workflow_id).await?;
    }
    Ok(())
}

fn require_queued_workflow_steerable(workflow_id: Option<&str>) -> CommandResult<()> {
    if workflow_id.is_some() {
        Err(CommandError::new(
            "queued_workflow_not_steerable",
            "a queued workflow start must begin as its own turn",
        ))
    } else {
        Ok(())
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<serde_json::Value>,
}

impl CommandError {
    fn new(code: &'static str, error: impl std::fmt::Display) -> Self {
        Self {
            code,
            message: error.to_string(),
            details: None,
        }
    }

    fn with_details(mut self, details: impl Into<serde_json::Value>) -> Self {
        self.details = Some(details.into());
        self
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self::new("internal_error", error)
    }
}

type CommandResult<T> = Result<T, CommandError>;

fn plugin_command_error(error: impl std::fmt::Display) -> CommandError {
    CommandError::new("plugins", error)
}

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

struct SubagentPublishers {
    agent_events: Arc<dyn EventPublisher>,
    lifecycle_events: Arc<dyn SubagentEventPublisher>,
}

impl SubagentPublishers {
    fn tauri(app: &AppHandle) -> Self {
        Self {
            agent_events: Arc::new(TauriEventPublisher { app: app.clone() }),
            lifecycle_events: Arc::new(TauriSubagentEventPublisher { app: app.clone() }),
        }
    }
}

fn subagent_context(
    state: &AppState,
    provider: Arc<dyn crate::providers::Provider>,
    model: String,
    context_limit: usize,
    tools: crate::tools::ToolRegistry,
    publishers: SubagentPublishers,
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
        agent_events: publishers.agent_events,
        lifecycle_events: publishers.lifecycle_events,
        logger: Some(state.logger()),
    }
}

struct PreparedTurnTools {
    registry: crate::tools::ToolRegistry,
    names: Vec<String>,
}

impl PreparedTurnTools {
    fn new(registry: crate::tools::ToolRegistry) -> Self {
        let names = registry.definition_names();
        Self { registry, names }
    }

    fn with_delegation(
        base_tools: crate::tools::ToolRegistry,
        manager: MultiAgentCoordinator,
        context: SubagentExecutionContext,
        parent_thread_id: String,
        parent_cancellation: CancellationToken,
    ) -> Result<Self, crate::tools::ToolError> {
        let (handlers, risks) =
            delegation_tools(manager, context, parent_thread_id, parent_cancellation);
        base_tools
            .with_additional_handlers(handlers, risks)
            .map(Self::new)
    }

    fn into_parts(self) -> (crate::tools::ToolRegistry, Vec<String>) {
        (self.registry, self.names)
    }
}

/// 分层 system prompt 构建器（借鉴 Codex 的分层 system message 架构）。
/// 把 prompt 按 `<identity>`/`<workspace>`/`<collaboration_mode>`/`<tools>`/`<memory>`/`<extension_prompts>` 分块，
/// 让模型能清晰区分不同层级的指令。
fn build_system_prompt(
    workspace_root: Option<&std::path::Path>,
    extension_instructions: &str,
    advanced_instructions: &str,
    memory_context: &str,
    mode_instructions: &str,
    tool_names: &[String],
) -> String {
    let mut sections = Vec::<String>::new();

    // 1. identity — 固定的身份指令
    sections.push("<identity>\n你是 k-Coder，一个专业的 AI 编码助手。你的能力严格限于当前请求公开的工具；不得假定可以使用未列出的文件、命令、项目或外部能力。\n\n**重要**：请始终用中文回复用户。执行多步骤任务时，把简短、具体的进度说明自然穿插在工具调用之间：第一次调用工具前说明当前目标；完成一组探索、修改或验证工具后，在开始下一组工具前说明刚确认的事实和下一步。不要让长任务退化为连续多轮“模型调用 + 工具调用”而没有用户可见的阶段沟通，也不要为了凑频率逐条复述每个命令。进度说明只包含动作、已确认事实和下一步，不是隐藏推理过程；不要输出私有思维链或逐步内心推演。\n\n**思考摘要语言**：推理摘要（reasoning summary）和思考过程对用户可见，必须始终使用中文输出；即使内部推理使用其他语言，也要把摘要内容翻译成中文后再输出，与界面语言保持一致。\nAll user-visible reasoning summaries must be written in Simplified Chinese. Never use an English heading for a reasoning summary.\n</identity>".to_string());

    // 2. workspace — 工作区信息
    if let Some(workspace_root) = workspace_root {
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
        sections.push("<workspace_tool_protocol>\n调用文件工具前必须先确认路径事实，不要凭记忆拼接文件名：未知位置先用 list_directory 从 `.` 开始逐级查看，或用 search_repository 搜索明确的代码标识或内容并采用结果返回的路径。list_directory 只接受已存在的目录；read_file 只接受一个已存在的普通文件；两者都不接受目录/文件混用、猜测路径或 `*`、`?`、`[...]` 通配符。工具报路径错误后不要重复同一参数，应读取父目录或重新搜索后再试。路径和内容已经确认且文件版本未变化时，不得为了“再次确认真实状态”重复读取高度重叠的行；修改后最多做一次针对改动点的验证，任务已完成时直接给出最终答复。\nWindows PowerShell 下原生 `rg` 不会展开 `dist/assets/index-*.js` 这类路径通配符；请使用 `rg --glob 'index-*.js' -n 'CodeEditor' dist/assets`，或先用 `Get-ChildItem` 取出精确 `.FullName` 再传给 `rg`。若命令把一个原生程序的输出通过管道交给 `rg`，并在正则中使用 `$` 行尾锚点，Windows PowerShell 会把管道内容转换为 CRLF；接收端必须使用 `rg --crlf`（例如 `rg --files path | rg --crlf 'name\\.js$'`），或改用 `Select-String`。\n</workspace_tool_protocol>".to_string());
        sections.push("<workspace_tool_batch_protocol>\n如果路径没有在当前用户请求、工作区上下文或此前的 list_directory/search_repository/read_file 结果中被明确确认，不得调用 read_file 或 list_directory。先单独调用 list_directory 或 search_repository，等待返回结果，再使用返回的精确路径读取；不要在同一批中并行发起发现调用和猜测的读取调用，也不要根据常见命名自行拼接目录或文件名。\n</workspace_tool_batch_protocol>".to_string());
    } else {
        sections.push("<workspace>\n当前会话不在任何项目中。不得读取、修改、搜索或执行任何本地项目内容，也不得把宿主当前打开的工作区当作本会话项目。只有 <available_tools> 中明确列出的非项目工具可用。\n</workspace>".to_string());
    }

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

    if tool_names.iter().any(|name| name == "create_agent")
        && tool_names.iter().any(|name| name == "wait_agent")
    {
        sections.push("<delegation_scheduling>\n需要委派时，先启动相互独立的子任务，再推进自己不依赖其结果的工作；不要重复子任务的工作来填补等待时间。仅当没有可独立推进的工作或下一步依赖结果时调用 wait_agent。多个子任务待收集时，优先用 agentIds 一次等待任意一个结束，及时检查 finishedAgentIds 对应的结果，并只对剩余活动任务继续等待。等待超时不会停止子任务；避免短间隔反复轮询，无事可做时正常等待即可。\n</delegation_scheduling>".into());
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

fn live_runtime_instruction_provider(
    state: &AppState,
    thread_id: String,
    input: String,
    workspace_root: Option<std::path::PathBuf>,
    mode_instructions: String,
    tool_names: Vec<String>,
) -> Arc<dyn RuntimeInstructionProvider> {
    let advanced = state.advanced();
    let extensions = state.extension_service();
    Arc::new(move || {
        let workflow_active = advanced
            .workflows
            .current(&thread_id)
            .map_err(|error| format!("workflow state: {error}"))?
            .is_some_and(|run| run.state == WorkflowRunState::Active);
        let extension_instructions = if workspace_root.is_some() {
            let result = if workflow_active {
                extensions.runtime_instructions_for_robot(&input)
            } else {
                extensions.runtime_instructions(&input)
            };
            result.map_err(|error| format!("extensions: {error}"))?
        } else {
            String::new()
        };
        let mut advanced_instructions = advanced
            .runtime_instructions(&thread_id)
            .map_err(|error| format!("advanced runtime: {error}"))?;
        let workflow_skills = advanced
            .workflows
            .runtime_skill_instructions(&thread_id, &extensions)
            .map_err(|error| format!("workflow Skills: {error}"))?;
        if !workflow_skills.trim().is_empty() {
            if !advanced_instructions.trim().is_empty() {
                advanced_instructions.push_str("\n\n");
            }
            advanced_instructions.push_str(&workflow_skills);
        }
        let memory_instructions = advanced
            .memory
            .context()
            .map_err(|error| format!("memory: {error}"))?;
        Ok(build_system_prompt(
            workspace_root.as_deref(),
            &extension_instructions,
            &advanced_instructions,
            &memory_instructions,
            &mode_instructions,
            &tool_names,
        ))
    })
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
            "builtin-workflows".to_string(),
            "browser-automation".to_string(),
            "repository-search".to_string(),
            "opt-in-memory".to_string(),
            "bounded-document-extraction".to_string(),
            "runtime-metrics".to_string(),
            "knowledge-fts".to_string(),
            "knowledge-citations".to_string(),
            "embedding-configuration".to_string(),
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

#[tauri::command]
pub fn list_builtin_workflows(
    state: State<'_, AppState>,
) -> CommandResult<Vec<WorkflowDefinitionView>> {
    Ok(state.advanced().workflows.definitions())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_workflow_skill_readiness(
    state: State<'_, AppState>,
    workflow_id: String,
) -> CommandResult<WorkflowSkillReadinessView> {
    let workflow_id = normalize_workflow_id(Some(&workflow_id))?
        .expect("a provided workflow id is normalized or rejected");
    state
        .get_workflow_skill_readiness(workflow_id)
        .await
        .map_err(|error| CommandError::new("workflow_skill_readiness", error))
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_workflow_run(
    state: State<'_, AppState>,
    thread_id: String,
) -> CommandResult<Option<WorkflowRunView>> {
    state
        .advanced()
        .workflows
        .current(&thread_id)
        .map_err(|error| CommandError::new("workflow", error))
}

#[tauri::command]
pub async fn cancel_workflow_run(
    state: State<'_, AppState>,
    request: CancelWorkflowRunRequest,
) -> CommandResult<WorkflowRunView> {
    let detail = state
        .repository()
        .read_thread(&request.thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))?;
    require_project_thread_for_workflow(&detail.summary)?;
    state
        .cancel_workflow_run(request)
        .await
        .map_err(|error| match error {
            AppStateError::ThreadOperationBusy(_) | AppStateError::ThreadMailboxNotEmpty(_) => {
                CommandError::new(
                    "workflow_busy",
                    "stop or finish the active and queued turns before cancelling the workflow",
                )
            }
            other => CommandError::new("workflow", other),
        })
}

fn scheduled_task_command_error(error: ScheduledTaskError) -> CommandError {
    let code = match &error {
        ScheduledTaskError::NotFound => "scheduled_task_not_found",
        ScheduledTaskError::Invalid(_) => "scheduled_task_invalid",
        ScheduledTaskError::Storage(_) => "scheduled_task_storage",
    };
    CommandError::new(code, error)
}

#[tauri::command]
pub fn list_scheduled_tasks(state: State<'_, AppState>) -> CommandResult<Vec<ScheduledTaskView>> {
    state
        .scheduled_tasks()
        .list()
        .map_err(scheduled_task_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn upsert_scheduled_task(
    state: State<'_, AppState>,
    request: UpsertScheduledTaskRequest,
) -> CommandResult<ScheduledTaskView> {
    if let Some(thread_id) = request
        .thread_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        state
            .repository()
            .read_thread(thread_id)
            .await
            .map_err(|error| CommandError::new("scheduled_task_thread", error))?;
    }
    state
        .scheduled_tasks()
        .upsert(request, &state.workspace_root())
        .map_err(scheduled_task_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_scheduled_task(state: State<'_, AppState>, task_id: String) -> CommandResult<()> {
    state
        .scheduled_tasks()
        .delete(&task_id)
        .map_err(scheduled_task_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_scheduled_task_enabled(
    state: State<'_, AppState>,
    task_id: String,
    enabled: bool,
) -> CommandResult<ScheduledTaskView> {
    state
        .scheduled_tasks()
        .set_enabled(&task_id, enabled)
        .map_err(scheduled_task_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn trigger_scheduled_task(
    state: State<'_, AppState>,
    task_id: String,
) -> CommandResult<ScheduledTaskView> {
    state
        .scheduled_tasks()
        .trigger_now(&task_id)
        .map_err(scheduled_task_command_error)
}

/// Execute one claimed schedule using the same command-level Turn path as a
/// user request.  This function is called by the scheduler worker and is not a
/// second agent loop.
pub(crate) async fn execute_scheduled_task(
    app: AppHandle,
    task: ScheduledTaskView,
) -> Result<TurnOutcome, String> {
    let state_app = app.clone();
    let state = state_app.state::<AppState>();
    let workspace = std::path::PathBuf::from(&task.workspace_path)
        .canonicalize()
        .map_err(|error| format!("scheduled workspace is unavailable: {error}"))?;
    if workspace != state.workspace_root() {
        return Err(
            "scheduled task workspace is not the active workspace; switch projects before it runs"
                .into(),
        );
    }
    let thread_id = match task.mode {
        crate::scheduled_tasks::ScheduledTaskMode::Thread => task
            .thread_id
            .clone()
            .ok_or_else(|| "scheduled task has no target conversation".to_string())?,
        crate::scheduled_tasks::ScheduledTaskMode::Background => {
            let thread = state
                .repository()
                .create_thread_in_workspace(&workspace)
                .await
                .map_err(|error| error.to_string())?;
            let _ = state
                .repository()
                .rename_thread(&thread.id, task.name.clone())
                .await;
            thread.id
        }
    };
    let publisher: Arc<dyn EventPublisher> = Arc::new(TauriEventPublisher { app: app.clone() });
    execute_turn(
        app,
        state.inner(),
        RunTurnRequest {
            thread_id,
            input: task.prompt,
            agent_mode: Some("craft".into()),
        },
        Vec::new(),
        None,
        None,
        None,
        publisher,
    )
    .await
    .map_err(|error| error.message)
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

fn knowledge_command_error(error: KnowledgeError) -> CommandError {
    CommandError::new(error.code(), error)
}

#[tauri::command]
pub fn get_knowledge_settings(state: State<'_, AppState>) -> CommandResult<KnowledgeSettings> {
    state
        .knowledge()
        .settings()
        .map_err(knowledge_command_error)
}

#[tauri::command]
pub fn set_knowledge_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> CommandResult<KnowledgeSettings> {
    state
        .knowledge()
        .set_enabled(enabled)
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_knowledge_collections(
    state: State<'_, AppState>,
) -> CommandResult<Vec<KnowledgeCollection>> {
    state
        .knowledge()
        .list_collections(&state.workspace_root())
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn upsert_knowledge_collection(
    state: State<'_, AppState>,
    request: UpsertCollectionRequest,
) -> CommandResult<KnowledgeCollection> {
    state
        .knowledge()
        .upsert_collection(&state.workspace_root(), request)
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_knowledge_collection(
    state: State<'_, AppState>,
    collection_id: String,
    confirmation_token: String,
) -> CommandResult<serde_json::Value> {
    state
        .knowledge()
        .delete_collection(&state.workspace_root(), &collection_id, &confirmation_token)
        .await
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn add_knowledge_source(
    state: State<'_, AppState>,
    request: AddSourceRequest,
) -> CommandResult<KnowledgeSource> {
    state
        .knowledge()
        .add_source(&state.workspace_root(), request)
        .await
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_knowledge_sources(
    state: State<'_, AppState>,
    collection_id: String,
) -> CommandResult<Vec<KnowledgeSource>> {
    state
        .knowledge()
        .list_sources(&state.workspace_root(), &collection_id)
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_knowledge_source(
    state: State<'_, AppState>,
    source_id: String,
    confirmation_token: String,
) -> CommandResult<serde_json::Value> {
    state
        .knowledge()
        .delete_source(&state.workspace_root(), &source_id, &confirmation_token)
        .await
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_knowledge_source(
    state: State<'_, AppState>,
    source_id: String,
) -> CommandResult<KnowledgeIndexJob> {
    state
        .knowledge()
        .refresh_source(&state.workspace_root(), &source_id)
        .await
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_knowledge_index_job(
    state: State<'_, AppState>,
    job_id: String,
) -> CommandResult<KnowledgeIndexJob> {
    state
        .knowledge()
        .get_job(&job_id)
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn cancel_knowledge_index_job(
    state: State<'_, AppState>,
    job_id: String,
) -> CommandResult<KnowledgeIndexJob> {
    state
        .knowledge()
        .cancel_job(&job_id)
        .map_err(knowledge_command_error)
}

/// 把知识库索引进度转发给前端。事件只携带不透明 ID、计数和阶段，不含文件内容或路径。
pub struct TauriKnowledgeProgressSink {
    app: AppHandle,
}

impl TauriKnowledgeProgressSink {
    pub fn new(app: AppHandle) -> Arc<Self> {
        Arc::new(Self { app })
    }
}

impl KnowledgeProgressSink for TauriKnowledgeProgressSink {
    fn publish(&self, progress: KnowledgeIndexProgress) {
        let _ = self.app.emit(KNOWLEDGE_PROGRESS_EVENT_NAME, progress);
    }
}

#[tauri::command]
pub fn get_knowledge_metrics(state: State<'_, AppState>) -> CommandResult<KnowledgeIndexMetrics> {
    Ok(state.knowledge().metrics_snapshot())
}

#[tauri::command]
pub fn get_embedding_settings(state: State<'_, AppState>) -> CommandResult<EmbeddingSettings> {
    state
        .knowledge()
        .embedding_settings()
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_embedding_settings(
    state: State<'_, AppState>,
    request: SetEmbeddingSettingsRequest,
) -> CommandResult<EmbeddingSettings> {
    state
        .knowledge()
        .set_embedding_settings(request)
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_embedding_api_key(
    state: State<'_, AppState>,
    api_key: String,
) -> CommandResult<serde_json::Value> {
    state
        .knowledge()
        .set_embedding_key(&api_key)
        .map_err(knowledge_command_error)
}

#[tauri::command]
pub fn delete_embedding_api_key(state: State<'_, AppState>) -> CommandResult<serde_json::Value> {
    state
        .knowledge()
        .delete_embedding_key()
        .map_err(knowledge_command_error)
}

#[tauri::command]
pub async fn test_embedding_connection(
    state: State<'_, AppState>,
) -> CommandResult<EmbeddingConnectionTest> {
    state
        .knowledge()
        .test_embedding_connection()
        .await
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn search_knowledge(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
    thread_id: Option<String>,
    turn_id: Option<String>,
) -> CommandResult<KnowledgeSearchResponse> {
    state
        .knowledge()
        .search(
            &state.workspace_root(),
            thread_id.as_deref().unwrap_or("settings"),
            turn_id.as_deref().unwrap_or("settings"),
            &query,
            limit.unwrap_or(6),
        )
        .await
        .map_err(knowledge_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub fn read_knowledge_citation(
    state: State<'_, AppState>,
    citation_id: String,
    before: Option<usize>,
    after: Option<usize>,
    thread_id: String,
    turn_id: String,
) -> CommandResult<crate::knowledge::KnowledgeCitation> {
    state
        .knowledge()
        .read_citation(
            &thread_id,
            &turn_id,
            &citation_id,
            before.unwrap_or(0),
            after.unwrap_or(0),
        )
        .map_err(knowledge_command_error)
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

#[tauri::command(rename_all = "camelCase")]
pub fn read_browser_artifact(state: State<'_, AppState>, name: String) -> CommandResult<String> {
    use base64::Engine as _;
    let bytes = state
        .advanced()
        .browser
        .read_artifact(&name)
        .map_err(|error| CommandError::new("browser", error))?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn read_message_image(
    state: State<'_, AppState>,
    thread_id: String,
    path: String,
) -> CommandResult<String> {
    let root = state
        .resolve_thread_workspace(&thread_id)
        .await
        .map_err(|error| CommandError::new("workspace_mismatch", error))?
        .ok_or_else(|| CommandError::new("image", "无项目会话不能读取工作区图片"))?;
    tauri::async_runtime::spawn_blocking(move || workbench::read_message_image(&root, &path))
        .await
        .map_err(|error| CommandError::new("image", error))?
        .map_err(|error| CommandError::new("image", error))
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
            ProviderEvent::Usage { usage: value }
            | ProviderEvent::DetailedUsage { usage: value, .. } => usage = Some(value),
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

#[tauri::command]
pub async fn user_rules(state: State<'_, AppState>, refresh: bool) -> CommandResult<UserRulesView> {
    let prepared = state.prepare_extensions(refresh).await;
    let mut view = state
        .user_rules_view()
        .map_err(|error| CommandError::new("extensions", error))?;
    if let Err(error) = prepared {
        view.error = Some(error.to_string());
    }
    Ok(view)
}

#[tauri::command]
pub async fn save_user_rule(
    state: State<'_, AppState>,
    request: SaveUserRuleRequest,
) -> CommandResult<UserRulesView> {
    state
        .save_user_rule(request)
        .await
        .map_err(|error| CommandError::new("extensions", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_user_rule(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<UserRulesView> {
    state
        .delete_user_rule(&id)
        .await
        .map_err(|error| CommandError::new("extensions", error))
}

#[tauri::command]
pub async fn plugin_overview(
    state: State<'_, AppState>,
    refresh: bool,
) -> CommandResult<PluginOverview> {
    Ok(state.plugin_overview(refresh).await)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_plugin_enabled(
    state: State<'_, AppState>,
    plugin_id: String,
    enabled: bool,
) -> CommandResult<PluginOverview> {
    state
        .set_plugin_enabled(&plugin_id, enabled)
        .await
        .map_err(plugin_command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_plugin(
    state: State<'_, AppState>,
    plugin_id: String,
) -> CommandResult<PluginOverview> {
    state
        .delete_plugin(&plugin_id)
        .await
        .map_err(plugin_command_error)
}

#[tauri::command]
pub async fn mcp_config(state: State<'_, AppState>, refresh: bool) -> CommandResult<McpConfigView> {
    let prepared = state.prepare_extensions(refresh).await;
    let mut view = state
        .mcp_config_view()
        .map_err(|error| CommandError::new("extensions", error))?;
    if let Err(error) = prepared {
        view.overview.error = Some(error.to_string());
    }
    Ok(view)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn save_mcp_config(
    state: State<'_, AppState>,
    scope: String,
    content: String,
) -> CommandResult<McpConfigView> {
    state
        .save_mcp_config(&scope, &content)
        .map_err(|error| CommandError::new("extensions", error))?;
    let prepared = state.prepare_extensions(true).await;
    let mut view = state
        .mcp_config_view()
        .map_err(|error| CommandError::new("extensions", error))?;
    if let Err(error) = prepared {
        view.overview.error = Some(error.to_string());
    }
    Ok(view)
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
pub async fn extract_local_document(
    name: String,
    data_url: String,
) -> CommandResult<AttachmentContent> {
    let document =
        tauri::async_runtime::spawn_blocking(move || extract_document_data_url(&name, &data_url))
            .await
            .map_err(|error| CommandError::new("attachment", error))?
            .map_err(|error| CommandError::new("attachment", error))?;
    Ok(AttachmentContent {
        path: document.path,
        name: document.name,
        kind: "document".into(),
        content: document.content,
        size: document.source_bytes,
        truncated: document.truncated,
    })
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
pub async fn run_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RunTurnRequest,
    attachments: Vec<ImageAttachment>,
    workflow_id: Option<String>,
) -> CommandResult<TurnOutcome> {
    let publisher: Arc<dyn EventPublisher> = Arc::new(TauriEventPublisher { app: app.clone() });
    execute_turn(
        app,
        state.inner(),
        request,
        attachments,
        workflow_id,
        None,
        None,
        publisher,
    )
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn turn_start(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RunTurnRequest,
    attachments: Vec<ImageAttachment>,
    workflow_id: Option<String>,
) -> CommandResult<TurnHandle> {
    preflight_requested_or_active_workflow(
        state.inner(),
        &request.thread_id,
        workflow_id.as_deref(),
    )
    .await?;
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
                workflow_id,
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
    preflight_requested_or_active_workflow(state.inner(), &thread_id, None).await?;
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
                workflow_id,
            } => {
                execute_turn(
                    app.clone(),
                    state.inner(),
                    request,
                    attachments,
                    workflow_id,
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
                    SubagentPublishers::tauri(&app),
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
    require_queued_workflow_steerable(pending.workflow_id.as_deref())?;
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
    workflow_id: Option<String>,
    assigned_turn_id: Option<String>,
    operation_guard: Option<ThreadOperationGuard>,
    publisher: Arc<dyn EventPublisher>,
) -> CommandResult<TurnOutcome> {
    let mut attachments = attachments;
    let has_image_attachments = !attachments.is_empty();
    let thread_id = request.thread_id.clone();
    let project_workspace = state
        .resolve_thread_workspace(&thread_id)
        .await
        .map_err(|error| CommandError::new("workspace_mismatch", error))?;
    let workspace_root = project_workspace
        .clone()
        .unwrap_or_else(|| state.workspace_root());
    let has_project = project_workspace.is_some();
    let advanced = state.advanced();
    let agent_mode = request
        .agent_mode
        .as_deref()
        .map(AgentMode::from_str)
        .unwrap_or_default();
    let requested_workflow_id = normalize_workflow_id(workflow_id.as_deref())?;
    let current_workflow = advanced
        .workflows
        .current(&thread_id)
        .map_err(|error| CommandError::new("workflow", error))?;
    let workflow_preflight_id = requested_workflow_id.map(str::to_owned).or_else(|| {
        current_workflow
            .as_ref()
            .filter(|run| run.state == WorkflowRunState::Active)
            .map(|run| run.workflow_id.clone())
    });
    validate_workflow_turn_context(
        has_project,
        agent_mode,
        requested_workflow_id.is_some(),
        current_workflow
            .as_ref()
            .is_some_and(|run| run.state == WorkflowRunState::Active),
    )?;
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
    if let Some(workflow_id) = workflow_preflight_id.as_deref() {
        require_workflow_skill_preflight(state, workflow_id).await?;
    } else if has_project {
        state
            .prepare_extensions(false)
            .await
            .map_err(|error| CommandError::new("extensions", error))?;
    }
    // 根据协作模式注入指令并限制可用工具
    let mode_instructions = instructions_for_mode(agent_mode).to_string();

    // Plan/Ask 模式下把工具限制为只读子集（借鉴 Codex 的 plan_mask）。
    let mode_tools = tools_for_mode(state.tool_registry(), agent_mode)
        .map_err(|error| CommandError::new("agent_mode", error))?;
    let base_tools = if has_project {
        mode_tools
    } else {
        tools_without_project(mode_tools)
            .map_err(|error| CommandError::new("workspace_tools", error))?
    };

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
    if let Some(workflow_id) = requested_workflow_id {
        let objective = if request.input.trim().is_empty() {
            "Process the user-provided attachments under this workflow."
        } else {
            request.input.as_str()
        };
        advanced
            .workflows
            .start_or_resume(&thread_id, workflow_id, objective)
            .map_err(|error| CommandError::new("workflow", error))?;
    }
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
    let prepared_tools = if has_project {
        let child_context = subagent_context(
            &state,
            provider.clone(),
            model.clone(),
            context_limit,
            base_tools.clone(),
            SubagentPublishers::tauri(&app),
        );
        match PreparedTurnTools::with_delegation(
            base_tools,
            state.subagents(),
            child_context,
            thread_id.clone(),
            cancellation.child_token(),
        ) {
            Ok(tools) => tools,
            Err(error) => {
                if let Some(timeout) = goal_timeout {
                    timeout.abort();
                }
                state.finish_turn(&thread_id).await;
                return Err(CommandError::new("multi_agent", error));
            }
        }
    } else {
        PreparedTurnTools::new(base_tools)
    };
    let (tools, tool_names) = prepared_tools.into_parts();
    let runtime_instruction_provider = live_runtime_instruction_provider(
        state,
        thread_id.clone(),
        request.input.clone(),
        project_workspace.clone(),
        mode_instructions,
        tool_names,
    );
    let mut runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        tools,
        workspace_root,
        state.approvals(),
    )
    .with_approval_mode(state.approval_mode())
    .with_runtime_instruction_provider(runtime_instruction_provider)
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
    execute_retry(
        state.inner(),
        thread_id,
        None,
        None,
        publisher,
        SubagentPublishers::tauri(&app),
    )
    .await
}

async fn execute_retry(
    state: &AppState,
    thread_id: String,
    assigned_turn_id: Option<String>,
    operation_guard: Option<ThreadOperationGuard>,
    publisher: Arc<dyn EventPublisher>,
    subagent_publishers: SubagentPublishers,
) -> CommandResult<TurnOutcome> {
    let project_workspace = state
        .resolve_thread_workspace(&thread_id)
        .await
        .map_err(|error| CommandError::new("workspace_mismatch", error))?;
    let workspace_root = project_workspace
        .clone()
        .unwrap_or_else(|| state.workspace_root());
    let has_project = project_workspace.is_some();
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
    let active_workflow_id = advanced
        .workflows
        .current(&thread_id)
        .map_err(|error| CommandError::new("workflow", error))?
        .filter(|run| run.state == WorkflowRunState::Active)
        .map(|run| run.workflow_id);
    let workflow_active = active_workflow_id.is_some();
    validate_workflow_turn_context(has_project, agent_mode, false, workflow_active)?;
    if let Some(workflow_id) = active_workflow_id.as_deref() {
        require_workflow_skill_preflight(state, workflow_id).await?;
    } else if has_project {
        state
            .prepare_extensions(false)
            .await
            .map_err(|error| CommandError::new("extensions", error))?;
    }
    let mode_instructions = instructions_for_mode(agent_mode).to_string();
    let mode_tools = tools_for_mode(state.tool_registry(), agent_mode)
        .map_err(|error| CommandError::new("agent_mode", error))?;
    let base_tools = if has_project {
        mode_tools
    } else {
        tools_without_project(mode_tools)
            .map_err(|error| CommandError::new("workspace_tools", error))?
    };
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
    let prepared_tools = if has_project {
        let child_context = subagent_context(
            &state,
            provider.clone(),
            model.clone(),
            context_limit,
            base_tools.clone(),
            subagent_publishers,
        );
        match PreparedTurnTools::with_delegation(
            base_tools,
            state.subagents(),
            child_context,
            thread_id.clone(),
            cancellation.child_token(),
        ) {
            Ok(tools) => tools,
            Err(error) => {
                if let Some(timeout) = goal_timeout {
                    timeout.abort();
                }
                state.finish_turn(&thread_id).await;
                return Err(CommandError::new("multi_agent", error));
            }
        }
    } else {
        PreparedTurnTools::new(base_tools)
    };
    let (tools, tool_names) = prepared_tools.into_parts();
    let runtime_instruction_provider = live_runtime_instruction_provider(
        state,
        thread_id.clone(),
        retry_input,
        project_workspace.clone(),
        mode_instructions,
        tool_names,
    );
    let mut runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        tools,
        workspace_root,
        state.approvals(),
    )
    .with_approval_mode(state.approval_mode())
    .with_runtime_instruction_provider(runtime_instruction_provider)
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
    let parent = state
        .repository()
        .read_thread(&request.parent_thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))?;
    require_project_thread_for_subagent(&parent.summary)?;
    state
        .prepare_extensions(false)
        .await
        .map_err(|error| CommandError::new("extensions", error))?;
    let (provider, model, context_limit) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let context = subagent_context(
        state.inner(),
        provider,
        model,
        context_limit,
        state.tool_registry(),
        SubagentPublishers::tauri(&app),
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
    trigger_turn: Option<bool>,
) -> CommandResult<SubagentView> {
    let (provider, model, context_limit) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let context = subagent_context(
        state.inner(),
        provider,
        model,
        context_limit,
        state.tool_registry(),
        SubagentPublishers::tauri(&app),
    );
    state
        .subagents()
        .send_message(&agent_id, message, context, trigger_turn.unwrap_or(true))
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
        state.inner(),
        provider,
        model,
        context_limit,
        state.tool_registry(),
        SubagentPublishers::tauri(&app),
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
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use base64::Engine;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::{
        CRAFT_MODE_INSTRUCTIONS, CommandError, PROJECT_FREE_TOOL_NAMES, PreparedTurnTools,
        SubagentPublishers, TurnStartPublisher, build_system_prompt, execute_retry,
        extract_local_document, ordinary_turn_soft_limits, plugin_command_error,
        preflight_requested_or_active_workflow, require_project_thread_for_subagent,
        require_project_thread_for_workflow, require_queued_workflow_steerable, retry_mode,
        tools_for_mode, tools_without_project, validate_workflow_turn_context,
    };
    use crate::agent::mailbox::{MailboxTurn, MailboxTurnKind};
    use crate::agent::{AgentRuntime, EventPublisher, RunTurnRequest};
    use crate::app_state::AppState;
    use crate::multi_agent::{
        MultiAgentCoordinator, NoopSubagentPublisher, SubagentExecutionContext,
    };
    use crate::policy::ApprovalManager;
    use crate::protocol::{
        AgentEvent, AgentEventEnvelope, AgentMode, ApprovalMode, PROTOCOL_VERSION, ReasoningEffort,
    };
    use crate::providers::{
        CredentialError, CredentialStore, ProviderError, ProviderKind, ProviderModelConfig,
        ProviderTransport, SaveProviderConfigRequest, testing::FakeProvider,
    };
    use crate::storage::{JsonlThreadRepository, StoredEvent, StoredEventKind, ThreadSummary};
    use crate::{patch::PatchService, tools::ToolRegistry};

    #[derive(Default)]
    struct RecordingPublisher {
        events: Mutex<Vec<AgentEventEnvelope>>,
    }

    #[test]
    fn plugin_command_errors_use_a_stable_public_code() {
        let error = plugin_command_error("unknown local plugin review-tools@local");

        assert_eq!(error.code, "plugins");
        assert_eq!(error.message, "unknown local plugin review-tools@local");
    }

    impl EventPublisher for RecordingPublisher {
        fn publish(&self, event: AgentEventEnvelope) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn test_subagent_publishers() -> SubagentPublishers {
        SubagentPublishers {
            agent_events: Arc::new(RecordingPublisher::default()),
            lifecycle_events: Arc::new(NoopSubagentPublisher),
        }
    }

    #[derive(Default)]
    struct TestCredentials {
        api_keys: Mutex<HashMap<String, String>>,
    }

    impl CredentialStore for TestCredentials {
        fn get_api_key(&self, provider_id: &str) -> Result<Option<String>, CredentialError> {
            Ok(self.api_keys.lock().unwrap().get(provider_id).cloned())
        }

        fn set_api_key(&self, provider_id: &str, api_key: &str) -> Result<(), CredentialError> {
            self.api_keys
                .lock()
                .unwrap()
                .insert(provider_id.to_string(), api_key.to_string());
            Ok(())
        }

        fn delete_api_key(&self, provider_id: &str) -> Result<(), CredentialError> {
            self.api_keys.lock().unwrap().remove(provider_id);
            Ok(())
        }
    }

    async fn read_test_http_json(stream: &mut TcpStream) -> serde_json::Value {
        let mut request = Vec::new();
        let header_end = loop {
            let mut chunk = [0_u8; 4096];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "request ended before its headers");
            request.extend_from_slice(&chunk[..read]);
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = std::str::from_utf8(&request[..header_end]).unwrap();
        let content_length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while request.len() < header_end + content_length {
            let mut chunk = [0_u8; 4096];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "request ended before its body");
            request.extend_from_slice(&chunk[..read]);
        }
        serde_json::from_slice(&request[header_end..header_end + content_length]).unwrap()
    }

    async fn spawn_retry_provider_server(
        request_count: usize,
    ) -> (
        String,
        mpsc::Receiver<serde_json::Value>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel(request_count);
        let body = concat!(
            r#"data: {"choices":[{"delta":{"content":"retry complete"},"finish_reason":"stop"}]}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let server = tokio::spawn(async move {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().await.unwrap();
                sender
                    .send(read_test_http_json(&mut stream).await)
                    .await
                    .unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{address}/v1"), receiver, server)
    }

    fn provider_tool_names(payload: &serde_json::Value) -> Vec<String> {
        payload["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect()
    }

    fn disclosed_tool_names(payload: &serde_json::Value) -> Vec<String> {
        let prompt = payload["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "system")
            .filter_map(|message| message["content"].as_str())
            .find(|content| content.contains("<available_tools>"))
            .unwrap();
        prompt
            .split("<available_tools>\n")
            .nth(1)
            .and_then(|section| section.split("\n</available_tools>").next())
            .unwrap()
            .lines()
            .map(|line| line.strip_prefix("- ").unwrap().to_string())
            .collect()
    }

    async fn seed_failed_turn(state: &AppState, thread_id: &str, workspace: &Path) {
        let runtime = AgentRuntime::with_tools(
            state.repository(),
            ToolRegistry::read_only(),
            workspace.to_path_buf(),
        );
        let _ = runtime
            .run_turn(
                Arc::new(FakeProvider::new(vec![Err(
                    ProviderError::InvalidResponse("intentional retry fixture failure".into()),
                )])),
                "fixture".into(),
                RunTurnRequest {
                    thread_id: thread_id.to_string(),
                    input: "Use a subagent for this inspection.".into(),
                    agent_mode: Some("craft".into()),
                },
                CancellationToken::new(),
                Arc::new(RecordingPublisher::default()),
            )
            .await;
        assert_eq!(
            state
                .repository()
                .read_thread(thread_id)
                .await
                .unwrap()
                .last_turn
                .unwrap()
                .state,
            crate::protocol::TurnState::Failed
        );
    }

    fn prepared_project_turn_tools() -> (tempfile::TempDir, PreparedTurnTools) {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let repository = Arc::new(JsonlThreadRepository::new(data.path()).unwrap());
        let base_tools = ToolRegistry::read_only();
        let context = SubagentExecutionContext {
            repository,
            provider: Arc::new(FakeProvider::text(&["unused"])),
            model: "fixture".into(),
            context_limit: crate::context::DEFAULT_CONTEXT_LIMIT,
            tools: base_tools.clone(),
            workspace_root: workspace.path().to_path_buf(),
            approvals: Arc::new(ApprovalManager::new(Duration::from_secs(1))),
            approval_mode: ApprovalMode::Ask,
            reasoning_effort: ReasoningEffort::default(),
            agent_events: Arc::new(RecordingPublisher::default()),
            lifecycle_events: Arc::new(NoopSubagentPublisher),
            logger: None,
        };
        let tools = PreparedTurnTools::with_delegation(
            base_tools,
            MultiAgentCoordinator::new(data.path()).unwrap(),
            context,
            "parent-thread".into(),
            CancellationToken::new(),
        )
        .unwrap();
        drop(workspace);
        (data, tools)
    }

    #[test]
    fn project_turn_tools_expose_every_delegation_operation() {
        let (_data, tools) = prepared_project_turn_tools();

        for expected in [
            "close_agent",
            "create_agent",
            "list_agents",
            "resume_agent",
            "send_agent_message",
            "wait_agent",
        ] {
            assert!(
                tools.names.iter().any(|name| name == expected),
                "project turn should expose {expected}"
            );
        }
    }

    #[test]
    fn system_prompt_discloses_the_final_provider_tool_set() {
        let (_data, tools) = prepared_project_turn_tools();
        let provider_names = tools
            .registry
            .provider_definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();
        let prompt = build_system_prompt(
            Some(Path::new(r"D:\code\k-coder")),
            "",
            "",
            "",
            "",
            &tools.names,
        );
        let disclosed_names = prompt
            .split("<available_tools>\n")
            .nth(1)
            .and_then(|section| section.split("\n</available_tools>").next())
            .unwrap()
            .lines()
            .map(|line| line.strip_prefix("- ").unwrap().to_string())
            .collect::<Vec<_>>();

        assert_eq!(disclosed_names, provider_names);
        assert!(prompt.contains("<delegation_scheduling>"));
        assert!(prompt.contains("finishedAgentIds"));
        assert!(
            !build_system_prompt(None, "", "", "", "", &[]).contains("<delegation_scheduling>")
        );
    }

    #[tokio::test]
    async fn direct_mailbox_and_standalone_retries_use_the_final_scoped_tool_registry() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let (base_url, mut requests, server) = spawn_retry_provider_server(3).await;
        let state =
            AppState::with_credentials(data.path(), Arc::new(TestCredentials::default())).unwrap();
        state
            .save_provider_config(SaveProviderConfigRequest {
                id: "retry-contract".into(),
                kind: ProviderKind::OpenAiCompatible,
                transport: ProviderTransport::OpenAiChatCompletions,
                name: "Retry contract fixture".into(),
                base_url,
                model: "fixture".into(),
                models: vec![ProviderModelConfig {
                    id: "fixture".into(),
                    display_name: "Fixture".into(),
                    context_window: 128_000,
                    max_output_tokens: Some(256),
                    supports_vision: false,
                    fallback: false,
                }],
                endpoints: Vec::new(),
                api_key: Some("fixture-key".into()),
                activate: true,
            })
            .unwrap();
        state.switch_workspace(workspace.path()).await.unwrap();

        let direct_project = state
            .repository()
            .create_thread_in_workspace(workspace.path())
            .await
            .unwrap();
        let standalone = state.repository().create_standalone_thread().await.unwrap();
        let mailbox_project = state
            .repository()
            .create_thread_in_workspace(workspace.path())
            .await
            .unwrap();
        for thread in [&direct_project, &standalone, &mailbox_project] {
            seed_failed_turn(&state, &thread.id, workspace.path()).await;
        }

        let direct = execute_retry(
            &state,
            direct_project.id.clone(),
            None,
            None,
            Arc::new(RecordingPublisher::default()),
            test_subagent_publishers(),
        )
        .await
        .unwrap();
        assert_eq!(direct.state, crate::protocol::TurnState::Completed);

        let standalone_outcome = execute_retry(
            &state,
            standalone.id.clone(),
            None,
            None,
            Arc::new(RecordingPublisher::default()),
            test_subagent_publishers(),
        )
        .await
        .unwrap();
        assert_eq!(
            standalone_outcome.state,
            crate::protocol::TurnState::Completed
        );

        let mailbox_turn_id = Uuid::new_v4().to_string();
        assert!(
            state
                .enqueue_thread_turn(MailboxTurn {
                    handle: crate::protocol::TurnHandle {
                        schema_version: PROTOCOL_VERSION,
                        thread_id: mailbox_project.id.clone(),
                        turn_id: mailbox_turn_id.clone(),
                        state: crate::protocol::TurnState::Queued,
                    },
                    kind: MailboxTurnKind::Retry,
                    started: None,
                })
                .await
        );
        let (queued_retry, operation_guard) = state
            .next_thread_turn(&mailbox_project.id)
            .await
            .expect("mailbox retry should be dequeued with its operation guard");
        assert!(matches!(queued_retry.kind, MailboxTurnKind::Retry));
        let mailbox = execute_retry(
            &state,
            queued_retry.handle.thread_id,
            Some(queued_retry.handle.turn_id),
            Some(operation_guard),
            Arc::new(RecordingPublisher::default()),
            test_subagent_publishers(),
        )
        .await
        .unwrap();
        assert_eq!(mailbox.turn_id, mailbox_turn_id);
        assert_eq!(mailbox.state, crate::protocol::TurnState::Completed);

        let direct_request = requests.recv().await.unwrap();
        let standalone_request = requests.recv().await.unwrap();
        let mailbox_request = requests.recv().await.unwrap();
        server.await.unwrap();

        let delegation_tools = [
            "close_agent",
            "create_agent",
            "list_agents",
            "resume_agent",
            "send_agent_message",
            "wait_agent",
        ];
        for request in [&direct_request, &mailbox_request] {
            let provider_names = provider_tool_names(request);
            assert_eq!(disclosed_tool_names(request), provider_names);
            for expected in delegation_tools {
                assert!(
                    provider_names.iter().any(|name| name == expected),
                    "project retry should expose {expected}"
                );
            }
        }

        let standalone_names = provider_tool_names(&standalone_request);
        assert_eq!(disclosed_tool_names(&standalone_request), standalone_names);
        assert!(standalone_names.iter().all(|name| {
            PROJECT_FREE_TOOL_NAMES.contains(&name.as_str())
                && !delegation_tools.contains(&name.as_str())
        }));
    }

    #[test]
    fn soft_turn_limits_apply_only_without_an_active_goal() {
        assert!(ordinary_turn_soft_limits(false).is_some());
        assert!(ordinary_turn_soft_limits(true).is_none());
    }

    #[tokio::test]
    async fn local_document_command_accepts_only_user_supplied_memory_content() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("release notes");
        let attachment = extract_local_document(
            "notes.md".into(),
            format!("data:text/markdown;base64,{encoded}"),
        )
        .await
        .unwrap();

        assert_eq!(attachment.kind, "document");
        assert_eq!(attachment.name, "notes.md");
        assert_eq!(attachment.content, "release notes");
        assert_eq!(attachment.size, 13);
        assert!(attachment.path.starts_with("attachment://"));

        let error =
            extract_local_document("../notes.md".into(), "data:text/plain;base64,QQ==".into())
                .await
                .unwrap_err();
        assert_eq!(error.code, "attachment");
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

    #[tokio::test]
    async fn workflow_preflight_returns_all_blockers_before_state_or_turn_events() {
        let data = tempfile::tempdir().unwrap();
        let builtin = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = AppState::new_with_builtin_skills(data.path(), builtin.path()).unwrap();
        state.switch_workspace(workspace.path()).await.unwrap();
        let publisher = RecordingPublisher::default();

        let error = preflight_requested_or_active_workflow(
            &state,
            "thread-without-run",
            Some("quality-assurance"),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code, "workflow_skill_preflight_failed");
        let details = error.details.as_ref().unwrap();
        assert_eq!(details["workflowId"], "quality-assurance");
        assert_eq!(details["ready"], false);
        assert_eq!(details["blockerCount"], 16);
        assert_eq!(details["blockers"].as_array().unwrap().len(), 16);
        assert!(!serde_json::to_string(details).unwrap().contains("\"body\""));
        assert!(
            state
                .advanced()
                .workflows
                .current("thread-without-run")
                .unwrap()
                .is_none()
        );
        assert!(publisher.events.lock().unwrap().is_empty());
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
        let prompt = build_system_prompt(Some(Path::new(r"D:\code\k-coder")), "", "", "", "", &[]);

        assert!(prompt.contains("仅用于识别，不是工具参数"));
        assert!(prompt.contains("必须是相对工作区根目录的路径"));
        assert!(prompt.contains("工作区根目录使用 `.`"));
        assert!(prompt.contains("不得把上面的绝对路径传给工具"));
        assert!(prompt.contains("不得使用 `..`"));
        assert!(prompt.contains("<workspace_tool_protocol>"));
        assert!(prompt.contains("<workspace_tool_batch_protocol>"));
        assert!(prompt.contains("list_directory 只接受已存在的目录"));
        assert!(prompt.contains("read_file 只接受一个已存在的普通文件"));
        assert!(prompt.contains("工具报路径错误后不要重复同一参数"));
        assert!(prompt.contains("不得为了“再次确认真实状态”重复读取高度重叠的行"));
        assert!(prompt.contains("修改后最多做一次针对改动点的验证"));
        assert!(prompt.contains("不要在同一批中并行发起发现调用和猜测的读取调用"));
        assert!(prompt.contains("不会展开 `dist/assets/index-*.js`"));
        assert!(prompt.contains("rg --glob 'index-*.js'"));
        assert!(prompt.contains("Windows PowerShell 会把管道内容转换为 CRLF"));
        assert!(prompt.contains("rg --files path | rg --crlf 'name\\.js$'"));
    }

    #[test]
    fn standalone_prompt_hides_the_active_workspace_and_denies_project_access() {
        let prompt = build_system_prompt(None, "", "", "", "", &[]);

        assert!(prompt.contains("当前会话不在任何项目中"));
        assert!(prompt.contains("不得读取、修改、搜索或执行任何本地项目内容"));
        assert!(!prompt.contains(r"D:\code\k-coder"));
    }

    #[test]
    fn standalone_tool_filter_removes_workspace_tools() {
        let tools = tools_without_project(ToolRegistry::workspace_tools(PatchService::new()))
            .expect("standalone filtering should succeed");

        assert!(tools.definition_names().is_empty());
    }

    #[test]
    fn read_only_modes_allow_optional_plugin_read_tools_without_requiring_them() {
        assert!(
            crate::protocol::AgentMode::Ask
                .allowed_tools()
                .contains(&"plugin_skill_read")
        );
        assert!(
            crate::protocol::AgentMode::Plan
                .allowed_tools()
                .contains(&"plugin_resource_read")
        );

        let tools = tools_for_mode(ToolRegistry::read_only(), crate::protocol::AgentMode::Ask)
            .expect("optional plugin tools should not be required when no plugin is enabled");
        assert_eq!(
            tools.definition_names(),
            vec!["list_directory", "read_file"]
        );
    }

    #[test]
    fn standalone_threads_cannot_create_subagents() {
        let summary = ThreadSummary {
            schema_version: PROTOCOL_VERSION,
            id: "standalone-thread".into(),
            title: "Standalone".into(),
            created_at_ms: 1,
            updated_at_ms: 1,
            archived: false,
            in_project: false,
            workspace_path: None,
        };

        let error = require_project_thread_for_subagent(&summary).unwrap_err();
        assert_eq!(error.code, "standalone_thread");
        let workflow_error = require_project_thread_for_workflow(&summary).unwrap_err();
        assert_eq!(workflow_error.code, "standalone_thread");
    }

    #[test]
    fn workflow_turns_require_a_project_and_craft_mode() {
        let standalone =
            validate_workflow_turn_context(false, AgentMode::Craft, true, false).unwrap_err();
        assert_eq!(standalone.code, "standalone_thread");

        let read_only =
            validate_workflow_turn_context(true, AgentMode::Ask, false, true).unwrap_err();
        assert_eq!(read_only.code, "workflow_mode");
        assert!(validate_workflow_turn_context(true, AgentMode::Craft, true, false).is_ok());
        assert!(validate_workflow_turn_context(false, AgentMode::Ask, false, false).is_ok());
    }

    #[test]
    fn queued_workflow_starts_cannot_be_steered_into_an_active_turn() {
        let error = require_queued_workflow_steerable(Some("quality-assurance")).unwrap_err();
        assert_eq!(error.code, "queued_workflow_not_steerable");
        assert!(require_queued_workflow_steerable(None).is_ok());
    }

    #[test]
    fn system_prompt_requires_interleaved_progress_without_private_reasoning() {
        let prompt = build_system_prompt(Some(Path::new(r"D:\code\k-coder")), "", "", "", "", &[]);

        assert!(prompt.contains("自然穿插在工具调用之间"));
        assert!(prompt.contains("刚确认的事实和下一步"));
        assert!(prompt.contains("不要输出私有思维链"));
    }

    #[test]
    fn system_prompt_requires_chinese_reasoning_summaries() {
        let prompt = build_system_prompt(Some(Path::new(r"D:\code\k-coder")), "", "", "", "", &[]);

        assert!(prompt.contains("思考摘要语言"));
        assert!(prompt.contains("推理摘要（reasoning summary）"));
        assert!(prompt.contains("必须始终使用中文输出"));
        assert!(prompt.contains("翻译成中文"));
        assert!(prompt.contains("must be written in Simplified Chinese"));
        assert!(prompt.contains("Never use an English heading"));
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
