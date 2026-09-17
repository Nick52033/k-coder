//! JSON-RPC 方法分发。
//!
//! 网关是「轻量 Tauri 边界」的延伸：它只做协议、身份、能力裁剪和载荷投影，
//! 所有状态变更都调用现有应用服务（`AppState` / `commands::enqueue_message_turn`），
//! 不复制智能体循环，也不直接访问 Provider、SQLite 或工作区文件。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::{Value, json};

use crate::agent::RunTurnRequest;
use crate::app_state::{AppState, AppStateError};
use crate::protocol::{
    AgentEvent, AgentEventEnvelope, ApprovalAction, ApprovalResolution, ExpectedFileHash,
    PatchPreview, UserInputAction, UserInputAnswer, UserInputResolution,
};

use super::auth::{DeviceRegistry, PairingStore, TokenStore, now_ms};
use super::capability::CapabilityPolicy;
use super::events::EventHub;
use super::host::GatewayHost;
use super::protocol::{
    EVENT_PROTOCOL_VERSION, MOBILE_PROTOCOL_VERSION, MobileError, MobileErrorKind, RpcRequest,
    RpcResponse,
};
use super::{projects, view};

/// 幂等缓存与待处理请求索引的容量上限。
pub const MAX_TRACKED_REQUESTS: usize = 256;

/// 网关共享上下文。所有字段都是进程内共享的句柄，可以廉价克隆。
#[derive(Clone)]
pub struct GatewayContext {
    /// 宿主应用边界。网关只通过它拿应用状态、写日志和启动 Turn。
    pub host: Arc<dyn GatewayHost>,
    pub registry: Arc<DeviceRegistry>,
    pub tokens: Arc<TokenStore>,
    pub pairing: Arc<PairingStore>,
    pub policy: Arc<CapabilityPolicy>,
    pub hub: Arc<EventHub>,
    pub pending: Arc<PendingRequestIndex>,
    pub idempotency: Arc<IdempotencyCache>,
    pub fingerprint: Arc<RwLock<Option<String>>>,
}

impl GatewayContext {
    pub fn app_state(&self) -> Result<&AppState, MobileError> {
        self.host
            .app_state()
            .ok_or_else(|| MobileError::internal("application state is unavailable"))
    }
}

/// 单条连接的会话状态。由 `server` 持有，`rpc` 只读取和推进状态机。
#[derive(Debug, Clone, Default)]
pub struct RpcSession {
    pub connection_id: u64,
    pub peer: String,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub client_name: Option<String>,
    pub initialized: bool,
    pub ready: bool,
    pub negotiated_version: u32,
}

/// 待处理审批的服务器侧元数据。
///
/// 手机端只拿到工具名、风险和路径；补丁正文留在这里，仅用于在用户点「批准」时
/// 复用桌面端相同的 `selectedPaths` / `expectedHashes` 语义。
#[derive(Debug, Clone)]
pub struct PendingApprovalMeta {
    pub thread_id: String,
    pub tool_call_id: String,
    pub created_at_ms: u64,
    pub tool_name: String,
    pub preview: Option<PatchPreview>,
}

#[derive(Debug, Clone)]
pub struct PendingUserInputMeta {
    pub thread_id: String,
    pub created_at_ms: u64,
}

/// 从领域事件中维护「当前还挂着的审批 / 提问」索引。
///
/// 手机端需要 `toolCallId` 和版本回显来识别过期请求，而 `ApprovalManager`
/// 只提供 resolve，因此这里保留一份最小元数据。
#[derive(Debug, Default)]
pub struct PendingRequestIndex {
    approvals: Mutex<HashMap<String, PendingApprovalMeta>>,
    user_inputs: Mutex<HashMap<String, PendingUserInputMeta>>,
}

