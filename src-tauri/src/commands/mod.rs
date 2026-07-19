use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::agent::{AgentRuntime, EventPublisher, RunTurnRequest, TurnOutcome};
use crate::app_state::AppState;
use crate::context::CompactionSummary;
use crate::execution::{
    CommandSessionView, OutputPage, PtyOutputPage, PtySessionView, StartCommandRequest,
    StartPtyRequest,
};
use crate::protocol::{
    AgentEvent, AgentEventEnvelope, ApprovalResolution, ChangeSet, PatchPreview, RuntimeStatus,
};
use crate::providers::{ProviderConfigView, SaveProviderConfigRequest};
use crate::storage::{ThreadDetail, ThreadSummary};

const AGENT_EVENT_NAME: &str = "agent-event";

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

#[tauri::command]
pub fn runtime_status(state: State<'_, AppState>) -> RuntimeStatus {
    RuntimeStatus {
        ready: true,
        phase: "resilient-context".to_string(),
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
        ],
    }
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
    let runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        state.tool_registry(),
        state.workspace_root(),
        state.approvals(),
    );
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
) -> CommandResult<TurnOutcome> {
    let thread_id = request.thread_id.clone();
    let _ = state.logger().log(
        "info",
        "turn_requested",
        serde_json::json!({"threadId": thread_id}),
    );
    let (provider, model) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let cancellation = state
        .begin_turn(&thread_id)
        .await
        .map_err(|error| CommandError::new("turn_active", error))?;
    let runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        state.tool_registry(),
        state.workspace_root(),
        state.approvals(),
    );
    let publisher: Arc<dyn EventPublisher> = Arc::new(TauriEventPublisher { app });
    let result = runtime
        .run_turn(provider, model, request, cancellation, publisher)
        .await;
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
    let (provider, model) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let cancellation = state
        .begin_turn(&thread_id)
        .await
        .map_err(|error| CommandError::new("turn_active", error))?;
    let runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        state.tool_registry(),
        state.workspace_root(),
        state.approvals(),
    );
    let publisher: Arc<dyn EventPublisher> = Arc::new(TauriEventPublisher { app });
    let result = runtime
        .retry_turn(provider, model, thread_id.clone(), cancellation, publisher)
        .await;
    state.finish_turn(&thread_id).await;
    result.map_err(|error| CommandError::new("agent_runtime", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn cancel_turn(state: State<'_, AppState>, thread_id: String) -> CommandResult<bool> {
    Ok(state.cancel_turn(&thread_id).await)
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
