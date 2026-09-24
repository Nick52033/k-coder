use tauri::{AppHandle, Manager};

use crate::channels::wechat_clawbot::service::{
    QrLoginStatus, QrLoginView, WechatClawbotService, WechatStatus,
};

use super::{CommandError, CommandResult};

fn service(app: &AppHandle) -> CommandResult<tauri::State<'_, WechatClawbotService>> {
    app.try_state::<WechatClawbotService>()
        .ok_or_else(|| CommandError::internal("WeChat ClawBot service is unavailable"))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn wechat_clawbot_status(app: AppHandle) -> CommandResult<WechatStatus> {
    Ok(service(&app)?.status())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn wechat_clawbot_start_login(app: AppHandle) -> CommandResult<QrLoginView> {
    service(&app)?
        .start_qr_login()
        .await
        .map_err(|error| CommandError::new("wechat_clawbot", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn wechat_clawbot_login_status(
    app: AppHandle,
    login_id: String,
) -> CommandResult<QrLoginStatus> {
    service(&app)?
        .qr_login_status(&login_id)
        .await
        .map_err(|error| CommandError::new("wechat_clawbot", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn wechat_clawbot_submit_verify_code(
    app: AppHandle,
    login_id: String,
    verify_code: String,
) -> CommandResult<()> {
    service(&app)?
        .submit_verify_code(&login_id, &verify_code)
        .await
        .map_err(|error| CommandError::new("wechat_clawbot", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn wechat_clawbot_approve_sender(
    app: AppHandle,
    sender_key: String,
) -> CommandResult<()> {
    service(&app)?
        .approve_sender(&sender_key)
        .await
        .map_err(|error| CommandError::new("wechat_clawbot", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn wechat_clawbot_revoke_sender(
    app: AppHandle,
    sender_key: String,
) -> CommandResult<()> {
    service(&app)?
        .revoke_sender(&sender_key)
        .await
        .map_err(|error| CommandError::new("wechat_clawbot", error))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn wechat_clawbot_disconnect(app: AppHandle) -> CommandResult<WechatStatus> {
    service(&app)?
        .disconnect()
        .await
        .map_err(|error| CommandError::new("wechat_clawbot", error))
}
