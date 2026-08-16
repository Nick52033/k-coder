use tauri::State;

use crate::agent::AgentRuntime;
use crate::app_state::AppState;
use crate::context::CompactionSummary;
use crate::persistence::UsageSummary;
use crate::protocol::{
    HistorySortDirection, ThreadHistorySnapshot, ThreadItemsPage, ThreadTurnsPage, TurnItemsView,
};
use crate::storage::{ThreadDetail, ThreadSummary};

use super::{CommandError, CommandResult};

#[tauri::command(rename_all = "camelCase")]
pub async fn create_thread(
    state: State<'_, AppState>,
    in_project: Option<bool>,
) -> CommandResult<ThreadSummary> {
    let repository = state.repository();
    if in_project.unwrap_or(true) {
        repository
            .create_thread_in_workspace(&state.workspace_root())
            .await
    } else {
        repository.create_standalone_thread().await
    }
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
pub async fn read_thread_history(
    state: State<'_, AppState>,
    thread_id: String,
) -> CommandResult<ThreadHistorySnapshot> {
    state
        .read_thread_history(&thread_id)
        .await
        .map_err(|error| CommandError::new("storage", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn list_thread_turns(
    state: State<'_, AppState>,
    thread_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
    sort_direction: Option<HistorySortDirection>,
    items_view: Option<TurnItemsView>,
) -> CommandResult<ThreadTurnsPage> {
    state
        .list_thread_turns(
            &thread_id,
            cursor.as_deref(),
            limit,
            sort_direction.unwrap_or_default(),
            items_view.unwrap_or(TurnItemsView::Summary),
        )
        .await
        .map_err(|error| CommandError::new("storage", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn list_thread_items(
    state: State<'_, AppState>,
    thread_id: String,
    turn_id: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
    sort_direction: Option<HistorySortDirection>,
) -> CommandResult<ThreadItemsPage> {
    state
        .list_thread_items(
            &thread_id,
            turn_id.as_deref(),
            cursor.as_deref(),
            limit,
            sort_direction.unwrap_or(HistorySortDirection::Asc),
        )
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
    let project_workspace = state
        .resolve_thread_workspace(&thread_id)
        .await
        .map_err(|error| CommandError::new("workspace_mismatch", error))?;
    let workspace_root = project_workspace
        .clone()
        .unwrap_or_else(|| state.workspace_root());
    let context_limit = state
        .provider_context_limit()
        .map_err(|error| CommandError::new("provider_config", error))?;
    let runtime = AgentRuntime::with_tools_and_approvals(
        state.runtime_repository(),
        if project_workspace.is_some() {
            state.tool_registry()
        } else {
            state
                .tool_registry()
                .restricted_to(&[])
                .map_err(|error| CommandError::new("workspace_tools", error))?
        },
        workspace_root,
        state.approvals(),
    )
    .with_context_limit(context_limit);
    let runtime = runtime.with_metrics(state.advanced().metrics.clone());
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
