pub mod agent;
pub mod app_state;
pub mod commands;
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
            commands::run_turn,
            commands::retry_turn,
            commands::cancel_turn,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
