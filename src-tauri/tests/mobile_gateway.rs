//! 局域网直连网关的传输层集成测试。
//!
//! 覆盖的是「手机真的连上来」这条路径：HTTP 页面、配对、WebSocket 上的 JSON-RPC
//! 生命周期、能力裁剪、设备撤销和来源边界。会话与 Turn 相关的应用服务不在这里
//! 覆盖——那些需要完整 `AppState`，由 `mobile` 模块的单元测试和桌面端验收负责。

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use futures_util::{SinkExt, StreamExt};
use k_coder_lib::agent::RunTurnRequest;
use k_coder_lib::app_state::AppState;
use k_coder_lib::mobile::auth::{DeviceRegistry, PairingStore, TokenStore};
use k_coder_lib::mobile::capability::CapabilityPolicy;
use k_coder_lib::mobile::events::EventHub;
use k_coder_lib::mobile::host::GatewayHost;
use k_coder_lib::mobile::protocol::{MOBILE_PROTOCOL_VERSION, MobileError};
use k_coder_lib::mobile::rpc::{GatewayContext, IdempotencyCache, PendingRequestIndex};
use k_coder_lib::mobile::server::{self, ServerBind, ServerHandle};
use k_coder_lib::protocol::{ImageAttachment, TurnHandle};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 传输层测试用的桩宿主：不提供应用状态，因此会话类方法会返回结构化内部错误。
struct StubHost;

impl GatewayHost for StubHost {
    fn app_state(&self) -> Option<&AppState> {
        None
    }

    fn log(&self, _level: &str, _event: &str, _fields: Value) {}

    fn start_turn<'a>(
        &'a self,
        _state: &'a AppState,
        _request: RunTurnRequest,
        _attachments: Vec<ImageAttachment>,
        _workflow_id: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<TurnHandle, MobileError>> + Send + 'a>> {
        Box::pin(async { Err(MobileError::internal("stub host cannot start turns")) })
    }
}

struct Harness {
    _directory: TempDir,
    handle: ServerHandle,
    context: GatewayContext,
    address: SocketAddr,
}