impl PendingRequestIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&self, envelope: &AgentEventEnvelope) {
        match &envelope.event {
            AgentEvent::ApprovalRequested {
                thread_id,
                request,
                turn_id: _,
            } => {
                let mut approvals = self.approvals.lock().expect("pending lock poisoned");
                approvals.insert(
                    request.id.clone(),
                    PendingApprovalMeta {
                        thread_id: thread_id.clone(),
                        tool_call_id: request.tool_call_id.clone(),
                        created_at_ms: request.created_at_ms,
                        tool_name: request.tool_name.clone(),
                        preview: request.preview.clone(),
                    },
                );
                trim_map(&mut approvals, |meta| meta.created_at_ms);
            }
            AgentEvent::ApprovalResolved { request_id, .. } => {
                self.approvals
                    .lock()
                    .expect("pending lock poisoned")
                    .remove(request_id);
            }
            AgentEvent::UserInputRequested {
                thread_id,
                request,
                turn_id: _,
            } => {
                let mut user_inputs = self.user_inputs.lock().expect("pending lock poisoned");
                user_inputs.insert(
                    request.id.clone(),
                    PendingUserInputMeta {
                        thread_id: thread_id.clone(),
                        created_at_ms: request.created_at_ms,
                    },
                );
                trim_map(&mut user_inputs, |meta| meta.created_at_ms);
            }
            AgentEvent::UserInputResolved { request_id, .. } => {
                self.user_inputs
                    .lock()
                    .expect("pending lock poisoned")
                    .remove(request_id);
            }
            _ => {}
        }
    }

    pub fn approval(&self, request_id: &str) -> Option<PendingApprovalMeta> {
        self.approvals
            .lock()
            .expect("pending lock poisoned")
            .get(request_id)
            .cloned()
    }

    pub fn user_input(&self, request_id: &str) -> Option<PendingUserInputMeta> {
        self.user_inputs
            .lock()
            .expect("pending lock poisoned")
            .get(request_id)
            .cloned()
    }

    pub fn resolve_approval(&self, request_id: &str) {
        self.approvals
            .lock()
            .expect("pending lock poisoned")
            .remove(request_id);
    }

    pub fn resolve_user_input(&self, request_id: &str) {
        self.user_inputs
            .lock()
            .expect("pending lock poisoned")
            .remove(request_id);
    }

    pub fn pending_approval_count(&self, thread_id: &str) -> u32 {
        self.approvals
            .lock()
            .expect("pending lock poisoned")
            .values()
            .filter(|meta| meta.thread_id == thread_id)
            .count() as u32
    }

    pub fn pending_user_input_count(&self, thread_id: &str) -> u32 {
        self.user_inputs
            .lock()
            .expect("pending lock poisoned")
            .values()
            .filter(|meta| meta.thread_id == thread_id)
            .count() as u32
    }
}

fn trim_map<K, V>(map: &mut HashMap<K, V>, created_at: impl Fn(&V) -> u64)
where
    K: Clone + Eq + std::hash::Hash,
{
    while map.len() > MAX_TRACKED_REQUESTS {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, value)| created_at(value))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        map.remove(&oldest);
    }
}

/// 写操作幂等缓存：按「设备 + 方法 + requestId」去重。
///
/// 移动网络重试不能造成重复 Turn，因此命中缓存时直接回放上一次结果。
#[derive(Debug, Default)]
pub struct IdempotencyCache {
    entries: Mutex<VecDeque<(String, Value)>>,
}

impl IdempotencyCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn key(device_id: &str, method: &str, request_id: &str) -> String {
        format!("{device_id}:{method}:{request_id}")
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.entries
            .lock()
            .expect("idempotency lock poisoned")
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.clone())
    }

    pub fn record(&self, key: &str, value: Value) {
        let mut entries = self.entries.lock().expect("idempotency lock poisoned");
        if entries.iter().any(|(candidate, _)| candidate == key) {
            return;
        }
        entries.push_back((key.to_string(), value));
        while entries.len() > MAX_TRACKED_REQUESTS {
            entries.pop_front();
        }
    }
}

fn require_string(
    params: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String, MobileError> {
    let value = params
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            MobileError::invalid_params(format!("{field} must be a non-empty string"))
        })?;
    Ok(value.to_string())
}

