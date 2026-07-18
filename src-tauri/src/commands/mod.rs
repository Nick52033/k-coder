use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::agent::{AgentRuntime, EventPublisher, RunTurnRequest, TurnOutcome};
use crate::app_state::AppState;
use crate::protocol::{AgentEventEnvelope, RuntimeStatus};
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
        phase: "streaming-chat".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: state.uptime_seconds(),
        capabilities: vec![
            "streaming-chat".to_string(),
            "persistent-threads".to_string(),
            "cancellation".to_string(),
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
        .repository()
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

#[tauri::command]
pub async fn run_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RunTurnRequest,
) -> CommandResult<TurnOutcome> {
    let thread_id = request.thread_id.clone();
    let (provider, model) = state
        .build_provider()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let cancellation = state
        .begin_turn(&thread_id)
        .await
        .map_err(|error| CommandError::new("turn_active", error))?;
    let runtime = AgentRuntime::new(state.runtime_repository());
    let publisher: Arc<dyn EventPublisher> = Arc::new(TauriEventPublisher { app });
    let result = runtime
        .run_turn(provider, model, request, cancellation, publisher)
        .await;
    state.finish_turn(&thread_id).await;
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
    let runtime = AgentRuntime::new(state.runtime_repository());
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
