use std::sync::Arc;

use k_coder_lib::app_state::AppState;
use k_coder_lib::commands;
use k_coder_lib::providers::{CredentialError, CredentialStore};
use serde_json::{Value, json};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{
    INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
};
use tauri::webview::InvokeRequest;
use tauri::{WebviewWindow, WebviewWindowBuilder};

struct EmptyCredentials;

impl CredentialStore for EmptyCredentials {
    fn get_api_key(&self, _: &str) -> Result<Option<String>, CredentialError> {
        Ok(None)
    }
    fn set_api_key(&self, _: &str, _: &str) -> Result<(), CredentialError> {
        Ok(())
    }
    fn delete_api_key(&self, _: &str) -> Result<(), CredentialError> {
        Ok(())
    }
}

fn invoke(window: &WebviewWindow<MockRuntime>, command: &str) -> Result<Value, Value> {
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: command.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: InvokeBody::Json(json!({})),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        },
    )
    .map(|body| body.deserialize::<Value>().unwrap())
}

#[test]
fn log_storage_ipc_returns_camel_case_state_and_rejects_other_windows() {
    let data = tempfile::tempdir().unwrap();
    let state = AppState::with_credentials(data.path(), Arc::new(EmptyCredentials)).unwrap();
    let app = mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_log_storage,
            commands::choose_log_directory,
            commands::reset_log_directory
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let main = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let other = WebviewWindowBuilder::new(&app, "other", Default::default())
        .build()
        .unwrap();
    let storage = invoke(&main, "get_log_storage").unwrap();
    assert_eq!(storage["directory"], storage["defaultDirectory"]);
    assert!(
        storage["filePath"]
            .as_str()
            .unwrap()
            .ends_with("runtime.jsonl")
    );
    assert!(storage["customDirectory"].is_null());
    assert!(storage["warning"].is_null());
    assert_eq!(storage.as_object().unwrap().len(), 5);
    let reset = invoke(&main, "reset_log_directory").unwrap();
    assert_eq!(reset, storage);
    for command in [
        "get_log_storage",
        "choose_log_directory",
        "reset_log_directory",
    ] {
        assert!(invoke(&other, command).is_err());
    }
    assert_eq!(invoke(&main, "get_log_storage").unwrap(), storage);
}
