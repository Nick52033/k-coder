use std::io::Read;
use std::sync::{Arc, Mutex};

use k_coder_lib::agent::{AgentRuntime, EventPublisher, RunTurnRequest};
use k_coder_lib::protocol::{AgentEvent, AgentEventEnvelope, ToolCall, TurnState};
use k_coder_lib::providers::testing::FakeProvider;
use k_coder_lib::providers::{ProviderEvent, ProviderMessage};
use k_coder_lib::storage::JsonlThreadRepository;
use k_coder_lib::tools::ToolRegistry;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const REPLY: &str = "k-Coder 最小验证通过：真实 AgentRuntime 已读取隔离测试文件并完成回复（本地测试 Provider，未访问项目、未调用外部模型）。";

#[derive(Default)]
struct RecordingPublisher(Mutex<Vec<AgentEventEnvelope>>);

impl EventPublisher for RecordingPublisher {
    fn publish(&self, event: AgentEventEnvelope) {
        self.0.lock().unwrap().push(event);
    }
}

fn valid_challenge(input: &str) -> bool {
    input.strip_prefix("kcoder-").is_some_and(|suffix| {
        suffix.len() == 12 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

async fn run_smoke(input: &str) -> Value {
    assert!(valid_challenge(input), "invalid smoke challenge");
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("smoke.txt"), "weixin-smoke-fixture-ok").unwrap();
    let repository = Arc::new(JsonlThreadRepository::new(directory.path().join("events")).unwrap());
    let thread = repository.create_thread().await.unwrap();
    let runtime =
        AgentRuntime::with_tools(repository.clone(), ToolRegistry::read_only(), workspace)
            .with_provider_call_budget(3);
    let provider = Arc::new(FakeProvider::script(vec![
        vec![
            Ok(ProviderEvent::ToolCall {
                call: ToolCall {
                    id: "weixin-smoke-read".into(),
                    name: "read_file".into(),
                    arguments: json!({"path": "smoke.txt"}),
                    metadata: json!({}),
                },
            }),
            Ok(ProviderEvent::Completed),
        ],
        vec![
            Ok(ProviderEvent::TextDelta {
                delta: REPLY.into(),
            }),
            Ok(ProviderEvent::Completed),
        ],
    ]));
    let publisher = Arc::new(RecordingPublisher::default());
    let outcome = runtime
        .run_turn(
            provider.clone(),
            "local-weixin-smoke-fixture".into(),
            RunTurnRequest {
                thread_id: thread.id.clone(),
                input: input.into(),
                agent_mode: Some("ask".into()),
            },
            CancellationToken::new(),
            publisher.clone(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.state, TurnState::Completed);
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].messages.iter().any(|message| matches!(
        message,
        ProviderMessage::ToolResult { name, success: true, output, .. }
            if name == "read_file" && output.contains("weixin-smoke-fixture-ok")
    )));
    let detail = repository.read_thread(&thread.id).await.unwrap();
    assert_eq!(detail.messages.first().unwrap().text(), input);
    let reply = detail.messages.last().unwrap().text();
    assert_eq!(reply, REPLY);
    let events = publisher.0.lock().unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.event,
        AgentEvent::TurnCompleted { message, .. } if message.text() == REPLY
    )));
    json!({"state": "completed", "reply": reply, "providerCalls": requests.len(), "tools": 1})
}

#[test]
fn smoke_challenge_rejects_general_commands() {
    assert!(valid_challenge("kcoder-012345abcdef"));
    for input in [
        "",
        "delete files",
        "kcoder-../secret",
        "kcoder-012345abcdef\n",
        "kcoder-012345abcdef-more",
    ] {
        assert!(!valid_challenge(input));
    }
}

#[tokio::test]
async fn smoke_uses_existing_runtime_and_read_only_tool() {
    assert_eq!(run_smoke("kcoder-012345abcdef").await["tools"], 1);
}

#[tokio::test]
#[ignore = "run only through scripts/validate-weixin-smoke.mjs after user QR authorization"]
async fn live_weixin_runtime_once() {
    let mut input = String::new();
    std::io::stdin()
        .take(128)
        .read_to_string(&mut input)
        .unwrap();
    let request: Value = serde_json::from_str(&input).unwrap();
    let result = run_smoke(request["input"].as_str().unwrap()).await;
    println!(
        "KC_WEIXIN_SMOKE_RESULT {}",
        serde_json::to_string(&result).unwrap()
    );
}
