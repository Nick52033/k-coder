use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::RunTurnRequest;
use crate::persistence::ProjectionDb;
use crate::protocol::{AgentEvent, AgentEventEnvelope, ImageAttachment, TurnHandle};

use super::protocol::{
    GetUpdatesRequest, GetUpdatesResponse, IlinkMessage, QrCodeResponse, QrStatusResponse,
    SendMessageRequest, SendMessageResponse, split_reply, validate_api_base_url,
};

const DEFAULT_API_BASE: &str = "https://ilinkai.weixin.qq.com";
const SETTINGS_KEY: &str = "wechat_clawbot_state_v1";
const CREDENTIAL_SERVICE: &str = "com.kcoder.app";
const CREDENTIAL_ACCOUNT: &str = "wechat-clawbot-bot-token";
const MAX_SEEN_MESSAGE_IDS: usize = 2_000;
const MAX_REPLY_CHUNK_BYTES: usize = 4_000;

#[derive(Clone, Copy)]
enum HeaderMode {
    QrStart,
    QrPoll,
    Bot,
}

pub trait IlinkTokenStore: Send + Sync {
    fn get(&self) -> Result<Option<String>, String>;
    fn set(&self, token: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

#[derive(Default)]
pub struct OsIlinkTokenStore {
    #[cfg(not(test))]
    lock: Mutex<()>,
}

impl IlinkTokenStore for OsIlinkTokenStore {
    fn get(&self) -> Result<Option<String>, String> {
        #[cfg(test)]
        {
            return Ok(None);
        }
        #[cfg(not(test))]
        {
            let _guard = self.lock.lock().map_err(|_| "credential lock poisoned".to_owned())?;
            let entry = keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT)
                .map_err(|error| error.to_string())?;
            match entry.get_password() {
                Ok(value) => Ok(Some(value)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        }
    }

    fn set(&self, token: &str) -> Result<(), String> {
        if token.trim().is_empty() {
            return Err("empty iLink token".to_owned());
        }
        #[cfg(test)]
        {
            let _ = token;
            return Err("native credentials disabled in tests".to_owned());
        }
        #[cfg(not(test))]
        {
            let _guard = self.lock.lock().map_err(|_| "credential lock poisoned".to_owned())?;
            keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT)
                .map_err(|error| error.to_string())?
                .set_password(token.trim())
                .map_err(|error| error.to_string())
        }
    }

    fn delete(&self) -> Result<(), String> {
        #[cfg(test)]
        {
            return Ok(());
        }
        #[cfg(not(test))]
        {
            let _guard = self.lock.lock().map_err(|_| "credential lock poisoned".to_owned())?;
            let entry = keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT)
                .map_err(|error| error.to_string())?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    api_base_url: Option<String>,
    update_cursor: String,
    senders: BTreeMap<String, SenderRecord>,
    seen_message_ids: VecDeque<u64>,
    paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SenderRecord {
    approved: bool,
    thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SenderView {
    /// A SHA-256 digest, never the raw WeChat user identifier.
    pub sender_key: String,
    pub approved: bool,
    pub has_thread: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WechatStatus {
    pub connected: bool,
    pub polling: bool,
    pub paused: bool,
    pub pending_senders: Vec<SenderView>,
    pub approved_senders: Vec<SenderView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QrLoginView {
    pub login_id: String,
    pub qr_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QrLoginStatus {
    pub phase: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
struct LoginSession {
    qrcode: String,
    api_base_url: String,
    phase: String,
    verify_code: Option<String>,
    message: Option<String>,
    expires_at: tokio::time::Instant,
}

#[derive(Debug, Clone)]
struct ReplyContext {
    sender_key: String,
    peer_id: String,
    context_token: String,
}

struct Inner {
    app: AppHandle,
    projection: ProjectionDb,
    client: reqwest::Client,
    token_store: Arc<dyn IlinkTokenStore>,
    state: Mutex<PersistedState>,
    logins: AsyncMutex<BTreeMap<String, LoginSession>>,
    poller: AsyncMutex<Option<CancellationToken>>,
    replies: Mutex<BTreeMap<(String, String), ReplyContext>>,
}

pub struct WechatClawbotService {
    inner: Arc<Inner>,
}

impl Clone for WechatClawbotService {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl WechatClawbotService {
    pub fn new(app: AppHandle, projection: ProjectionDb) -> Result<Self, String> {
        Self::with_token_store(app, projection, Arc::new(OsIlinkTokenStore::default()))
    }

    pub fn with_token_store(
        app: AppHandle,
        projection: ProjectionDb,
        token_store: Arc<dyn IlinkTokenStore>,
    ) -> Result<Self, String> {
        let state = projection
            .setting(SETTINGS_KEY)
            .map_err(|error| error.to_string())?
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(40))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            inner: Arc::new(Inner {
                app,
                projection,
                client,
                token_store,
                state: Mutex::new(state),
                logins: AsyncMutex::new(BTreeMap::new()),
                poller: AsyncMutex::new(None),
                replies: Mutex::new(BTreeMap::new()),
            }),
        })
    }

    pub fn status(&self) -> WechatStatus {
        let connected = self.inner.token_store.get().ok().flatten().is_some();
        let state = self.inner.state.lock().expect("WeChat state lock poisoned");
        let mut pending_senders = Vec::new();
        let mut approved_senders = Vec::new();
        for (sender_key, sender) in &state.senders {
            let view = SenderView {
                sender_key: sender_key.chars().take(16).collect(),
                approved: sender.approved,
                has_thread: sender.thread_id.is_some(),
            };
            if sender.approved {
                approved_senders.push(view);
            } else {
                pending_senders.push(view);
            }
        }
        WechatStatus {
            connected,
            polling: self.inner.poller.try_lock().map(|guard| guard.as_ref().is_some_and(|token| !token.is_cancelled())).unwrap_or(true),
            paused: state.paused,
            pending_senders,
            approved_senders,
        }
    }

    pub async fn start_qr_login(&self) -> Result<QrLoginView, String> {
        if self.inner.token_store.get()?.is_some() {
            return Err("微信 ClawBot 已连接，请先断开当前账号".to_owned());
        }
        let qrcode_response: QrCodeResponse = self
            .post_json(
                DEFAULT_API_BASE,
                "ilink/bot/get_bot_qrcode?bot_type=3",
                &serde_json::json!({ "local_token_list": [] }),
                None,
                HeaderMode::QrStart,
                Duration::from_secs(15),
            )
            .await?;
        if qrcode_response.qrcode.is_empty()
            || qrcode_response.qrcode_img_content.is_empty()
            || qrcode_response.qrcode_img_content.len() > 8 * 1024
        {
            return Err("iLink returned an invalid QR code response".to_owned());
        }
        let login_id = Uuid::new_v4().to_string();
        self.inner.logins.lock().await.insert(
            login_id.clone(),
            LoginSession {
                qrcode: qrcode_response.qrcode,
                api_base_url: DEFAULT_API_BASE.to_owned(),
                phase: "waiting".to_owned(),
                verify_code: None,
                message: None,
                expires_at: tokio::time::Instant::now() + Duration::from_secs(5 * 60),
            },
        );
        let service = self.clone();
        let background_id = login_id.clone();
        tauri::async_runtime::spawn(async move {
            service.run_qr_login(background_id).await;
        });
        Ok(QrLoginView {
            login_id,
            qr_content: qrcode_response.qrcode_img_content,
        })
    }

    pub async fn qr_login_status(&self, login_id: &str) -> Result<QrLoginStatus, String> {
        let logins = self.inner.logins.lock().await;
        let session = logins
            .get(login_id)
            .ok_or_else(|| "QR login has expired".to_owned())?;
        Ok(QrLoginStatus {
            phase: session.phase.clone(),
            message: session.message.clone(),
        })
    }

    pub async fn submit_verify_code(&self, login_id: &str, verify_code: &str) -> Result<(), String> {
        let verify_code = verify_code.trim();
        if !(4..=12).contains(&verify_code.len())
            || !verify_code.chars().all(|character| character.is_ascii_alphanumeric())
        {
            return Err("验证码格式无效".to_owned());
        }
        let mut logins = self.inner.logins.lock().await;
        let session = logins
            .get_mut(login_id)
            .ok_or_else(|| "QR login has expired".to_owned())?;
        session.verify_code = Some(verify_code.to_owned());
        session.phase = "verifying".to_owned();
        session.message = None;
        Ok(())
    }

    async fn run_qr_login(&self, login_id: String) {
        loop {
            let snapshot = {
                let logins = self.inner.logins.lock().await;
                let Some(session) = logins.get(&login_id) else { return; };
                if tokio::time::Instant::now() >= session.expires_at {
                    None
                } else {
                    Some(session.clone())
                }
            };
            let Some(snapshot) = snapshot else {
                self.finish_login(&login_id, "expired", Some("二维码已过期，请刷新".to_owned())).await;
                return;
            };
            let result: Result<QrStatusResponse, String> = self
                .get_qr_status(&snapshot.api_base_url, &snapshot.qrcode, snapshot.verify_code.as_deref())
                .await;
            match result {
                Ok(response) => match response.status.as_str() {
                    "wait" => {}
                    "scaned" => self.update_login(&login_id, |session| {
                        session.phase = "scanned".to_owned();
                        session.verify_code = None;
                    }).await,
                    "need_verifycode" => self.update_login(&login_id, |session| {
                        session.phase = "needs_verification".to_owned();
                        session.verify_code = None;
                        session.message = Some("请在手机微信上查看并输入验证码".to_owned());
                    }).await,
                    "scaned_but_redirect" => {
                        if let Some(host) = response.redirect_host.as_deref() {
                            if let Ok(base) = normalize_redirect_host(host) {
                                self.update_login(&login_id, |session| session.api_base_url = base).await;
                            } else {
                                self.finish_login(&login_id, "error", Some("iLink 返回了无效的登录地址".to_owned())).await;
                                return;
                            }
                        }
                    }
                    "expired" | "verify_code_blocked" => {
                        self.finish_login(&login_id, "expired", Some("二维码已过期或验证码验证次数过多，请刷新".to_owned())).await;
                        return;
                    }
                    "binded_redirect" => {
                        self.finish_login(&login_id, "error", Some("此 Bot 已绑定到其他客户端，iLink 未签发新的凭证".to_owned())).await;
                        return;
                    }
                    "confirmed" => {
                        let Some(token) = response.bot_token.filter(|token| !token.trim().is_empty() && token.len() <= 16 * 1024) else {
                            self.finish_login(&login_id, "error", Some("登录响应缺少有效凭证".to_owned())).await;
                            return;
                        };
                        let Some(base_url) = response.baseurl.as_deref().and_then(|base| validate_api_base_url(base).ok()) else {
                            self.finish_login(&login_id, "error", Some("登录响应包含不受信任的 iLink 地址".to_owned())).await;
                            return;
                        };
                        if let Err(error) = self.inner.token_store.set(&token) {
                            self.finish_login(&login_id, "error", Some(format!("无法安全保存微信凭证：{error}"))).await;
                            return;
                        }
                        if let Err(error) = self.update_state(|state| {
                            state.api_base_url = Some(base_url);
                            state.update_cursor.clear();
                            state.seen_message_ids.clear();
                            state.paused = false;
                        }) {
                            let _ = self.inner.token_store.delete();
                            self.finish_login(&login_id, "error", Some(error)).await;
                            return;
                        }
                        self.finish_login(&login_id, "connected", None).await;
                        self.ensure_poller();
                        return;
                    }
                    _ => {
                        self.finish_login(&login_id, "error", Some("iLink 返回了无法识别的登录状态".to_owned())).await;
                        return;
                    }
                },
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
    }

    async fn get_qr_status(
        &self,
        api_base_url: &str,
        qrcode: &str,
        verify_code: Option<&str>,
    ) -> Result<QrStatusResponse, String> {
        let mut url = url::Url::parse(&format!("{}/ilink/bot/get_qrcode_status", api_base_url.trim_end_matches('/')))
            .map_err(|_| "invalid iLink QR URL".to_owned())?;
        url.query_pairs_mut().append_pair("qrcode", qrcode);
        if let Some(code) = verify_code {
            url.query_pairs_mut().append_pair("verify_code", code);
        }
        self.authorized_builder(self.inner.client.get(url), None, HeaderMode::QrPoll)
            .timeout(Duration::from_secs(35))
            .send()
            .await
            .map_err(|_| "iLink QR status request failed".to_owned())?
            .error_for_status()
            .map_err(|_| "iLink QR status returned an HTTP error".to_owned())?
            .json()
            .await
            .map_err(|_| "iLink QR status response was invalid".to_owned())
    }

    async fn finish_login(&self, login_id: &str, phase: &str, message: Option<String>) {
        self.update_login(login_id, |session| {
            session.phase = phase.to_owned();
            session.message = message;
        }).await;
    }

    async fn update_login(&self, login_id: &str, update: impl FnOnce(&mut LoginSession)) {
        if let Some(session) = self.inner.logins.lock().await.get_mut(login_id) {
            update(session);
        }
    }

    pub async fn approve_sender(&self, sender_key_prefix: &str) -> Result<(), String> {
        self.update_sender(sender_key_prefix, |sender| sender.approved = true)?;
        Ok(())
    }

    pub async fn revoke_sender(&self, sender_key_prefix: &str) -> Result<(), String> {
        let state = self.load_state()?;
        let removed = find_sender_key(&state, sender_key_prefix)?;
        self.update_state(|state| { state.senders.remove(&removed); })?;
        if let Ok(mut replies) = self.inner.replies.lock() {
            replies.retain(|_, reply| reply.sender_key != removed);
        }
        Ok(())
    }

    fn update_sender(&self, sender_key_prefix: &str, update: impl FnOnce(&mut SenderRecord)) -> Result<(), String> {
        let mut state = self.load_state()?;
        let key = find_sender_key(&state, sender_key_prefix)?;
        let sender = state.senders.get_mut(&key).expect("sender key found");
        update(sender);
        self.store_state(&state)
    }

    pub async fn disconnect(&self) -> Result<WechatStatus, String> {
        self.stop_poller().await;
        self.inner.token_store.delete()?;
        self.update_state(|state| *state = PersistedState::default())?;
        if let Ok(mut replies) = self.inner.replies.lock() {
            replies.clear();
        }
        Ok(self.status())
    }

    pub fn restore_on_startup(&self) {
        if self.inner.token_store.get().ok().flatten().is_some() {
            self.ensure_poller();
        }
    }

    fn ensure_poller(&self) {
        let service = self.clone();
        let inner = self.inner.clone();
        tauri::async_runtime::spawn(async move {
            let mut current = inner.poller.lock().await;
            if current.as_ref().is_some_and(|token| !token.is_cancelled()) {
                return;
            }
            let cancel = CancellationToken::new();
            *current = Some(cancel.clone());
            drop(current);
            service.poll_loop(cancel).await;
        });
    }

    async fn stop_poller(&self) {
        let mut poller = self.inner.poller.lock().await;
        if let Some(cancel) = poller.take() {
            cancel.cancel();
        }
    }

    async fn poll_loop(&self, cancel: CancellationToken) {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let token = match self.inner.token_store.get() {
                Ok(Some(token)) => token,
                _ => return,
            };
            let state = match self.load_state() {
                Ok(state) => state,
                Err(_) => return,
            };
            if state.paused {
                return;
            }
            let Some(api_base_url) = state.api_base_url.as_deref() else { return; };
            let request = GetUpdatesRequest::new(state.update_cursor.clone());
            let call = self.post_json::<_, GetUpdatesResponse>(
                api_base_url,
                "ilink/bot/getupdates",
                &request,
                Some(&token),
                HeaderMode::Bot,
                Duration::from_secs(40),
            );
            let response = tokio::select! {
                _ = cancel.cancelled() => return,
                response = call => response,
            };
            match response {
                Ok(response) if response.ret.unwrap_or(0) == 0 && response.errcode.unwrap_or(0) == 0 => {
                    if let Err(_) = self.process_updates(response).await {
                        tokio::select! {
                            _ = cancel.cancelled() => return,
                            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                        }
                    }
                }
                Ok(response) if response.ret == Some(-14) || response.errcode == Some(-14) => {
                    let _ = self.update_state(|state| state.paused = true);
                    cancel.cancel();
                    return;
                }
                _ => {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    }
                }
            }
        }
    }

    async fn process_updates(&self, response: GetUpdatesResponse) -> Result<(), String> {
        let token = self.inner.token_store.get()?.ok_or_else(|| "WeChat account disconnected".to_owned())?;
        let state_snapshot = self.load_state()?;
        let api_base_url = state_snapshot.api_base_url.clone().ok_or_else(|| "iLink API URL is missing".to_owned())?;
        for message in response.msgs.unwrap_or_default() {
            if let Some(id) = message.message_id {
                if state_snapshot.seen_message_ids.contains(&id) { continue; }
            }
            if let Some(text) = message.direct_text() {
                self.process_direct_message(&api_base_url, &token, &message, &text).await?;
            }
            if let Some(id) = message.message_id {
                self.remember_message_id(id)?;
            }
        }
        if let Some(cursor) = response.get_updates_buf.filter(|cursor| !cursor.is_empty()) {
            self.update_state(|state| state.update_cursor = cursor)?;
        }
        Ok(())
    }

    async fn process_direct_message(
        &self,
        api_base_url: &str,
        token: &str,
        message: &IlinkMessage,
        text: &str,
    ) -> Result<(), String> {
        let peer_id = message.from_user_id.as_deref().ok_or_else(|| "missing sender".to_owned())?;
        let sender_key = sender_key(peer_id);
        let state = self.load_state()?;
        let Some(sender) = state.senders.get(&sender_key) else {
            self.update_state(|state| {
                state.senders.entry(sender_key).or_insert(SenderRecord { approved: false, thread_id: None });
            })?;
            return Ok(());
        };
        if !sender.approved { return Ok(()); }

        let app_state = self.inner.app.try_state::<crate::app_state::AppState>()
            .ok_or_else(|| "k-Coder runtime unavailable".to_owned())?;
        let thread_id = if let Some(thread_id) = sender.thread_id {
            thread_id
        } else {
            let workspace = app_state.workspace_root();
            let canonical = workspace.canonicalize().map_err(|_| "active workspace is unavailable".to_owned())?;
            let thread = app_state.repository().create_thread_in_workspace(&canonical).await
                .map_err(|_| "could not create the WeChat conversation".to_owned())?;
            self.update_state(|state| {
                if let Some(record) = state.senders.get_mut(&sender_key) {
                    if record.approved { record.thread_id = Some(thread.id.clone()); }
                }
            })?;
            thread.id
        };
        let current_workspace = app_state.workspace_root();
        let workspace = app_state.ensure_thread_workspace(&thread_id).await
            .map_err(|_| "the WeChat conversation's project is not the active workspace; switch back to its project and resend".to_owned())?;
        if workspace != current_workspace {
            return Err("the WeChat conversation's project is not the active workspace; switch back to its project and resend".to_owned());
        }

        let context_token = message.context_token.clone().ok_or_else(|| "missing reply context".to_owned())?;
        let turn_id = Uuid::new_v4().to_string();
        let reply = ReplyContext { sender_key: sender_key.clone(), peer_id: peer_id.to_owned(), context_token };
        self.inner.replies.lock().map_err(|_| "reply context lock poisoned".to_owned())?
            .insert((thread_id.clone(), turn_id.clone()), reply);
        let result: Result<TurnHandle, crate::commands::CommandError> = crate::commands::enqueue_message_turn_with_id(
            self.inner.app.clone(),
            app_state.inner(),
            RunTurnRequest { thread_id: thread_id.clone(), input: text.to_owned(), agent_mode: None },
            Vec::<ImageAttachment>::new(),
            None,
            turn_id.clone(),
        ).await;
        if result.is_err() {
            if let Ok(mut replies) = self.inner.replies.lock() {
                replies.remove(&(thread_id, turn_id));
            }
            let _ = self.send_text(api_base_url, token, peer_id, message.context_token.as_deref().unwrap_or(""), "k-Coder 当前无法接收这条消息，请检查桌面端状态后重试").await;
        }
        Ok(())
    }

    pub fn on_agent_event(&self, envelope: &AgentEventEnvelope) {
        let (thread_id, turn_id) = match &envelope.event {
            AgentEvent::TurnCompleted { thread_id, turn_id, .. }
            | AgentEvent::TurnFailed { thread_id, turn_id, .. }
            | AgentEvent::TurnCancelled { thread_id, turn_id, .. } => (thread_id, turn_id),
            _ => return,
        };
        let reply = self.inner.replies.lock().ok().and_then(|mut replies| replies.remove(&(thread_id.clone(), turn_id.clone())));
        let Some(reply) = reply else { return; };
        let message = match &envelope.event {
            AgentEvent::TurnCompleted { message, .. } => {
                let text = message.visible_text();
                if text.trim().is_empty() { "k-Coder 已完成任务；请在桌面端查看结果。".to_owned() } else { text }
            }
            AgentEvent::TurnFailed { .. } => "k-Coder 处理失败，请在桌面端查看详情。".to_owned(),
            AgentEvent::TurnCancelled { .. } => "k-Coder 任务已停止。".to_owned(),
            _ => unreachable!(),
        };
        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            let Ok(state) = service.load_state() else { return; };
            if !state.senders.get(&reply.sender_key).is_some_and(|sender| sender.approved) { return; }
            let Ok(Some(token)) = service.inner.token_store.get() else { return; };
            let Some(base) = state.api_base_url.as_deref() else { return; };
            let _ = service.send_text(base, &token, &reply.peer_id, &reply.context_token, &message).await;
        });
    }

    async fn send_text(&self, base_url: &str, token: &str, peer_id: &str, context_token: &str, text: &str) -> Result<(), String> {
        for chunk in split_reply(text, MAX_REPLY_CHUNK_BYTES) {
            if chunk.trim().is_empty() { continue; }
            let request = SendMessageRequest::text(peer_id, context_token, &Uuid::new_v4().to_string(), &chunk);
            let response: SendMessageResponse = self.post_json(
                base_url, "ilink/bot/sendmessage", &request, Some(token), HeaderMode::Bot, Duration::from_secs(15),
            ).await?;
            if response.ret.is_some_and(|ret| ret != 0) {
                return Err("iLink rejected the response".to_owned());
            }
        }
        Ok(())
    }

    async fn post_json<T: Serialize, U: for<'de> Deserialize<'de>>(
        &self,
        base_url: &str,
        endpoint: &str,
        payload: &T,
        token: Option<&str>,
        header_mode: HeaderMode,
        timeout: Duration,
    ) -> Result<U, String> {
        let base_url = validate_api_base_url(base_url)?;
        let url = format!("{}/{}", base_url.trim_end_matches('/'), endpoint.trim_start_matches('/'));
        let response = self.authorized_builder(self.inner.client.post(url).json(payload), token, header_mode)
            .timeout(timeout)
            .send().await.map_err(|_| "iLink request failed".to_owned())?
            .error_for_status().map_err(|_| "iLink returned an HTTP error".to_owned())?;
        response.json().await.map_err(|_| "iLink response was invalid".to_owned())
    }

    fn authorized_builder(&self, mut request: reqwest::RequestBuilder, token: Option<&str>, mode: HeaderMode) -> reqwest::RequestBuilder {
        request = request.header("iLink-App-Id", "bot")
            .header("iLink-App-ClientVersion", client_version());
        if matches!(mode, HeaderMode::QrStart | HeaderMode::Bot) {
            let raw = Uuid::new_v4();
            let uin = u32::from_be_bytes(raw.as_bytes()[..4].try_into().expect("UUID has 16 bytes"));
            request = request.header("X-WECHAT-UIN", base64::engine::general_purpose::STANDARD.encode(uin.to_string()))
                .header("AuthorizationType", "ilink_bot_token");
        }
        if matches!(mode, HeaderMode::Bot) {
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
        }
        request
    }

    fn load_state(&self) -> Result<PersistedState, String> {
        self.inner.state.lock().map(|state| state.clone()).map_err(|_| "WeChat state lock poisoned".to_owned())
    }

    fn update_state(&self, update: impl FnOnce(&mut PersistedState)) -> Result<(), String> {
        let mut state = self.inner.state.lock().map_err(|_| "WeChat state lock poisoned".to_owned())?;
        let mut next = state.clone();
        update(&mut next);
        self.store_state(&next)?;
        *state = next;
        Ok(())
    }

    fn store_state(&self, state: &PersistedState) -> Result<(), String> {
        let serialized = serde_json::to_string(state).map_err(|_| "could not serialize WeChat state".to_owned())?;
        self.inner.projection.set_setting(SETTINGS_KEY, &serialized).map_err(|error| error.to_string())
    }

    fn remember_message_id(&self, id: u64) -> Result<(), String> {
        self.update_state(|state| {
            state.seen_message_ids.push_back(id);
            while state.seen_message_ids.len() > MAX_SEEN_MESSAGE_IDS {
                state.seen_message_ids.pop_front();
            }
        })
    }
}

fn sender_key(peer_id: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(peer_id.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn find_sender_key(state: &PersistedState, prefix: &str) -> Result<String, String> {
    let matches = state.senders.keys().filter(|key| key.starts_with(prefix)).cloned().collect::<Vec<_>>();
    match matches.as_slice() {
        [key] => Ok(key.clone()),
        [] => Err("WeChat sender was not found".to_owned()),
        _ => Err("WeChat sender key is ambiguous".to_owned()),
    }
}

fn normalize_redirect_host(host: &str) -> Result<String, String> {
    let value = if host.contains("://") { host.to_owned() } else { format!("https://{host}") };
    validate_api_base_url(&value)
}

fn client_version() -> String {
    let mut pieces = env!("CARGO_PKG_VERSION").split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    let (major, minor, patch) = (pieces.next().unwrap_or(0), pieces.next().unwrap_or(0), pieces.next().unwrap_or(0));
    ((major << 16) | (minor << 8) | patch).to_string()
}
