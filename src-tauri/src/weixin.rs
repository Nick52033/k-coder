//! 微信 ClawBot 接入。扫码登录后，微信消息进入已绑定的桌面会话。
//! 该模块只负责传输和会话路由，Turn 仍统一走 commands::enqueue_message_turn。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager, Wry};
use tokio_util::sync::CancellationToken;

const ENDPOINT: &str = "https://ilinkai.weixin.qq.com/";
const BASE_INFO: &str = "{\"channel_version\":\"2.4.9\",\"bot_agent\":\"k-Coder/0.10.0\"}";

fn official_base(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value).map_err(|_| "微信接口地址无效".to_string())?;
    let host = url.host_str().unwrap_or_default();
    let valid_host = host.ends_with(".weixin.qq.com")
        && host.strip_suffix(".weixin.qq.com").is_some_and(|prefix| {
            prefix == "ilink"
                || (prefix.starts_with("ilink-")
                    && prefix[6..]
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
        });
    if url.scheme() != "https"
        || !valid_host
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("微信接口地址不受信任".into());
    }
    Ok(url.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WeixinStatus {
    pub running: bool,
    pub phase: String,
    pub qr_code: Option<String>,
    pub qr_image_content: Option<String>,
    pub thread_id: Option<String>,
    pub remembered: bool,
    pub auto_connect: bool,
    pub owner: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeixinSettings {
    pub thread_id: String,
    pub remember_login: bool,
    pub auto_connect: bool,
}

struct Inner {
    status: WeixinStatus,
    cancel: Option<CancellationToken>,
}

pub struct WeixinService {
    inner: Arc<Mutex<Inner>>,
}

impl WeixinService {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                status: WeixinStatus::default(),
                cancel: None,
            })),
        }
    }
    pub fn status(&self) -> WeixinStatus {
        self.inner.lock().unwrap().status.clone()
    }
    pub fn stop(&self) {
        if let Some(token) = self.inner.lock().unwrap().cancel.take() {
            token.cancel();
        }
        self.inner.lock().unwrap().status.running = false;
    }
    pub async fn start(
        &self,
        app: AppHandle<Wry>,
        settings: WeixinSettings,
    ) -> Result<WeixinStatus, String> {
        if settings.thread_id.trim().is_empty() {
            return Err("请先选择要绑定的桌面会话".into());
        }
        if let Some(state) = app.try_state::<crate::app_state::AppState>() {
            if !settings.remember_login {
                let _ = state.delete_credential("weixin-bot-session");
            }
            let _ = state.repository().projection().set_setting(
                "weixin/settings",
                &serde_json::to_string(&settings).unwrap_or_default(),
            );
        }
        self.stop();
        let token = CancellationToken::new();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.cancel = Some(token.clone());
            inner.status = WeixinStatus {
                running: true,
                phase: "正在获取微信二维码".into(),
                thread_id: Some(settings.thread_id.clone()),
                remembered: settings.remember_login,
                auto_connect: settings.auto_connect,
                ..Default::default()
            };
        }
        let shared = self.inner.clone();
        tokio::spawn(async move {
            if let Err(error) = run(app, shared.clone(), token, settings, None).await {
                let mut i = shared.lock().unwrap();
                i.status.running = false;
                i.status.error = Some(error);
                i.status.phase = "连接失败".into();
            }
        });
        Ok(self.status())
    }

    pub fn restore_on_startup(&self, app: AppHandle<Wry>) {
        let Some(state) = app.try_state::<crate::app_state::AppState>() else {
            return;
        };
        let Ok(Some(raw)) = state.repository().projection().setting("weixin/settings") else {
            return;
        };
        let Ok(settings) = serde_json::from_str::<WeixinSettings>(&raw) else {
            return;
        };
        if !settings.auto_connect || !settings.remember_login {
            return;
        }
        let Ok(Some(saved)) = state.get_credential("weixin-bot-session") else {
            return;
        };
        let Ok(value) = serde_json::from_str::<Value>(&saved) else {
            return;
        };
        let Some(token) = value.get("token").and_then(Value::as_str) else {
            return;
        };
        let Some(owner) = value.get("owner").and_then(Value::as_str) else {
            return;
        };
        let Some(base) = value.get("base").and_then(Value::as_str) else {
            return;
        };
        let token_cancel = CancellationToken::new();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.cancel = Some(token_cancel.clone());
            inner.status = WeixinStatus {
                running: true,
                phase: "正在使用已保存的微信登录连接".into(),
                thread_id: Some(settings.thread_id.clone()),
                remembered: true,
                auto_connect: true,
                ..Default::default()
            };
        }
        let shared = self.inner.clone();
        let account = (token.to_string(), owner.to_string(), base.to_string());
        tokio::spawn(async move {
            if let Err(error) =
                run(app, shared.clone(), token_cancel, settings, Some(account)).await
            {
                let mut i = shared.lock().unwrap();
                i.status.running = false;
                i.status.error = Some(error);
                i.status.phase = "自动连接失败".into();
            }
        });
    }
}