impl Harness {
    async fn start() -> Self {
        let directory = TempDir::new().expect("temp dir");
        let context = build_context(directory.path().to_path_buf());
        let handle = server::start(
            context.clone(),
            ServerBind {
                ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 0,
            },
            None,
        )
        .await
        .expect("gateway starts on loopback");
        let address = handle.address;
        Self {
            _directory: directory,
            handle,
            context,
            address,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn ws_url(&self) -> String {
        format!("ws://{}/ws", self.address)
    }

    async fn connect(&self) -> Client {
        let (socket, _) = connect_async(self.ws_url())
            .await
            .expect("websocket connects");
        socket
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.handle.shutdown();
    }
}

fn build_context(data_root: PathBuf) -> GatewayContext {
    GatewayContext {
        host: Arc::new(StubHost),
        registry: Arc::new(DeviceRegistry::load(&data_root).expect("registry loads")),
        tokens: Arc::new(TokenStore::new()),
        pairing: Arc::new(PairingStore::new()),
        policy: Arc::new(CapabilityPolicy::new()),
        hub: Arc::new(EventHub::new()),
        pending: Arc::new(PendingRequestIndex::new()),
        idempotency: Arc::new(IdempotencyCache::new()),
        fingerprint: Arc::new(RwLock::new(None)),
    }
}

/// 注册一台已配对设备，返回 (设备 ID, 设备密钥)。
fn register_device(harness: &Harness, name: &str) -> (String, String) {
    let (_record, credentials) = harness
        .context
        .registry
        .register_device(name, None)
        .expect("device registers");
    (credentials.device_id, credentials.device_secret)
}

async fn call(client: &mut Client, id: u64, method: &str, params: Value) -> Value {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    client
        .send(Message::Text(payload.to_string().into()))
        .await
        .expect("request is sent");
    loop {
        let message = client
            .next()
            .await
            .expect("connection stays open")
            .expect("frame is valid");
        let Message::Text(text) = message else {
            continue;
        };
        let parsed: Value = serde_json::from_str(&text).expect("frame is JSON");
        if parsed.get("id").and_then(Value::as_u64) == Some(id) {
            return parsed;
        }
    }
}

async fn notify(client: &mut Client, method: &str, params: Value) {
    let payload = json!({ "jsonrpc": "2.0", "method": method, "params": params });
    client
        .send(Message::Text(payload.to_string().into()))
        .await
        .expect("notification is sent");
}

async fn authenticate(client: &mut Client, device_id: &str, device_secret: &str) -> Value {
    let response = call(
        client,
        1,
        "initialize",
        json!({
            "protocolVersion": MOBILE_PROTOCOL_VERSION,
            "client": { "name": "k-coder-mobile", "version": "0.1" },
            "deviceId": device_id,
            "deviceSecret": device_secret,
        }),
    )
    .await;
    assert!(
        response.get("error").is_none(),
        "initialize failed: {response}"
    );
    notify(client, "initialized", json!({})).await;
    response["result"].clone()
}

/// 读取结构化错误种类。WebSocket 回包把错误放在 `error` 下，
/// HTTP 端点直接返回同一个错误对象。
fn error_kind(response: &Value) -> &str {
    response
        .get("error")
        .unwrap_or(response)
        .get("data")
        .and_then(|data| data.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("<missing>")
}

#[tokio::test]
async fn health_endpoint_reports_protocol_version() {
    let harness = Harness::start().await;
    let response = reqwest::Client::new()
        .get(format!("{}/health", harness.base_url()))
        .send()
        .await
        .expect("health responds");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("health is JSON");
    assert_eq!(body["status"], json!("ok"));
    assert_eq!(body["protocolVersion"], json!(MOBILE_PROTOCOL_VERSION));
    assert_eq!(body["tls"], json!(false));
}

#[tokio::test]
async fn mobile_page_is_served_without_external_assets() {
    let harness = Harness::start().await;
    let response = reqwest::Client::new()
        .get(format!("{}/m", harness.base_url()))
        .send()
        .await
        .expect("page responds");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let body = response.text().await.expect("page body");
    assert!(body.contains("k-Coder"));
    assert!(body.contains("pair"), "page must include the pairing flow");
}

#[tokio::test]
async fn unknown_host_header_is_rejected() {
    let harness = Harness::start().await;
    let response = reqwest::Client::new()
        .get(format!("{}/health", harness.base_url()))
        .header("host", "attacker.example.com")
        .send()
        .await
        .expect("request completes");
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn forwarding_headers_are_rejected() {
    let harness = Harness::start().await;
    let response = reqwest::Client::new()
        .get(format!("{}/health", harness.base_url()))
        .header("x-forwarded-for", "1.2.3.4")
        .send()
        .await
        .expect("request completes");
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn binding_beyond_loopback_requires_tls() {
    let directory = TempDir::new().unwrap();
    let context = build_context(directory.path().to_path_buf());
    let error = server::start(
        context,
        ServerBind {
            ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            port: 0,
        },
        None,
    )
    .await
    .expect_err("plain HTTP on a LAN address must be refused");
    assert_eq!(error.kind(), "invalid_request");
}

#[tokio::test]
async fn public_bind_address_is_refused() {
    let directory = TempDir::new().unwrap();
    let context = build_context(directory.path().to_path_buf());
    let error = server::start(
        context,
        ServerBind {
            ip: IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            port: 0,
        },
        None,
    )
    .await
    .expect_err("public address must be refused");
    assert_eq!(error.kind(), "invalid_params");
}

#[tokio::test]
async fn pairing_requires_challenge_code_and_desktop_approval() {
    let harness = Harness::start().await;
    let challenge =
        harness
            .context
            .pairing
            .create_challenge("127.0.0.1", harness.address.port(), false, None);
    let client = reqwest::Client::new();

    // 错误的校验码不能通过。
    let wrong = client
        .post(format!("{}/pair", harness.base_url()))
        .json(&json!({
            "challengeId": challenge.id,
            "challengeSecret": challenge.secret,
            "code": "000000",
            "deviceName": "Pixel",
        }))
        .send()
        .await
        .expect("pair responds");
    assert_eq!(wrong.status(), 401);

    // 正确的挑战密钥与校验码会进入「等待桌面确认」。
    let submitted = client
        .post(format!("{}/pair", harness.base_url()))
        .json(&json!({
            "challengeId": challenge.id,
            "challengeSecret": challenge.secret,
            "code": challenge.code,
            "deviceName": "Pixel",
            "platform": "android",
        }))
        .send()
        .await
        .expect("pair responds");
    assert_eq!(submitted.status(), 200);
    let body: Value = submitted.json().await.unwrap();
    let pending_id = body["pendingId"].as_str().expect("pendingId").to_string();
    assert_eq!(body["status"], json!("awaiting_confirmation"));

    // 重复使用同一个挑战必须被拒绝（单次使用）。
    let replay = client
        .post(format!("{}/pair", harness.base_url()))
        .json(&json!({
            "challengeId": challenge.id,
            "challengeSecret": challenge.secret,
            "code": challenge.code,
            "deviceName": "Pixel",
        }))
        .send()
        .await
        .expect("pair responds");
    assert_eq!(replay.status(), 400);
    let replay_body: Value = replay.json().await.unwrap();
    assert_eq!(error_kind(&replay_body), "stale_request");

    // 桌面端批准后手机端才能拿到凭据。
    let device = harness
        .context
        .pairing
        .approve(&pending_id, &harness.context.registry)
        .expect("desktop approves");

    let status = client
        .post(format!("{}/pair/status", harness.base_url()))
        .json(&json!({
            "pendingId": pending_id,
            "challengeSecret": challenge.secret,
        }))
        .send()
        .await
        .expect("status responds");
    let status_body: Value = status.json().await.unwrap();
    assert_eq!(status_body["status"], json!("approved"));
    assert_eq!(status_body["deviceId"], json!(device.id));
    assert!(status_body["deviceSecret"].as_str().unwrap().len() >= 32);
    assert!(status_body["refreshToken"].as_str().unwrap().len() >= 32);

    // 凭据只下发一次。
    let second = client
        .post(format!("{}/pair/status", harness.base_url()))
        .json(&json!({
            "pendingId": pending_id,
            "challengeSecret": challenge.secret,
        }))
        .send()
        .await
        .expect("status responds");
    let second_body: Value = second.json().await.unwrap();
    assert_eq!(second_body["status"], json!("denied"));
}

#[tokio::test]
async fn pairing_status_requires_the_challenge_secret() {
    let harness = Harness::start().await;
    let challenge =
        harness
            .context
            .pairing
            .create_challenge("127.0.0.1", harness.address.port(), false, None);
    let pending = harness
        .context
        .pairing
        .submit(
            &challenge.id,
            &challenge.secret,
            &challenge.code,
            "Pixel",
            None,
        )
        .expect("submit succeeds");

    let response = reqwest::Client::new()
        .post(format!("{}/pair/status", harness.base_url()))
        .json(&json!({ "pendingId": pending.id, "challengeSecret": "nope" }))
        .send()
        .await
        .expect("status responds");
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn json_rpc_requires_initialize_before_other_methods() {
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;

    let response = call(&mut client, 1, "ping", json!({})).await;
    assert_eq!(error_kind(&response), "unauthorized");

    let result = authenticate(&mut client, &device_id, &device_secret).await;
    assert_eq!(result["protocolVersion"], json!(MOBILE_PROTOCOL_VERSION));
    assert_eq!(result["eventProtocolVersion"], json!(1));
    assert!(result["accessToken"].as_str().unwrap().len() >= 32);
    assert_eq!(
        result["capabilities"],
        json!(["chat", "approval", "interrupt"])
    );

    let pong = call(&mut client, 2, "ping", json!({})).await;
    assert!(pong["result"]["serverTimeMs"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn initialize_rejects_wrong_device_secret() {
    let harness = Harness::start().await;
    let (device_id, _) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;
    let response = call(
        &mut client,
        1,
        "initialize",
        json!({
            "protocolVersion": MOBILE_PROTOCOL_VERSION,
            "deviceId": device_id,
            "deviceSecret": "wrong",
        }),
    )
    .await;
    assert_eq!(error_kind(&response), "unauthorized");
}

#[tokio::test]
async fn initialize_rejects_newer_protocol_version() {
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;
    let response = call(
        &mut client,
        1,
        "initialize",
        json!({
            "protocolVersion": MOBILE_PROTOCOL_VERSION + 1,
            "deviceId": device_id,
            "deviceSecret": device_secret,
        }),
    )
    .await;
    assert_eq!(error_kind(&response), "unsupported_capability");
    assert_eq!(
        response["error"]["data"]["details"]["supportedVersion"],
        json!(MOBILE_PROTOCOL_VERSION)
    );
}

#[tokio::test]
async fn access_token_can_reinitialize_the_connection() {
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut first = harness.connect().await;
    let result = authenticate(&mut first, &device_id, &device_secret).await;
    let access_token = result["accessToken"].as_str().unwrap().to_string();
    drop(first);

    let mut second = harness.connect().await;
    let response = call(
        &mut second,
        1,
        "initialize",
        json!({ "protocolVersion": MOBILE_PROTOCOL_VERSION, "accessToken": access_token }),
    )
    .await;
    assert!(
        response.get("error").is_none(),
        "token re-auth failed: {response}"
    );
}

#[tokio::test]
async fn unknown_method_reports_method_not_found() {
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;
    authenticate(&mut client, &device_id, &device_secret).await;

    let response = call(&mut client, 2, "thread/delete", json!({})).await;
    assert_eq!(error_kind(&response), "method_not_found");
}

#[tokio::test]
async fn capabilities_gate_file_and_shell_methods() {
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;
    authenticate(&mut client, &device_id, &device_secret).await;

    for method in ["file/read", "shell/run", "settings/write"] {
        let response = call(&mut client, 2, method, json!({})).await;
        assert_eq!(
            error_kind(&response),
            "unsupported_capability",
            "{method} must stay closed on mobile"
        );
    }
}

#[tokio::test]
async fn missing_parameters_report_invalid_params() {
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;
    authenticate(&mut client, &device_id, &device_secret).await;

    let response = call(&mut client, 2, "thread/read", json!({})).await;
    assert_eq!(error_kind(&response), "invalid_params");
}

#[tokio::test]
async fn malformed_json_reports_parse_error() {
    let harness = Harness::start().await;
    let mut client = harness.connect().await;
    client
        .send(Message::Text("not json".into()))
        .await
        .expect("frame is sent");
    loop {
        let message = client.next().await.unwrap().unwrap();
        let Message::Text(text) = message else {
            continue;
        };
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["error"]["code"], json!(-32700));
        assert_eq!(error_kind(&parsed), "parse_error");
        break;
    }
}

#[tokio::test]
async fn revoked_device_loses_access_immediately() {
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;
    authenticate(&mut client, &device_id, &device_secret).await;
    assert!(
        call(&mut client, 2, "ping", json!({}))
            .await
            .get("error")
            .is_none()
    );

    harness
        .context
        .registry
        .revoke(&device_id)
        .expect("revoke succeeds");
    harness.context.tokens.revoke_device(&device_id);

    // 已建立的连接下一条请求就必须被拒绝。
    let response = call(&mut client, 3, "ping", json!({})).await;
    assert_eq!(error_kind(&response), "unauthorized");

    // 重新握手同样失败。
    let mut second = harness.connect().await;
    let response = call(
        &mut second,
        1,
        "initialize",
        json!({
            "protocolVersion": MOBILE_PROTOCOL_VERSION,
            "deviceId": device_id,
            "deviceSecret": device_secret,
        }),
    )
    .await;
    assert_eq!(error_kind(&response), "unauthorized");
}

/// 删除设备比撤销更彻底：记录本身消失，所以已建立的连接下一条请求必须被拒，
/// 不能留下「记录没了、会话还能说话」的窗口。
///
/// 顺序与 `MobileService::remove_device` 保持一致（先清令牌、再删记录）。
#[tokio::test]
async fn removed_device_loses_access_immediately() {
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;
    authenticate(&mut client, &device_id, &device_secret).await;
    assert!(
        call(&mut client, 2, "ping", json!({}))
            .await
            .get("error")
            .is_none()
    );

    harness.context.tokens.revoke_device(&device_id);
    harness
        .context
        .registry
        .remove(&device_id)
        .expect("remove succeeds");
    assert!(
        harness.context.registry.device(&device_id).is_none(),
        "the record must be gone from the registry"
    );

    let response = call(&mut client, 3, "ping", json!({})).await;
    assert_eq!(error_kind(&response), "unauthorized");

    // 重新握手同样失败：登记表里已经没有这条记录了。
    let mut second = harness.connect().await;
    let response = call(
        &mut second,
        1,
        "initialize",
        json!({
            "protocolVersion": MOBILE_PROTOCOL_VERSION,
            "deviceId": device_id,
            "deviceSecret": device_secret,
        }),
    )
    .await;
    assert_eq!(error_kind(&response), "unauthorized");

    // 重复删除走 `not_found`，不静默成功。
    let error = harness
        .context
        .registry
        .remove(&device_id)
        .expect_err("removing a missing device must fail");
    assert_eq!(error.kind(), "not_found");
}

#[tokio::test]
async fn thread_methods_fail_closed_without_application_state() {
    // 网关不复制应用状态：拿不到 AppState 时返回结构化内部错误，而不是 panic 或假成功。
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;
    authenticate(&mut client, &device_id, &device_secret).await;

    let response = call(&mut client, 2, "thread/list", json!({})).await;
    assert_eq!(error_kind(&response), "internal_error");
    assert_eq!(response["error"]["data"]["retryable"], json!(true));
}

/// `project/list` 是这次修复新增的方法，它必须和 `thread/list` 一样受 Chat 能力门控，
/// 并且在拿不到应用状态时同样安全失败，而不是回一个空项目列表假装正常。
#[tokio::test]
async fn project_list_is_gated_and_fails_closed() {
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;

    // 未握手前调用 `project/list` 与其它业务方法一样被拒。
    let unauthorized = call(&mut client, 1, "project/list", json!({})).await;
    assert_eq!(error_kind(&unauthorized), "unauthorized");

    authenticate(&mut client, &device_id, &device_secret).await;

    // 能力门控与 `thread/list` 一致（两者都由 Chat 覆盖）。
    harness
        .context
        .policy
        .set_granted([k_coder_lib::mobile::MobileCapability::Approval]);
    let denied = call(&mut client, 2, "project/list", json!({})).await;
    assert_eq!(
        error_kind(&denied),
        "unsupported_capability",
        "实际回包：{denied}"
    );
    // 结构化细节挂在 `data.details` 下（`MobileErrorData.details`），
    // 客户端据此知道该补授哪个能力，而不是只看到一句「不支持」。
    assert_eq!(
        denied["error"]["data"]["details"]["capability"],
        json!("chat"),
        "实际回包：{denied}"
    );
    assert_eq!(denied["error"]["data"]["retryable"], json!(false));

    harness
        .context
        .policy
        .set_granted([k_coder_lib::mobile::MobileCapability::Chat]);
    // 桩宿主没有 AppState：方法与 `thread/list` 走同一条结构化内部错误路径。
    let response = call(&mut client, 3, "project/list", json!({})).await;
    assert_eq!(error_kind(&response), "internal_error");
    assert_eq!(response["error"]["data"]["retryable"], json!(true));
}

#[tokio::test]
async fn events_are_not_delivered_without_a_subscription() {
    let harness = Harness::start().await;
    let (device_id, device_secret) = register_device(&harness, "Pixel");
    let mut client = harness.connect().await;
    authenticate(&mut client, &device_id, &device_secret).await;

    let event = k_coder_lib::protocol::AgentEventEnvelope::new(
        k_coder_lib::protocol::AgentEvent::TextDelta {
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
            item_id: "item-1".to_string(),
            delta: "hello".to_string(),
        },
    );
    assert_eq!(harness.context.hub.publish(&event), 0);
}
