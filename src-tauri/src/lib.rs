pub mod advanced;
pub mod agent;
pub mod app_state;
pub mod commands;
pub mod context;
pub mod execution;
pub mod extensions;
pub mod logging;
pub mod multi_agent;
pub mod ocr;
pub mod patch;
pub mod persistence;
pub mod policy;
pub mod protocol;
pub mod providers;
pub mod storage;
pub mod tools;
pub mod workbench;

use app_state::AppState;
use std::path::{Path, PathBuf};
use tauri::Manager;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
#[cfg(desktop)]
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

#[cfg(desktop)]
const SINGLE_INSTANCE_TITLE: &str = "k-Coder 已在运行";
#[cfg(desktop)]
const SINGLE_INSTANCE_MESSAGE: &str = "k-Coder 已在运行，已切换到现有窗口。";

fn select_builtin_skills_root(resource_dir: &Path, development_root: Option<&Path>) -> PathBuf {
    development_root
        .filter(|root| root.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| resource_dir.join("skills"))
}

fn builtin_skills_root(resource_dir: &Path) -> PathBuf {
    #[cfg(debug_assertions)]
    let development_root =
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/resources/skills"));
    #[cfg(not(debug_assertions))]
    let development_root: Option<PathBuf> = None;

    select_builtin_skills_root(resource_dir, development_root.as_deref())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        show_main_window(app);
        app.dialog()
            .message(SINGLE_INSTANCE_MESSAGE)
            .kind(MessageDialogKind::Info)
            .title(SINGLE_INSTANCE_TITLE)
            .show(|_| {});
    }));

    builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let data_root = app.path().app_data_dir()?.join("runtime-data");
            let builtin_skills_root = builtin_skills_root(&app.path().resource_dir()?);
            let state = AppState::new_with_builtin_skills(data_root, builtin_skills_root)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            app.manage(state);

            // 创建系统托盘
            let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;

            let _tray = TrayIconBuilder::new()
                .icon(Image::from_bytes(include_bytes!("../icons/icon.png"))?)
                .tooltip("k-Coder")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(move |app, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        ..
                    } = event
                    {
                        show_main_window(app.app_handle());
                    }
                })
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            // 拦截窗口关闭事件：隐藏到托盘而非退出
            let window = app.get_webview_window("main").unwrap();
            let window_clone = window.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window_clone.hide();
                    // 发送系统通知，告诉用户窗口已最小化到托盘
                    let app_handle = window_clone.app_handle().clone();
                    let notification_app = app_handle.clone();
                    let _ = app_handle.run_on_main_thread(move || {
                        let _ = tauri_plugin_notification::NotificationExt::notification(
                            &notification_app,
                        )
                        .builder()
                        .title("k-Coder")
                        .body("已最小化到系统托盘，右键托盘图标可退出")
                        .show();
                    });
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::runtime_status,
            commands::get_approval_mode,
            commands::set_approval_mode,
            commands::get_reasoning_effort,
            commands::set_reasoning_effort,
            commands::get_plan,
            commands::update_plan,
            commands::get_goal,
            commands::create_goal,
            commands::transition_goal,
            commands::search_repository,
            commands::get_memory_settings,
            commands::set_memory_enabled,
            commands::list_memories,
            commands::upsert_memory,
            commands::delete_memory,
            commands::get_browser_settings,
            commands::save_browser_settings,
            commands::list_browser_audit,
            commands::list_browser_artifacts,
            commands::close_browser_session,
            commands::extract_document_content,
            commands::advanced_metrics,
            commands::run_regression_evaluation,
            commands::get_provider_config,
            commands::get_provider_catalog,
            commands::save_provider_config,
            commands::activate_provider,
            commands::delete_provider,
            commands::test_provider_connection,
            commands::delete_provider_api_key,
            commands::create_thread,
            commands::list_threads,
            commands::search_threads,
            commands::rename_thread,
            commands::delete_thread,
            commands::usage_summary,
            commands::workspace_state,
            commands::switch_workspace,
            commands::list_workspace_directory,
            commands::search_workspace_files,
            commands::preview_workspace_file,
            commands::save_workspace_file,
            commands::extract_attachment,
            commands::open_workspace_file,
            commands::reveal_workspace_file,
            commands::git_status,
            commands::git_diff,
            commands::git_branches,
            commands::git_switch_branch,
            commands::git_action,
            commands::extension_overview,
            commands::set_extension_enabled,
            commands::save_mcp_secret,
            commands::delete_mcp_secret,
            commands::read_thread,
            commands::read_thread_history,
            commands::list_thread_turns,
            commands::list_thread_items,
            commands::archive_thread,
            commands::compact_thread,
            commands::rebuild_session_projection,
            commands::turn_start,
            commands::turn_retry,
            commands::read_thread_mailbox,
            commands::remove_queued_turn,
            commands::clear_thread_mailbox,
            commands::turn_steer,
            commands::turn_steer_queued,
            commands::turn_interrupt,
            commands::thread_fork,
            commands::thread_resume,
            commands::thread_rollback,
            commands::run_turn,
            commands::retry_turn,
            commands::cancel_turn,
            commands::create_subagent,
            commands::list_subagents,
            commands::wait_subagent,
            commands::send_subagent_message,
            commands::resume_subagent,
            commands::close_subagent,
            commands::preview_patch,
            commands::resolve_approval,
            commands::resolve_user_input,
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
            commands::recognize_image,
            commands::read_logs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(all(test, desktop))]
mod tests {
    use super::{SINGLE_INSTANCE_MESSAGE, SINGLE_INSTANCE_TITLE};

    #[test]
    fn single_instance_notice_explains_existing_window_reuse() {
        assert_eq!(SINGLE_INSTANCE_TITLE, "k-Coder 已在运行");
        assert_eq!(
            SINGLE_INSTANCE_MESSAGE,
            "k-Coder 已在运行，已切换到现有窗口。"
        );
    }
}
