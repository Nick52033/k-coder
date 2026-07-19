pub mod agent;
pub mod app_state;
pub mod commands;
pub mod context;
pub mod execution;
pub mod logging;
pub mod patch;
pub mod persistence;
pub mod policy;
pub mod protocol;
pub mod providers;
pub mod storage;
pub mod tools;

use app_state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_root = app.path().app_data_dir()?.join("runtime-data");
            let state = AppState::new(data_root)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::runtime_status,
            commands::get_provider_config,
            commands::save_provider_config,
            commands::delete_provider_api_key,
            commands::create_thread,
            commands::list_threads,
            commands::read_thread,
            commands::archive_thread,
            commands::compact_thread,
            commands::rebuild_session_projection,
            commands::run_turn,
            commands::retry_turn,
            commands::cancel_turn,
            commands::preview_patch,
            commands::resolve_approval,
            commands::undo_change,
            commands::start_command,
            commands::command_status,
            commands::read_command_output,
            commands::wait_command,
            commands::write_command_stdin,
            commands::cancel_command,
            commands::close_command,
            commands::start_pty,
            commands::pty_status,
            commands::read_pty_output,
            commands::write_pty,
            commands::resize_pty,
            commands::wait_pty,
            commands::close_pty,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