#[derive(Deserialize)]
struct Qr {
    qrcode: String,
    qrcode_img_content: String,
}
#[derive(Deserialize)]
struct QrState {
    status: String,
    bot_token: Option<String>,
    ilink_user_id: Option<String>,
    baseurl: Option<String>,
    redirect_host: Option<String>,
}

async fn call(
    client: &Client,
    base: &str,
    route: &str,
    body: Option<Value>,
    token: Option<&str>,
) -> Result<Value, String> {
    let url = format!(
        "{}{}",
        base.trim_end_matches('/'),
        if route.starts_with('/') {
            route.to_string()
        } else {
            format!("/{route}")
        }
    );
    let mut request = if body.is_some() {
        client.post(url)
    } else {
        client.get(url)
    };
    request = request
        .header("iLink-App-Id", "bot")
        .header("iLink-App-ClientVersion", "132105");
    if let Some(token) = token {
        request = request
            .bearer_auth(token)
            .header("AuthorizationType", "ilink_bot_token");
    }
    if let Some(body) = body {
        request = request
            .header("Content-Type", "application/json")
            .body(body.to_string());
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("微信接口 HTTP {}", response.status()));
    }
    let value: Value = response.json().await.map_err(|e| e.to_string())?;
    if value
        .get("ret")
        .and_then(Value::as_i64)
        .is_some_and(|v| v != 0)
    {
        return Err("微信接口返回错误".into());
    }
    Ok(value)
}

