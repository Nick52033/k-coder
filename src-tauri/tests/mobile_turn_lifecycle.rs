//! 运行中 Turn 的流式事件、取消与审批往返。
//!
//! 这是 P10-188 的最后一个可自动化验收缺口。做法是**不消耗真实 Provider 额度**：用
//! [`FakeProvider`] 驱动**真实的** [`AgentRuntime`]，再经 `mobile::rpc::dispatch` 走一遍
//! 手机端会走的 JSON-RPC 契约，断言领域事件确实被扇出给已订阅的连接、以及
//! `turn/interrupt` / `approval/respond` / `tool/requestUserInput/respond` 三条往返真的
//! 作用在**运行中**的 Turn 上。
//!
//! 刻意不起 HTTP/TLS/WebSocket 服务器：传输层（页面、配对、握手、能力裁剪、来源边界）
//! 已由 `tests/mobile_gateway.rs` 的 19 项测试覆盖。这里只关心 `dispatch` 之后发生的事，
//! 而 `dispatch` 与传输层完全解耦，因此可以直接驱动。

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k_coder_lib::agent::{AgentRuntime, EventPublisher, RunTurnRequest};
use k_coder_lib::app_state::AppState;
use k_coder_lib::mobile::MobileService;
use k_coder_lib::mobile::events::{OutboundQueue, QueueMessage};
use k_coder_lib::mobile::host::GatewayHost;
use k_coder_lib::mobile::protocol::{
    MOBILE_PROTOCOL_VERSION, MobileError, RpcRequest, RpcResponse,
};
use k_coder_lib::mobile::rpc::{GatewayContext, RpcSession, map_state_error};
use k_coder_lib::protocol::{
    AgentEventEnvelope, ImageAttachment, PROTOCOL_VERSION, ToolCall, ToolDefinition, ToolResult,
    ToolRisk, TurnHandle, TurnState,
};
use k_coder_lib::providers::testing::FakeProvider;
use k_coder_lib::providers::{CredentialError, CredentialStore, Provider, ProviderEvent};
use k_coder_lib::tools::{ToolContext, ToolError, ToolHandler, ToolRegistry};
use serde_json::{Value, json};
use tauri::Manager;
use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

/// 事件从发布到出现在出站队列里是同步的，超时只用于兜住「根本没发生」的情况。
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);
/// 判定「安静下来」的观察窗口：这段时间内没有新帧才认为 Turn 真的停了。
const QUIET_WINDOW: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// 桩件
// ---------------------------------------------------------------------------

/// 集成测试拿不到 `commands::mod` 里那个 `#[cfg(test)]` 的凭据桩，自己写一个内存实现。
/// `AppState::with_workspace_and_credentials` 只需要它能读写，不会在 Turn 里被调用。
#[derive(Default)]
struct MemoryCredentials {
    keys: Mutex<HashMap<String, String>>,
}

impl CredentialStore for MemoryCredentials {
    fn get_api_key(&self, provider_id: &str) -> Result<Option<String>, CredentialError> {
        Ok(self
            .keys
            .lock()
            .expect("credential lock poisoned")
            .get(provider_id)
            .cloned())
    }

    fn set_api_key(&self, provider_id: &str, api_key: &str) -> Result<(), CredentialError> {
        self.keys
            .lock()
            .expect("credential lock poisoned")
            .insert(provider_id.to_string(), api_key.to_string());
        Ok(())
    }

    fn delete_api_key(&self, provider_id: &str) -> Result<(), CredentialError> {
        self.keys
            .lock()
            .expect("credential lock poisoned")
            .remove(provider_id);
        Ok(())
    }
}

/// 与生产侧 `TauriEventPublisher` 同形状的发布器：**惰性**从应用里找回 `MobileService`
/// 再扇出。
///
/// 之所以不直接持有 `Arc<MobileService>`：`MobileService` 由应用管理，而宿主又被
/// `MobileService` 持有，直接引用会形成构造顺序上的循环依赖。`try_state` 把这个环打开。
struct GatewayPublisher {
    app: tauri::AppHandle<MockRuntime>,
}

impl EventPublisher for GatewayPublisher {
    fn publish(&self, event: AgentEventEnvelope) {
        if let Some(service) = self.app.try_state::<MobileService<MockRuntime>>() {
            service.publish_event(&event);
        }
    }
}

