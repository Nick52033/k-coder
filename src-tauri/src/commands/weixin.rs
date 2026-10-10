use super::{CommandError, CommandResult};
use crate::weixin::{WeixinSettings, WeixinStatus};
use tauri::{AppHandle, Manager};

#[tauri::command(rename_all = "camelCase")]
pub fn weixin_status(app: AppHandle) -> CommandResult<WeixinStatus> {
    Ok(app
        .try_state::<crate::weixin::WeixinService>()
        .ok_or_else(|| CommandError::internal("微信服务不可用"))?
        .status())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn weixin_start(
    app: AppHandle,
    thread_id: String,
    remember_login: bool,
    auto_connect: bool,
) -> CommandResult<WeixinStatus> {
    let service = app
        .try_state::<crate::weixin::WeixinService>()
        .ok_or_else(|| CommandError::internal("微信服务不可用"))?;
    service
        .start(
            app.clone(),
            WeixinSettings {
                thread_id,
                remember_login,
                auto_connect,
            },
        )
        .await
        .map_err(|e| CommandError::new("weixin", e))
}

#[tauri::command]
pub fn weixin_stop(app: AppHandle) -> CommandResult<WeixinStatus> {
    let service = app
        .try_state::<crate::weixin::WeixinService>()
        .ok_or_else(|| CommandError::internal("微信服务不可用"))?;
    service.stop();
    Ok(service.status())
}