async fn run(
    app: AppHandle<Wry>,
    shared: Arc<Mutex<Inner>>,
    cancel: CancellationToken,
    settings: WeixinSettings,
    saved_account: Option<(String, String, String)>,
) -> Result<(), String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(35))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let account = if let Some(account) = saved_account {
        account
    } else {
        let qr: Qr = serde_json::from_value(
            call(
                &client,
                ENDPOINT,
                "ilink/bot/get_bot_qrcode?bot_type=3",
                Some(json!({"local_token_list":[]})),
                None,
            )
            .await?,
        )
        .map_err(|e| e.to_string())?;
        {
            let mut i = shared.lock().unwrap();
            i.status.qr_code = Some(qr.qrcode.clone());
            i.status.qr_image_content = Some(qr.qrcode_img_content.clone());
            i.status.phase = "请用微信扫码并在手机确认连接".into();
        }
        let mut base = ENDPOINT.to_string();
        let account = loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let encoded: String =
                url::form_urlencoded::byte_serialize(qr.qrcode.as_bytes()).collect();
            let route = format!("ilink/bot/get_qrcode_status?qrcode={encoded}");
            let state: QrState =
                serde_json::from_value(call(&client, &base, &route, None, None).await?)
                    .map_err(|e| e.to_string())?;
            match state.status.as_str() {
                "confirmed" => {
                    break (
                        state.bot_token.ok_or("微信未返回登录凭据")?,
                        state.ilink_user_id.ok_or("微信未返回用户标识")?,
                        official_base(&state.baseurl.unwrap_or(base))?,
                    );
                }
                "scaned_but_redirect" => {
                    base = official_base(&format!(
                        "https://{}/",
                        state.redirect_host.ok_or("微信重定向地址缺失")?
                    ))?;
                }
                "need_verifycode" => {
                    let mut i = shared.lock().unwrap();
                    i.status.phase = "请在设置页输入微信显示的验证码".into();
                }
                "expired" => return Err("二维码已过期，请重新扫码".into()),
                _ => {}
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        account
    };
    if settings.remember_login {
        if let Some(state) = app.try_state::<crate::app_state::AppState>() {
            let _ = state.set_credential(
                "weixin-bot-session",
                &serde_json::json!({"token": account.0, "owner": account.1, "base": account.2})
                    .to_string(),
            );
        }
    }
    {
        let mut i = shared.lock().unwrap();
        i.status.qr_code = None;
        i.status.qr_image_content = None;
        i.status.owner = Some(account.1.clone());
        i.status.phase = "微信已连接，等待消息".into();
    }
    let mut cursor = String::new();
    let mut seen = std::collections::HashSet::new();
    while !cancel.is_cancelled() {
        let updates = call(&client, &account.2, "ilink/bot/getupdates", Some(json!({"get_updates_buf":cursor,"base_info":serde_json::from_str::<Value>(BASE_INFO).unwrap()})), Some(&account.0)).await?;
        cursor = updates
            .get("get_updates_buf")
            .and_then(Value::as_str)
            .unwrap_or(&cursor)
            .to_string();
        for message in updates
            .get("msgs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = message
                .get("message_id")
                .or_else(|| message.get("msg_id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty()
                || !seen.insert(id)
                || message.get("from_user_id").and_then(Value::as_str) != Some(&account.1)
            {
                continue;
            }
            let text = message
                .get("item_list")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|x| {
                            x.get("text_item")
                                .and_then(|v| v.get("text"))
                                .and_then(Value::as_str)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
                .trim()
                .to_string();
            if text.is_empty() {
                continue;
            }
            let reply = execute_message(&app, &settings.thread_id, &text).await;
            let context = message
                .get("context_token")
                .and_then(Value::as_str)
                .unwrap_or("");
            call(&client, &account.2, "ilink/bot/sendmessage", Some(json!({"base_info":serde_json::from_str::<Value>(BASE_INFO).unwrap(),"msg":{"from_user_id":"","to_user_id":account.1,"client_id":format!("kcoder-{}",uuid::Uuid::new_v4()),"message_type":2,"message_state":2,"item_list":[{"type":1,"text_item":{"text":reply}}],"context_token":context}})), Some(&account.0)).await?;
        }
    }
    Ok(())
}

async fn execute_message(app: &AppHandle<Wry>, thread_id: &str, text: &str) -> String {
    let Some(state) = app.try_state::<crate::app_state::AppState>() else {
        return "k-Coder 当前不可用".into();
    };
    if let Some((command, number, rest)) = parse_numbered_command(text) {
        let Ok(detail) = state.read_thread(thread_id).await else {
            return "无法读取会话状态".into();
        };
        if let Ok(index) = number.parse::<usize>() {
            if command == "approve" || command == "reject" {
                let pending: Vec<_> = detail
                    .approvals
                    .iter()
                    .filter(|item| item.resolution.is_none())
                    .collect();
                if let Some(item) = pending.get(index.saturating_sub(1)) {
                    let action = if command == "approve" {
                        crate::protocol::ApprovalAction::Approved
                    } else {
                        crate::protocol::ApprovalAction::Rejected
                    };
                    let (patch, selected_paths, expected_hashes) = if command == "approve" {
                        item.request
                            .preview
                            .as_ref()
                            .map(|preview| {
                                (
                                    if item.request.tool_name == "apply_patch" {
                                        Some(preview.patch.clone())
                                    } else {
                                        None
                                    },
                                    preview.files.iter().map(|file| file.path.clone()).collect(),
                                    preview
                                        .files
                                        .iter()
                                        .map(|file| crate::protocol::ExpectedFileHash {
                                            path: file.path.clone(),
                                            before_hash: file.before_hash.clone(),
                                        })
                                        .collect(),
                                )
                            })
                            .unwrap_or((None, Vec::new(), Vec::new()))
                    } else {
                        (None, Vec::new(), Vec::new())
                    };
                    return match state
                        .approvals()
                        .resolve(
                            &item.request.id,
                            crate::protocol::ApprovalResolution {
                                action,
                                patch,
                                selected_paths,
                                expected_hashes,
                            },
                        )
                        .await
                    {
                        Ok(()) => format!(
                            "已{}审批 {}",
                            if command == "approve" {
                                "批准"
                            } else {
                                "拒绝"
                            },
                            number
                        ),
                        Err(error) => format!("审批 {} 已失效：{error}", number),
                    };
                }
                return format!("审批编号 {} 不存在、已处理或已过期", number);
            }
            if command == "answer" {
                let pending: Vec<_> = detail
                    .user_inputs
                    .iter()
                    .filter(|item| item.resolution.is_none())
                    .collect();
                if let Some(item) = pending.get(index.saturating_sub(1)) {
                    let answer = crate::protocol::UserInputAnswer {
                        question: item
                            .request
                            .questions
                            .first()
                            .map(|q| q.question.clone())
                            .unwrap_or_default(),
                        answer: rest.to_string(),
                    };
                    return match state
                        .user_inputs()
                        .resolve(
                            &item.request.id,
                            crate::protocol::UserInputResolution {
                                action: crate::protocol::UserInputAction::Answered,
                                answers: vec![answer],
                            },
                        )
                        .await
                    {
                        Ok(()) => format!("已回答问题 {}", number),
                        Err(error) => format!("问题 {} 已失效：{error}", number),
                    };
                }
                return format!("问题编号 {} 不存在、已处理或已过期", number);
            }
        }
    }
    if let Some(turn_id) = text
        .strip_prefix("/stop ")
        .and_then(|v| v.split_whitespace().next())
    {
        return if state.cancel_turn(thread_id).await {
            format!("已停止 Turn {turn_id}")
        } else {
            "停止失败：会话中没有对应的运行任务".into()
        };
    }
    let request = crate::agent::RunTurnRequest {
        thread_id: thread_id.into(),
        input: text.into(),
        agent_mode: None,
    };
    match crate::commands::enqueue_message_turn(
        app.clone(),
        state.inner(),
        request,
        Vec::new(),
        None,
    )
    .await
    {
        Ok(handle) => {
            for _ in 0..600 {
                if let Ok(detail) = state.read_thread(thread_id).await {
                    if detail.last_turn.as_ref().is_some_and(|t| {
                        t.turn_id == handle.turn_id
                            && matches!(
                                t.state,
                                crate::protocol::TurnState::Completed
                                    | crate::protocol::TurnState::Failed
                                    | crate::protocol::TurnState::Cancelled
                            )
                    }) {
                        return detail
                            .messages
                            .last()
                            .map(|m| m.visible_text())
                            .unwrap_or_else(|| "Turn 已完成".into());
                    }
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            "Turn 超时，请回到桌面查看状态".into()
        }
        Err(error) => format!("无法启动 Turn：{}", error.message),
    }
}

fn parse_numbered_command(text: &str) -> Option<(&str, &str, &str)> {
    let mut parts = text.trim().splitn(3, char::is_whitespace);
    let command = parts.next()?.strip_prefix('/')?;
    let number = parts.next()?.trim();
    if !number.chars().all(|c| c.is_ascii_digit()) || number == "0" {
        return None;
    }
    Some((command, number, parts.next().unwrap_or_default().trim()))
}