fn optional_string(params: &serde_json::Map<String, Value>, field: &str) -> Option<String> {
    params
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 现有应用状态错误的移动端映射。
///
/// 含绝对路径的变体一律替换为不含路径的说明，避免把工作区位置带到手机上。
pub fn map_state_error(error: AppStateError) -> MobileError {
    match error {
        AppStateError::NoActiveTurn(_) => {
            MobileError::stale_request("there is no active turn for this thread")
        }
        AppStateError::ExpectedTurnMismatch { .. } => {
            MobileError::stale_request("the active turn changed since this request was created")
        }
        AppStateError::QueuedTurnNotFound { .. } | AppStateError::QueuedTurnNotMessage { .. } => {
            MobileError::stale_request("the queued turn is no longer available")
        }
        AppStateError::ThreadWorkspaceMismatch { .. } => MobileError::forbidden(
            "this thread belongs to a different workspace than the active one",
        ),
        AppStateError::ThreadHasNoWorkspace(_) => {
            MobileError::forbidden("this thread is not associated with a project workspace")
        }
        AppStateError::ThreadOperationBusy(_) => MobileError::new(
            MobileErrorKind::ServerOverloaded,
            "the thread is busy with another operation",
        ),
        AppStateError::Storage(crate::storage::StorageError::NotFound(_)) => {
            MobileError::not_found("thread was not found")
        }
        other => MobileError::internal(other.to_string()),
    }
}

fn ensure_device(ctx: &GatewayContext, session: &RpcSession) -> Result<String, MobileError> {
    let device_id = session
        .device_id
        .clone()
        .ok_or_else(|| MobileError::unauthorized("connection is not authenticated"))?;
    let device = ctx
        .registry
        .device(&device_id)
        .ok_or_else(|| MobileError::unauthorized("device is no longer registered"))?;
    if !device.is_active() {
        return Err(MobileError::unauthorized("device was revoked"));
    }
    Ok(device_id)
}

/// 处理一条入站 JSON-RPC 消息。
///
/// 返回 `None` 表示这是通知，服务端不回包。
pub async fn dispatch(
    ctx: &GatewayContext,
    session: &Arc<Mutex<RpcSession>>,
    request: RpcRequest,
) -> Option<RpcResponse> {
    let id = request.id.clone();
    let response = match handle(ctx, session, &request).await {
        Ok(Some(result)) => Some(RpcResponse::success(
            id.clone().unwrap_or(Value::Null),
            result,
        )),
        Ok(None) => None,
        Err(error) => Some(RpcResponse::failure(
            id.clone().unwrap_or(Value::Null),
            error,
        )),
    };
    // 通知永远不回包，即使处理失败。
    if request.is_notification() {
        None
    } else {
        response
    }
}

async fn handle(
    ctx: &GatewayContext,
    session: &Arc<Mutex<RpcSession>>,
    request: &RpcRequest,
) -> Result<Option<Value>, MobileError> {
    if request.jsonrpc.as_deref() != Some("2.0") {
        return Err(MobileError::new(
            MobileErrorKind::InvalidRequest,
            "jsonrpc must be \"2.0\"",
        ));
    }
    let params = request.params_object()?;
    let method = request.method.as_str();

    match method {
        "initialize" => return handle_initialize(ctx, session, &params).await.map(Some),
        "initialized" => {
            return handle_initialized(session).map(|_| Some(json!({ "ready": true })));
        }
        "auth/refresh" => return handle_auth_refresh(ctx, session, &params).await.map(Some),
        _ => {}
    }

    {
        let session = session.lock().expect("session lock poisoned");
        if !session.initialized {
            return Err(MobileError::unauthorized(
                "connection must call initialize before any other method",
            ));
        }
        if !session.ready {
            return Err(MobileError::unauthorized(
                "connection must send initialized before any other method",
            ));
        }
    }

    if let Some(result) = ctx.policy.authorize(method) {
        result?;
    }

    let device_id = {
        let session = session.lock().expect("session lock poisoned");
        ensure_device(ctx, &session)?
    };
    let connection_id = session.lock().expect("session lock poisoned").connection_id;

    match method {
        "ping" => Ok(Some(json!({ "serverTimeMs": now_ms() }))),
        "project/list" => handle_project_list(ctx, &device_id).await.map(Some),
        "thread/list" => handle_thread_list(ctx, &device_id).await.map(Some),
        "thread/read" => handle_thread_read(ctx, &params).await.map(Some),
        "thread/subscribe" => handle_thread_subscribe(ctx, connection_id, &params)
            .await
            .map(Some),
        "thread/unsubscribe" => handle_thread_unsubscribe(ctx, connection_id, &params)
            .await
            .map(Some),
        "turn/start" => handle_turn_start(ctx, &device_id, &params).await.map(Some),
        "turn/steer" => handle_turn_steer(ctx, &device_id, &params).await.map(Some),
        "turn/interrupt" => handle_turn_interrupt(ctx, &device_id, &params)
            .await
            .map(Some),
        "approval/respond" => handle_approval_respond(ctx, &params).await.map(Some),
        "tool/requestUserInput/respond" => handle_user_input_respond(ctx, &params).await.map(Some),
        "events/resume" => handle_events_resume(ctx, connection_id, &params).map(Some),
        other => Err(MobileError::new(
            MobileErrorKind::MethodNotFound,
            format!("unknown method: {other}"),
        )),
    }
}

async fn handle_initialize(
    ctx: &GatewayContext,
    session: &Arc<Mutex<RpcSession>>,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    {
        let session = session.lock().expect("session lock poisoned");
        if session.initialized {
            return Err(MobileError::new(
                MobileErrorKind::InvalidRequest,
                "connection is already initialized",
            ));
        }
    }

    let client_version = params
        .get("protocolVersion")
        .and_then(Value::as_u64)
        .unwrap_or(MOBILE_PROTOCOL_VERSION as u64);
    if client_version > MOBILE_PROTOCOL_VERSION as u64 {
        return Err(MobileError::unsupported_capability("protocolVersion")
            .with_details(json!({ "supportedVersion": MOBILE_PROTOCOL_VERSION })));
    }
    let client_name = params
        .get("client")
        .and_then(|client| client.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);

    // 身份来自服务端登记的设备，客户端声明的能力只用于显示协商。
    let device_id = match optional_string(params, "accessToken") {
        Some(token) => ctx.tokens.resolve(&token, &ctx.registry)?,
        None => {
            let device_id = require_string(params, "deviceId")?;
            let device_secret = require_string(params, "deviceSecret")?;
            ctx.registry.authenticate(&device_id, &device_secret)?.id
        }
    };
    let device = ctx
        .registry
        .device(&device_id)
        .ok_or_else(|| MobileError::unauthorized("device is no longer registered"))?;
    if !device.is_active() {
        return Err(MobileError::unauthorized("device was revoked"));
    }

    let (access_token, expires_at_ms) = ctx.tokens.issue(&device_id);
    ctx.registry.touch(&device_id);

    {
        let mut session = session.lock().expect("session lock poisoned");
        session.initialized = true;
        session.ready = false;
        session.negotiated_version = MOBILE_PROTOCOL_VERSION;
        session.device_id = Some(device_id.clone());
        session.device_name = Some(device.name.clone());
        session.client_name = client_name;
    }

    Ok(json!({
        "protocolVersion": MOBILE_PROTOCOL_VERSION,
        "eventProtocolVersion": EVENT_PROTOCOL_VERSION,
        "sessionId": uuid::Uuid::new_v4().to_string(),
        "server": {
            "name": "k-coder",
            "version": env!("CARGO_PKG_VERSION"),
            "fingerprint": ctx
                .fingerprint
                .read()
                .expect("fingerprint lock poisoned")
                .clone(),
        },
        "device": { "id": device.id, "name": device.name },
        "capabilities": ctx.policy.granted(),
        "accessToken": access_token,
        "accessTokenExpiresAtMs": expires_at_ms,
    }))
}

fn handle_initialized(session: &Arc<Mutex<RpcSession>>) -> Result<(), MobileError> {
    let mut session = session.lock().expect("session lock poisoned");
    if !session.initialized {
        return Err(MobileError::unauthorized(
            "connection must call initialize first",
        ));
    }
    session.ready = true;
    Ok(())
}

async fn handle_auth_refresh(
    ctx: &GatewayContext,
    session: &Arc<Mutex<RpcSession>>,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    let device_id = require_string(params, "deviceId")?;
    let refresh_token = require_string(params, "refreshToken")?;
    let (device, credentials) = ctx.registry.refresh(&device_id, &refresh_token)?;
    ctx.tokens.revoke_device(&device.id);
    let (access_token, expires_at_ms) = ctx.tokens.issue(&device.id);

    let mut session = session.lock().expect("session lock poisoned");
    session.device_id = Some(device.id.clone());
    session.device_name = Some(device.name.clone());
    drop(session);

    Ok(json!({
        "deviceId": device.id,
        "accessToken": access_token,
        "accessTokenExpiresAtMs": expires_at_ms,
        "refreshToken": credentials.refresh_token,
        "capabilities": ctx.policy.granted(),
    }))
}

/// 项目清单。手机端据此渲染项目分组，包括**尚无会话**的项目——这正是此前手机端
/// 只能看到 k-coder 一个分组的原因（归属事实只存在于会话表里）。
///
/// 与 `thread/list` 分开发方法而不是揉进同一个响应：项目的生命周期比会话慢得多，
/// 手机端只需要在进入列表页时拉一次，之后刷新会话就能保持分组不跳动。
async fn handle_project_list(ctx: &GatewayContext, device_id: &str) -> Result<Value, MobileError> {
    let state = ctx.app_state()?;
    let active = state.workspace_root();
    let attribution = projects::resolve(state, active.to_str())
        .await
        .map_err(|error| MobileError::internal(error.to_string()))?;
    ctx.registry.touch(device_id);
    Ok(json!({
        "projects": attribution
            .projects
            .iter()
            .map(|project| view::MobileProject {
                id: project.id.clone(),
                name: project.name.clone(),
                key: project.key.clone(),
                last_opened_at_ms: project.last_opened_at_ms,
            })
            .collect::<Vec<_>>(),
    }))
}

async fn handle_thread_list(ctx: &GatewayContext, device_id: &str) -> Result<Value, MobileError> {
    let state = ctx.app_state()?;
    let active = state.workspace_root();
    let attribution = projects::resolve(state, active.to_str())
        .await
        .map_err(|error| MobileError::internal(error.to_string()))?;

    let threads = state
        .list_conversation_threads("")
        .await
        .map_err(|error| MobileError::internal(error.to_string()))?;
    let mut projected = Vec::with_capacity(threads.len());
    for summary in threads {
        let active_turn_id = state.active_turn_id(&summary.id).await;
        projected.push(view::MobileThreadSummary::from_summary(
            &summary,
            attribution.project_key_of(&summary.id),
            active_turn_id,
            ctx.pending.pending_approval_count(&summary.id),
            ctx.pending.pending_user_input_count(&summary.id),
        ));
    }
    ctx.registry.touch(device_id);
    Ok(json!({ "threads": projected }))
}

async fn handle_thread_read(
    ctx: &GatewayContext,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    let thread_id = require_string(params, "threadId")?;
    let state = ctx.app_state()?;
    let snapshot = state
        .read_thread_history(&thread_id)
        .await
        .map_err(map_state_error)?;

    let turns: Vec<view::MobileTurn> = snapshot.turns.data.iter().map(view::project_turn).collect();
    let mut pending_approvals = Vec::new();
    let mut pending_user_inputs = Vec::new();
    for turn in &turns {
        for item in &turn.items {
            if let Some(approval) = &item.approval {
                if !approval.resolved {
                    pending_approvals.push(approval.clone());
                }
            }
            if let Some(user_input) = &item.user_input {
                if !user_input.resolved {
                    pending_user_inputs.push(user_input.clone());
                }
            }
        }
    }

    let active_turn_id = state.active_turn_id(&thread_id).await;
    let view = view::MobileThreadView {
        thread_id: thread_id.clone(),
        title: snapshot.summary.title.clone(),
        updated_at_ms: snapshot.summary.updated_at_ms,
        running: active_turn_id.is_some(),
        active_turn_id,
        turns,
        todos: snapshot
            .todos
            .iter()
            .map(|todo| view::MobileTodo {
                content: todo.content.clone(),
                status: todo.status,
            })
            .collect(),
        pending_approvals,
        pending_user_inputs,
    };

    Ok(json!({
        "thread": view,
        "nextCursor": snapshot.turns.next_cursor,
    }))
}

async fn handle_thread_subscribe(
    ctx: &GatewayContext,
    connection_id: u64,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    let thread_id = require_string(params, "threadId")?;
    ensure_thread_accessible(ctx, &thread_id).await?;
    let delivery_seq = ctx.hub.subscribe(connection_id, &thread_id)?;
    Ok(json!({ "subscribed": true, "deliverySeq": delivery_seq }))
}

async fn handle_thread_unsubscribe(
    ctx: &GatewayContext,
    connection_id: u64,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    let thread_id = require_string(params, "threadId")?;
    ctx.hub.unsubscribe(connection_id, &thread_id);
    Ok(json!({ "subscribed": false }))
}

/// 订阅前的同步可见性检查：会话必须存在，且绑定在当前工作区。
async fn ensure_thread_accessible(
    ctx: &GatewayContext,
    thread_id: &str,
) -> Result<(), MobileError> {
    let state = ctx.app_state()?;
    state
        .resolve_thread_workspace(thread_id)
        .await
        .map_err(map_state_error)?;
    Ok(())
}

async fn handle_turn_start(
    ctx: &GatewayContext,
    device_id: &str,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    let thread_id = require_string(params, "threadId")?;
    let input = require_string(params, "input")?;
    let request_id = require_string(params, "requestId")?;
    let key = IdempotencyCache::key(device_id, "turn/start", &request_id);
    if let Some(cached) = ctx.idempotency.get(&key) {
        return Ok(cached);
    }

    let state = ctx.app_state()?;
    state
        .resolve_thread_workspace(&thread_id)
        .await
        .map_err(map_state_error)?;

    let handle = ctx
        .host
        .start_turn(
            state,
            RunTurnRequest {
                thread_id: thread_id.clone(),
                input,
                agent_mode: None,
            },
            Vec::new(),
            None,
        )
        .await?;

    let result = json!({
        "threadId": handle.thread_id,
        "turnId": handle.turn_id,
        "state": handle.state,
    });
    ctx.idempotency.record(&key, result.clone());
    Ok(result)
}

async fn handle_turn_steer(
    ctx: &GatewayContext,
    device_id: &str,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    let thread_id = require_string(params, "threadId")?;
    let expected_turn_id = require_string(params, "expectedTurnId")?;
    let input = require_string(params, "input")?;
    let request_id = require_string(params, "requestId")?;
    let key = IdempotencyCache::key(device_id, "turn/steer", &request_id);
    if let Some(cached) = ctx.idempotency.get(&key) {
        return Ok(cached);
    }

    let state = ctx.app_state()?;
    let active_turn_id = state
        .active_turn_id(&thread_id)
        .await
        .ok_or_else(|| MobileError::stale_request("there is no active turn for this thread"))?;
    if active_turn_id != expected_turn_id {
        return Err(MobileError::stale_request(
            "the active turn changed since this request was created",
        ));
    }

    // 首期移动端不携带附件，因此直接构造纯文本用户消息；附件路径仍由桌面端负责。
    let message = crate::protocol::ChatMessage {
        schema_version: crate::protocol::PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4().to_string(),
        role: crate::protocol::MessageRole::User,
        content: vec![crate::protocol::ContentBlock::Text { text: input }],
        created_at_ms: now_ms(),
    };
    let turn_id = state
        .steer_turn(&thread_id, &expected_turn_id, message)
        .await
        .map_err(map_state_error)?;

    let result = json!({ "threadId": thread_id, "turnId": turn_id });
    ctx.idempotency.record(&key, result.clone());
    Ok(result)
}

async fn handle_turn_interrupt(
    ctx: &GatewayContext,
    device_id: &str,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    let thread_id = require_string(params, "threadId")?;
    let turn_id = require_string(params, "turnId")?;
    let request_id = require_string(params, "requestId")?;
    let key = IdempotencyCache::key(device_id, "turn/interrupt", &request_id);
    if let Some(cached) = ctx.idempotency.get(&key) {
        return Ok(cached);
    }

    let state = ctx.app_state()?;
    state
        .interrupt_turn(&thread_id, &turn_id)
        .await
        .map_err(map_state_error)?;

    let result = json!({ "threadId": thread_id, "turnId": turn_id, "interrupted": true });
    ctx.idempotency.record(&key, result.clone());
    Ok(result)
}

async fn handle_approval_respond(
    ctx: &GatewayContext,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    let request_id = require_string(params, "requestId")?;
    let action = match require_string(params, "action")?.as_str() {
        "approved" => ApprovalAction::Approved,
        "rejected" => ApprovalAction::Rejected,
        "cancelled" => ApprovalAction::Cancelled,
        other => {
            return Err(MobileError::invalid_params(format!(
                "unsupported approval action: {other}"
            )));
        }
    };

    let meta = ctx.pending.approval(&request_id);
    // 版本回显校验：请求已被处理或已超时，客户端看到的版本就过期了。
    if let Some(meta) = meta.as_ref() {
        if let Some(tool_call_id) = optional_string(params, "toolCallId") {
            if meta.tool_call_id != tool_call_id {
                return Err(MobileError::stale_request(
                    "approval request does not match the observed tool call",
                ));
            }
        }
        if let Some(expected) = params.get("expectedCreatedAtMs").and_then(Value::as_u64) {
            if meta.created_at_ms != expected {
                return Err(MobileError::stale_request(
                    "approval request was replaced by a newer one",
                ));
            }
        }
    }

    // 批准时复用桌面端语义：默认选中预览里的全部文件，并回传对应哈希。
    let (patch, selected_paths, expected_hashes) = match (action, meta.as_ref()) {
        (ApprovalAction::Approved, Some(meta)) => match meta.preview.as_ref() {
            Some(preview) => (
                if meta.tool_name == "apply_patch" {
                    Some(preview.patch.clone())
                } else {
                    None
                },
                preview.files.iter().map(|file| file.path.clone()).collect(),
                preview
                    .files
                    .iter()
                    .map(|file| ExpectedFileHash {
                        path: file.path.clone(),
                        before_hash: file.before_hash.clone(),
                    })
                    .collect(),
            ),
            None => (None, Vec::new(), Vec::new()),
        },
        _ => (None, Vec::new(), Vec::new()),
    };

    let state = ctx.app_state()?;
    state
        .approvals()
        .resolve(
            &request_id,
            ApprovalResolution {
                action,
                patch,
                selected_paths,
                expected_hashes,
            },
        )
        .await
        .map_err(|error| match error {
            crate::policy::ApprovalError::NotFound(_) => {
                MobileError::stale_request("approval request is no longer pending")
            }
            other => MobileError::internal(other.to_string()),
        })?;
    ctx.pending.resolve_approval(&request_id);
    Ok(json!({ "requestId": request_id, "resolved": true }))
}

async fn handle_user_input_respond(
    ctx: &GatewayContext,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    let request_id = require_string(params, "requestId")?;
    let action = match require_string(params, "action")?.as_str() {
        "answered" => UserInputAction::Answered,
        "skipped" => UserInputAction::Skipped,
        "cancelled" => UserInputAction::Cancelled,
        other => {
            return Err(MobileError::invalid_params(format!(
                "unsupported user input action: {other}"
            )));
        }
    };

    if let Some(meta) = ctx.pending.user_input(&request_id) {
        if let Some(expected) = params.get("expectedCreatedAtMs").and_then(Value::as_u64) {
            if meta.created_at_ms != expected {
                return Err(MobileError::stale_request(
                    "user input request was replaced by a newer one",
                ));
            }
        }
    }

    let answers: Vec<UserInputAnswer> = params
        .get("answers")
        .and_then(Value::as_array)
        .map(|answers| {
            answers
                .iter()
                .filter_map(|answer| {
                    let question = answer.get("question").and_then(Value::as_str)?;
                    let value = answer
                        .get("answer")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Some(UserInputAnswer {
                        question: question.to_string(),
                        answer: value.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if matches!(action, UserInputAction::Answered) && answers.is_empty() {
        return Err(MobileError::invalid_params(
            "answered requires at least one answer",
        ));
    }

    let state = ctx.app_state()?;
    state
        .user_inputs()
        .resolve(&request_id, UserInputResolution { action, answers })
        .await
        .map_err(|error| match error {
            crate::policy::UserInputError::NotFound(_) => {
                MobileError::stale_request("user input request is no longer pending")
            }
            other => MobileError::internal(other.to_string()),
        })?;
    ctx.pending.resolve_user_input(&request_id);
    Ok(json!({ "requestId": request_id, "resolved": true }))
}

fn handle_events_resume(
    ctx: &GatewayContext,
    connection_id: u64,
    params: &serde_json::Map<String, Value>,
) -> Result<Value, MobileError> {
    let thread_id = require_string(params, "threadId")?;
    let after_delivery_seq = params
        .get("afterDeliverySeq")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let outcome = ctx
        .hub
        .resume(connection_id, &thread_id, after_delivery_seq)?;
    Ok(json!({
        "resumed": true,
        "replayed": outcome.replayed,
        "deliverySeq": outcome.delivery_seq,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        ApprovalRequest, ChatMessage, MessageRole, ToolCall, ToolRisk, TurnPhase,
    };

    fn envelope(event: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::with_phase(event, TurnPhase::Executing)
    }

    fn approval_requested(request_id: &str, created_at_ms: u64) -> AgentEventEnvelope {
        envelope(AgentEvent::ApprovalRequested {
            thread_id: "t1".to_string(),
            turn_id: "turn-1".to_string(),
            request: ApprovalRequest {
                id: request_id.to_string(),
                thread_id: "t1".to_string(),
                turn_id: "turn-1".to_string(),
                tool_call_id: "call-1".to_string(),
                tool_name: "run_command".to_string(),
                reason: "external".to_string(),
                auto_approved: false,
                risk: ToolRisk::External,
                arguments: Value::Null,
                preview: None,
                created_at_ms,
                expires_at_ms: created_at_ms + 100,
            },
        })
    }

    #[test]
    fn pending_index_tracks_and_clears_approvals() {
        let index = PendingRequestIndex::new();
        index.observe(&approval_requested("approval-1", 10));
        assert_eq!(index.pending_approval_count("t1"), 1);
        assert_eq!(index.approval("approval-1").unwrap().tool_call_id, "call-1");

        index.observe(&envelope(AgentEvent::ApprovalResolved {
            thread_id: "t1".to_string(),
            turn_id: "turn-1".to_string(),
            request_id: "approval-1".to_string(),
            resolution: ApprovalResolution {
                action: ApprovalAction::Rejected,
                patch: None,
                selected_paths: Vec::new(),
                expected_hashes: Vec::new(),
            },
        }));
        assert_eq!(index.pending_approval_count("t1"), 0);
    }

    #[test]
    fn pending_index_is_bounded() {
        let index = PendingRequestIndex::new();
        for sequence in 0..(MAX_TRACKED_REQUESTS + 20) {
            index.observe(&approval_requested(
                &format!("approval-{sequence}"),
                sequence as u64,
            ));
        }
        assert_eq!(
            index.approvals.lock().unwrap().len(),
            MAX_TRACKED_REQUESTS,
            "index must not grow without bound"
        );
        assert!(index.approval("approval-0").is_none(), "oldest is evicted");
    }

    #[test]
    fn idempotency_cache_returns_previous_result() {
        let cache = IdempotencyCache::new();
        let key = IdempotencyCache::key("device-1", "turn/start", "req-1");
        assert!(cache.get(&key).is_none());
        cache.record(&key, json!({ "turnId": "turn-1" }));
        assert_eq!(cache.get(&key).unwrap()["turnId"], json!("turn-1"));
        assert!(
            cache
                .get(&IdempotencyCache::key("device-2", "turn/start", "req-1"))
                .is_none()
        );
    }

    #[test]
    fn idempotency_cache_is_bounded_and_keeps_first_result() {
        let cache = IdempotencyCache::new();
        let key = IdempotencyCache::key("device-1", "turn/start", "req-1");
        cache.record(&key, json!({ "turnId": "turn-1" }));
        cache.record(&key, json!({ "turnId": "turn-2" }));
        assert_eq!(cache.get(&key).unwrap()["turnId"], json!("turn-1"));

        for sequence in 0..(MAX_TRACKED_REQUESTS + 5) {
            cache.record(
                &IdempotencyCache::key("device-1", "turn/start", &format!("req-{sequence}")),
                json!(sequence),
            );
        }
        assert!(cache.entries.lock().unwrap().len() <= MAX_TRACKED_REQUESTS);
    }

    #[test]
    fn missing_string_parameter_is_invalid_params() {
        let params = serde_json::Map::new();
        let error = require_string(&params, "threadId").unwrap_err();
        assert_eq!(error.kind(), "invalid_params");
        assert!(error.message.contains("threadId"));
    }

    #[test]
    fn blank_string_parameter_is_rejected() {
        let mut params = serde_json::Map::new();
        params.insert("threadId".to_string(), json!("   "));
        assert!(require_string(&params, "threadId").is_err());
    }

    #[test]
    fn state_errors_never_leak_absolute_paths() {
        let error = map_state_error(AppStateError::ThreadWorkspaceMismatch {
            thread_id: "t1".to_string(),
            expected: std::path::PathBuf::from("D:\\code\\other"),
            actual: std::path::PathBuf::from("D:\\code\\k-coder"),
        });
        assert_eq!(error.kind(), "forbidden");
        assert!(!error.message.contains("D:\\"));
    }

    #[test]
    fn state_error_maps_to_stale_request() {
        let error = map_state_error(AppStateError::ExpectedTurnMismatch {
            expected: "turn-1".to_string(),
            actual: "turn-2".to_string(),
        });
        assert_eq!(error.kind(), "stale_request");
    }

    #[test]
    fn notification_never_produces_a_response() {
        let request = RpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: None,
            method: "initialized".to_string(),
            params: None,
        };
        assert!(request.is_notification());
    }

    #[test]
    fn message_role_is_reused_for_steer_payload() {
        // 只是保证测试夹具与协议类型保持同步，避免签名漂移。
        let message = ChatMessage {
            schema_version: 1,
            id: "message-1".to_string(),
            role: MessageRole::User,
            content: Vec::new(),
            created_at_ms: 0,
        };
        assert_eq!(message.role, MessageRole::User);
        let _ = ToolCall {
            id: "call-1".to_string(),
            name: "run_command".to_string(),
            arguments: Value::Null,
            metadata: Value::Null,
        };
    }
}
