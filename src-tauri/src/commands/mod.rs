use tauri::State;

use crate::app_state::AppState;
use crate::protocol::RuntimeStatus;

#[tauri::command]
pub fn runtime_status(state: State<'_, AppState>) -> RuntimeStatus {
    RuntimeStatus {
        ready: true,
        phase: "foundation".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: state.uptime_seconds(),
        capabilities: vec!["desktop-shell".to_string(), "typed-ipc".to_string()],
    }
}