/// 真实 Turn 宿主：`app_state()` 返回真的 `AppState`，`start_turn()` 起一个真的
/// `AgentRuntime` 后台任务，publisher 走 [`GatewayPublisher`] 扇出到移动端。
///
/// 与生产路径的差异只有两处，都是刻意的：
/// 1. Provider 是 `FakeProvider`（不联网、不消耗额度）；
/// 2. 后台任务用测试自己的 tokio runtime（生产用 `tauri::async_runtime::spawn`）。
struct TurnHost {
    state: Arc<AppState>,
    app: tauri::AppHandle<MockRuntime>,
    provider: Arc<FakeProvider>,
    tools: ToolRegistry,
    workspace_root: PathBuf,
}

impl GatewayHost for TurnHost {
    fn app_state(&self) -> Option<&AppState> {
        Some(self.state.as_ref())
    }

    fn log(&self, _level: &str, _event: &str, _fields: Value) {}

    fn start_turn<'a>(
        &'a self,
        state: &'a AppState,
        request: RunTurnRequest,
        _attachments: Vec<ImageAttachment>,
        _workflow_id: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<TurnHandle, MobileError>> + Send + 'a>> {
        Box::pin(async move {
            let thread_id = request.thread_id.clone();
            let workspace = state
                .resolve_thread_workspace(&thread_id)
                .await
                .map_err(map_state_error)?
                .ok_or_else(|| {
                    MobileError::forbidden("this thread is not associated with a project workspace")
                })?;

            // `turn/interrupt` 要求线程已注册且 turnId 匹配，所以必须先登记再跑 runtime。
            let turn_id = uuid::Uuid::new_v4().to_string();
            let (cancellation, _control) = state
                .begin_turn_with_id_in_workspace(&thread_id, &turn_id, &workspace)
                .await
                .map_err(map_state_error)?;

            let runtime = AgentRuntime::with_tools_and_approvals(
                state.repository(),
                self.tools.clone(),
                self.workspace_root.clone(),
                state.approvals(),
            )
            // 审批与用户输入必须落在 `AppState` 的同一份管理器上，否则
            // `approval/respond` / `tool/requestUserInput/respond` 解析的是另一个等待者。
            .with_user_inputs(state.user_inputs());

            let provider: Arc<dyn Provider> = self.provider.clone();
            let publisher: Arc<dyn EventPublisher> = Arc::new(GatewayPublisher {
                app: self.app.clone(),
            });
            let state = self.state.clone();
            let task_thread_id = thread_id.clone();
            let task_turn_id = turn_id.clone();
            tokio::spawn(async move {
                let _ = runtime
                    .run_turn_with_attachments_and_id(
                        provider,
                        "fake-model".to_string(),
                        request,
                        Vec::new(),
                        task_turn_id,
                        cancellation,
                        publisher,
                    )
                    .await;
                // 生产侧的 `execute_turn` 也会在收尾时清掉 active turn。
                state.finish_turn(&task_thread_id).await;
            });

            Ok(TurnHandle {
                schema_version: PROTOCOL_VERSION,
                thread_id,
                turn_id,
                state: TurnState::Streaming,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

struct Harness {
    _data_root: TempDir,
    _workspace_root: TempDir,
    _app: tauri::App<MockRuntime>,
    context: GatewayContext,
    session: Arc<Mutex<RpcSession>>,
    outbound: Arc<OutboundQueue>,
    state: Arc<AppState>,
    provider: Arc<FakeProvider>,
    thread_id: String,
    device_id: String,
    device_secret: String,
}

impl Harness {
    async fn start(provider: FakeProvider, tools: ToolRegistry) -> Self {
        let data_root = TempDir::new().expect("data root");
        let workspace_root = TempDir::new().expect("workspace root");
        let state = Arc::new(
            AppState::with_workspace_and_credentials(
                data_root.path(),
                workspace_root.path(),
                Arc::new(MemoryCredentials::default()),
            )
            .expect("app state must build"),
        );
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("mock app must build");

        let host: Arc<dyn GatewayHost> = Arc::new(TurnHost {
            state: state.clone(),
            app: app.handle().clone(),
            provider: Arc::new(provider.clone()),
            tools,
            workspace_root: workspace_root
                .path()
                .canonicalize()
                .expect("workspace root must canonicalize"),
        });
        let service =
            MobileService::with_host(app.handle().clone(), data_root.path().to_path_buf(), host)
                .expect("mobile service must build");
        // 复用服务自己的上下文，保证「dispatch 订阅的连接」和「publish_event 扇出的连接」
        // 是同一个 EventHub。
        let context = service.context().clone();
        app.manage(service);

        let thread = state
            .repository()
            .create_thread()
            .await
            .expect("thread must be created");

        // 模拟一条已连上来的手机连接：注册出站队列，并把连接 ID 写进会话，
        // `thread/subscribe` 才会订阅到这条队列。
        let (connection_id, outbound) = context.hub.register();
        let session = Arc::new(Mutex::new(RpcSession {
            connection_id,
            peer: "127.0.0.1".to_string(),
            ..RpcSession::default()
        }));

        let (_record, credentials) = context
            .registry
            .register_device("Pixel", None)
            .expect("device must register");

        Self {
            _data_root: data_root,
            _workspace_root: workspace_root,
            _app: app,
            context,
            session,
            outbound,
            state,
            provider: Arc::new(provider),
            thread_id: thread.id,
            device_id: credentials.device_id,
            device_secret: credentials.device_secret,
        }
    }

    /// `initialize` + `initialized` + `thread/subscribe`，把连接推进到可以收 Turn 事件的状态。
    async fn connect_and_subscribe(&self) {
        let response = self
            .call(
                1,
                "initialize",
                json!({
                    "protocolVersion": MOBILE_PROTOCOL_VERSION,
                    "client": { "name": "k-coder-mobile", "version": "0.1" },
                    "deviceId": self.device_id,
                    "deviceSecret": self.device_secret,
                }),
            )
            .await;
        assert!(
            response.error.is_none(),
            "initialize must succeed: {response:?}"
        );
        self.notify("initialized", json!({})).await;

        let response = self
            .call(2, "thread/subscribe", json!({ "threadId": self.thread_id }))
            .await;
        assert!(
            response.error.is_none(),
            "subscribe must succeed: {response:?}"
        );
    }

    async fn dispatch(&self, id: Option<u64>, method: &str, params: Value) -> Option<RpcResponse> {
        k_coder_lib::mobile::rpc::dispatch(
            &self.context,
            &self.session,
            RpcRequest {
                jsonrpc: Some("2.0".to_string()),
                id: id.map(|id| json!(id)),
                method: method.to_string(),
                params: Some(params),
            },
        )
        .await
    }

    async fn call(&self, id: u64, method: &str, params: Value) -> RpcResponse {
        self.dispatch(Some(id), method, params)
            .await
            .expect("a request must produce a response")
    }

    async fn notify(&self, method: &str, params: Value) {
        let response = self.dispatch(None, method, params).await;
        assert!(response.is_none(), "notifications must not be answered");
    }

    /// 取下一帧事件载荷（`{"jsonrpc":"2.0","method":"event","params":{…}}`）。
    async fn next_frame(&self) -> Value {
        match tokio::time::timeout(EVENT_TIMEOUT, self.outbound.next())
            .await
            .expect("an event must arrive before the timeout")
            .expect("the outbound queue must stay open")
        {
            QueueMessage::Payload(payload) => payload,
            QueueMessage::Resync => panic!("the queue overflowed; the test must not drop events"),
        }
    }

    /// 一直读到 `stop` 返回 true 的那一帧（含），返回途中所有帧。
    async fn drain_until(&self, mut stop: impl FnMut(&Value) -> bool) -> Vec<Value> {
        let mut frames = Vec::new();
        loop {
            let frame = self.next_frame().await;
            let last = stop(&frame);
            frames.push(frame);
            if last {
                return frames;
            }
        }
    }

    async fn active_turn_id(&self) -> Option<String> {
        self.state.active_turn_id(&self.thread_id).await
    }

    /// 等 active turn 被清空；清不空就 panic，避免把「收尾没做」误判成断言失败。
    async fn wait_for_idle(&self) {
        for _ in 0..400 {
            if self.active_turn_id().await.is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the turn never left the active set");
    }

    /// 观察窗口内不应再有任何帧。返回超时前真的收到的帧（正常情况是空）。
    async fn assert_quiet(&self) -> Vec<Value> {
        let mut extra = Vec::new();
        while let Ok(Some(message)) = tokio::time::timeout(QUIET_WINDOW, self.outbound.next()).await
        {
            match message {
                QueueMessage::Payload(payload) => extra.push(payload),
                QueueMessage::Resync => panic!("unexpected resync"),
            }
        }
        extra
    }

    fn frame_kind(frame: &Value) -> &str {
        frame
            .get("params")
            .and_then(|params| params.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("<missing>")
    }

    fn frame_delta(frame: &Value) -> &str {
        frame["params"]["delta"].as_str().unwrap_or_default()
    }
}

/// 收集所有 `text_delta` 的分片，保持到达顺序。
fn deltas_of(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .filter(|frame| Harness::frame_kind(frame) == "text_delta")
        .map(Harness::frame_delta)
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// (a) 流式增量实时推送给已订阅的连接
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn running_turn_streams_text_deltas_to_the_subscribed_connection() {
    let harness = Harness::start(
        FakeProvider::text(&["Hello", " from", " fake"]),
        ToolRegistry::read_only(),
    )
    .await;
    harness.connect_and_subscribe().await;

    let response = harness
        .call(
            3,
            "turn/start",
            json!({
                "threadId": harness.thread_id,
                "input": "say hello",
                "requestId": "req-start-1",
            }),
        )
        .await;
    assert!(response.error.is_none(), "turn/start failed: {response:?}");
    let result = response.result.expect("turn/start must return a result");
    let turn_id = result["turnId"]
        .as_str()
        .expect("turnId must be a string")
        .to_string();
    assert_eq!(result["threadId"], json!(harness.thread_id));
    assert_eq!(result["state"], json!("streaming"));

    let frames = harness
        .drain_until(|frame| Harness::frame_kind(frame) == "turn_completed")
        .await;

    // 事件必须属于本次 Turn，且顺序是先 turn_started 再分片再 turn_completed。
    let kinds: Vec<&str> = frames.iter().map(Harness::frame_kind).collect();
    assert_eq!(
        kinds.first(),
        Some(&"turn_started"),
        "the first frame must open the turn: {kinds:?}"
    );
    assert_eq!(kinds.last(), Some(&"turn_completed"));
    for frame in &frames {
        assert_eq!(
            frame["params"]["turnId"],
            json!(turn_id),
            "every frame must belong to the running turn"
        );
    }

    // 分片内容与顺序必须与 FakeProvider 脚本逐字一致——这是「实时推送」的证据：
    // 分片是分多次到达的，不是结束后一次性补齐的。
    assert_eq!(deltas_of(&frames), vec!["Hello", " from", " fake"]);
    assert!(
        !frames
            .iter()
            .any(|frame| Harness::frame_kind(frame) == "reasoning_summary_delta"),
        "private reasoning must never reach the phone"
    );

    harness.wait_for_idle().await;
    assert_eq!(harness.provider.requests().len(), 1);
}

// ---------------------------------------------------------------------------
// (b) turn/interrupt 真的取消运行中的 Turn
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn turn_interrupt_cancels_a_running_turn_and_stops_the_stream() {
    // 每个分片之间留 120ms，让 Turn 持续足够久，以便中途打断。
    let harness = Harness::start(
        FakeProvider::text(&["one", "two", "three", "four", "five"])
            .with_delay(Duration::from_millis(120)),
        ToolRegistry::read_only(),
    )
    .await;
    harness.connect_and_subscribe().await;

    let response = harness
        .call(
            3,
            "turn/start",
            json!({
                "threadId": harness.thread_id,
                "input": "keep talking",
                "requestId": "req-start-2",
            }),
        )
        .await;
    let result = response.result.expect("turn/start must return a result");
    let turn_id = result["turnId"].as_str().unwrap().to_string();

    // 等到流真的开始了再打断，否则测的就不是「运行中」。
    let first = harness
        .drain_until(|frame| Harness::frame_kind(frame) == "text_delta")
        .await;
    assert_eq!(deltas_of(&first), vec!["one"]);
    assert_eq!(
        harness.active_turn_id().await.as_deref(),
        Some(&turn_id[..])
    );

    let response = harness
        .call(
            4,
            "turn/interrupt",
            json!({
                "threadId": harness.thread_id,
                "turnId": turn_id,
                "requestId": "req-interrupt-1",
            }),
        )
        .await;
    assert!(response.error.is_none(), "interrupt failed: {response:?}");
    assert_eq!(response.result.unwrap()["interrupted"], json!(true));

    let tail = harness
        .drain_until(|frame| Harness::frame_kind(frame) == "turn_cancelled")
        .await;
    assert_eq!(
        Harness::frame_kind(tail.last().unwrap()),
        "turn_cancelled",
        "the turn must end in the cancelled state"
    );
    assert!(
        !tail
            .iter()
            .chain(first.iter())
            .any(|frame| Harness::frame_kind(frame) == "turn_completed"),
        "a cancelled turn must never also report completion"
    );

    // 脚本有 5 个分片；被取消的 Turn 不可能把它们全部吐完。
    let all = deltas_of(&first).len() + deltas_of(&tail).len();
    assert!(
        all < 5,
        "cancellation must cut the stream short, but {all} deltas arrived"
    );

    // 「不再收到新的 textDelta」：取消之后必须有安静窗口。
    let extra = harness.assert_quiet().await;
    assert!(
        extra.is_empty(),
        "no frame may follow turn_cancelled: {extra:?}"
    );

    harness.wait_for_idle().await;
    assert!(
        harness.active_turn_id().await.is_none(),
        "interrupt must release the active turn slot"
    );
}

// ---------------------------------------------------------------------------
// (c) approval/respond 往返
// ---------------------------------------------------------------------------

/// 审批往返用的写工具：风险等级是 `Write`，策略据此要求审批；执行体只回一个字符串，
/// 不碰文件系统（测试要验证的是审批通路，不是补丁落盘）。
struct DemoWriteTool;

const DEMO_WRITE_TOOL: &str = "demo_write";

#[async_trait::async_trait]
impl ToolHandler for DemoWriteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: DEMO_WRITE_TOOL.to_string(),
            description: "test-only write tool that requires approval".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(
        &self,
        _context: &ToolContext,
        arguments: Value,
        _cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            success: true,
            output: format!("demo_write applied: {arguments}"),
            metadata: Value::Null,
        })
    }
}

fn registry_requiring_approval() -> ToolRegistry {
    let mut risks = HashMap::new();
    risks.insert(DEMO_WRITE_TOOL.to_string(), ToolRisk::Write);
    ToolRegistry::read_only()
        .with_additional_handlers(vec![Arc::new(DemoWriteTool)], risks)
        .expect("the demo tool must register")
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_respond_resumes_the_running_turn() {
    let provider = FakeProvider::script(vec![
        vec![
            Ok(ProviderEvent::ToolCall {
                call: ToolCall {
                    id: "call-demo-1".to_string(),
                    name: DEMO_WRITE_TOOL.to_string(),
                    arguments: json!({ "value": "hello" }),
                    metadata: json!({}),
                },
            }),
            Ok(ProviderEvent::Completed),
        ],
        vec![
            Ok(ProviderEvent::TextDelta {
                delta: "approved and finished".to_string(),
            }),
            Ok(ProviderEvent::Completed),
        ],
    ]);
    let harness = Harness::start(provider, registry_requiring_approval()).await;
    harness.connect_and_subscribe().await;

    let response = harness
        .call(
            3,
            "turn/start",
            json!({
                "threadId": harness.thread_id,
                "input": "write something",
                "requestId": "req-start-3",
            }),
        )
        .await;
    let turn_id = response.result.expect("turn/start must return a result")["turnId"]
        .as_str()
        .unwrap()
        .to_string();

    // 审批请求必须既登记进 PendingRequestIndex，也推给手机端。
    let pending = harness
        .drain_until(|frame| Harness::frame_kind(frame) == "approval_requested")
        .await;
    let request = pending.last().unwrap()["params"]["request"].clone();
    let request_id = request["id"].as_str().expect("request id").to_string();
    let tool_call_id = request["toolCallId"]
        .as_str()
        .expect("toolCallId")
        .to_string();
    assert_eq!(request["toolName"], json!(DEMO_WRITE_TOOL));
    assert_eq!(request["turnId"], json!(turn_id));
    assert_eq!(request["autoApproved"], json!(false));
    assert!(
        harness.context.pending.approval(&request_id).is_some(),
        "the approval must be indexed for approval/respond"
    );
    // 补丁正文不下发；测试工具没有预览，因此 preview 缺省。
    assert!(request.get("preview").is_none() || request["preview"].is_null());

    let response = harness
        .call(
            4,
            "approval/respond",
            json!({
                "requestId": request_id,
                "action": "approved",
                "toolCallId": tool_call_id,
                "expectedCreatedAtMs": request["createdAtMs"],
            }),
        )
        .await;
    assert!(response.error.is_none(), "approval failed: {response:?}");
    assert_eq!(response.result.unwrap()["resolved"], json!(true));
    assert!(
        harness.context.pending.approval(&request_id).is_none(),
        "a resolved approval must leave the index"
    );

    // Turn 必须继续：审批通过后工具执行、第二次请求拿到文本、然后正常收尾。
    let rest = harness
        .drain_until(|frame| Harness::frame_kind(frame) == "turn_completed")
        .await;
    let kinds: Vec<&str> = rest.iter().map(Harness::frame_kind).collect();
    assert!(
        kinds.contains(&"approval_resolved"),
        "the phone must see the resolution: {kinds:?}"
    );
    assert!(
        kinds.contains(&"tool_completed"),
        "the approved tool must actually run: {kinds:?}"
    );
    let completed = rest
        .iter()
        .find(|frame| Harness::frame_kind(frame) == "tool_completed")
        .expect("the approved call must report a result");
    assert_eq!(
        completed["params"]["result"]["success"],
        json!(true),
        "the approved tool must report success: {completed}"
    );
    assert_eq!(deltas_of(&rest), vec!["approved and finished"]);
    assert_eq!(
        kinds.last(),
        Some(&"turn_completed"),
        "the turn must complete after approval: {kinds:?}"
    );

    harness.wait_for_idle().await;
    // 两次 provider 请求：第一次给工具调用，第二次给审批通过后的收尾文本。
    assert_eq!(harness.provider.requests().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_approval_fails_the_tool_without_running_it() {
    let provider = FakeProvider::script(vec![
        vec![
            Ok(ProviderEvent::ToolCall {
                call: ToolCall {
                    id: "call-demo-2".to_string(),
                    name: DEMO_WRITE_TOOL.to_string(),
                    arguments: json!({ "value": "nope" }),
                    metadata: json!({}),
                },
            }),
            Ok(ProviderEvent::Completed),
        ],
        vec![
            Ok(ProviderEvent::TextDelta {
                delta: "gave up".to_string(),
            }),
            Ok(ProviderEvent::Completed),
        ],
    ]);
    let harness = Harness::start(provider, registry_requiring_approval()).await;
    harness.connect_and_subscribe().await;

    harness
        .call(
            3,
            "turn/start",
            json!({
                "threadId": harness.thread_id,
                "input": "write something",
                "requestId": "req-start-4",
            }),
        )
        .await;

    let pending = harness
        .drain_until(|frame| Harness::frame_kind(frame) == "approval_requested")
        .await;
    let request_id = pending.last().unwrap()["params"]["request"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let response = harness
        .call(
            4,
            "approval/respond",
            json!({ "requestId": request_id, "action": "rejected" }),
        )
        .await;
    assert!(response.error.is_none(), "reject failed: {response:?}");

    let rest = harness
        .drain_until(|frame| Harness::frame_kind(frame) == "turn_completed")
        .await;
    let kinds: Vec<&str> = rest.iter().map(Harness::frame_kind).collect();
    assert!(kinds.contains(&"approval_resolved"), "{kinds:?}");
    // 被拒绝的工具不得执行。`ToolCompleted` 仍会出现（Turn 必须为每个已开始的调用留下结果），
    // 但它携带的必须是失败结果，且带 `approval_rejected` 原因。
    let completed = rest
        .iter()
        .find(|frame| Harness::frame_kind(frame) == "tool_completed")
        .expect("the rejected call must still be closed out with a result");
    assert_eq!(
        completed["params"]["result"]["success"],
        json!(false),
        "a rejected tool must not report success: {completed}"
    );
    assert!(
        completed["params"]["result"]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("approval_rejected"),
        "the failure must carry the rejection reason: {completed}"
    );
    assert_eq!(deltas_of(&rest), vec!["gave up"]);
    harness.wait_for_idle().await;
}

// ---------------------------------------------------------------------------
// (d) tool/requestUserInput/respond 往返
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn user_input_respond_resumes_the_running_turn() {
    let provider = FakeProvider::script(vec![
        vec![
            Ok(ProviderEvent::ToolCall {
                call: ToolCall {
                    id: "call-ask-1".to_string(),
                    name: "request_user_input".to_string(),
                    arguments: json!({
                        "questions": [{
                            "question": "Which database should I target?",
                            "options": ["sqlite", "postgres"]
                        }]
                    }),
                    metadata: json!({}),
                },
            }),
            Ok(ProviderEvent::Completed),
        ],
        vec![
            Ok(ProviderEvent::TextDelta {
                delta: "using sqlite".to_string(),
            }),
            Ok(ProviderEvent::Completed),
        ],
    ]);
    // `request_user_input` 由 AgentRuntime 按名字拦截，不需要注册成工具；注册表保持只读，
    // 这样也顺带证明「拦截发生在授权之前」。
    let harness = Harness::start(provider, ToolRegistry::read_only()).await;
    harness.connect_and_subscribe().await;

    harness
        .call(
            3,
            "turn/start",
            json!({
                "threadId": harness.thread_id,
                "input": "pick a database",
                "requestId": "req-start-5",
            }),
        )
        .await;

    let pending = harness
        .drain_until(|frame| Harness::frame_kind(frame) == "user_input_requested")
        .await;
    let request = pending.last().unwrap()["params"]["request"].clone();
    let request_id = request["id"].as_str().expect("request id").to_string();
    assert_eq!(
        request["questions"][0]["question"],
        json!("Which database should I target?")
    );
    assert_eq!(
        request["questions"][0]["options"],
        json!(["sqlite", "postgres"])
    );
    assert!(
        harness.context.pending.user_input(&request_id).is_some(),
        "the question must be indexed for tool/requestUserInput/respond"
    );

    let response = harness
        .call(
            4,
            "tool/requestUserInput/respond",
            json!({
                "requestId": request_id,
                "action": "answered",
                "expectedCreatedAtMs": request["createdAtMs"],
                "answers": [{
                    "question": "Which database should I target?",
                    "answer": "sqlite"
                }],
            }),
        )
        .await;
    assert!(response.error.is_none(), "answer failed: {response:?}");
    assert_eq!(response.result.unwrap()["resolved"], json!(true));
    assert!(
        harness.context.pending.user_input(&request_id).is_none(),
        "an answered question must leave the index"
    );

    let rest = harness
        .drain_until(|frame| Harness::frame_kind(frame) == "turn_completed")
        .await;
    let kinds: Vec<&str> = rest.iter().map(Harness::frame_kind).collect();
    assert!(kinds.contains(&"user_input_resolved"), "{kinds:?}");
    assert!(
        kinds.contains(&"tool_completed"),
        "the answer must come back as the tool result: {kinds:?}"
    );
    assert_eq!(deltas_of(&rest), vec!["using sqlite"]);

    harness.wait_for_idle().await;
    assert_eq!(harness.provider.requests().len(), 2);
}

/// 结构化输入的两个失败路径必须被 `dispatch` 拒绝，而不是把运行中的 Turn 卡死。
#[tokio::test(flavor = "multi_thread")]
async fn user_input_respond_rejects_an_empty_answer() {
    let harness = Harness::start(FakeProvider::text(&["unused"]), ToolRegistry::read_only()).await;
    harness.connect_and_subscribe().await;

    let response = harness
        .call(
            3,
            "tool/requestUserInput/respond",
            json!({ "requestId": "does-not-exist", "action": "answered", "answers": [] }),
        )
        .await;
    let error = response.error.expect("an empty answer must be rejected");
    assert_eq!(error.kind(), "invalid_params");
}
