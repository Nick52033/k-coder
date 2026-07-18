pub mod agent;
pub mod app_state;
pub mod commands;
pub mod policy;
pub mod protocol;
pub mod providers;
pub mod storage;
pub mod tools;

use app_state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![commands::runtime_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
