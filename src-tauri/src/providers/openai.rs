use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::common::{classify_event_error, read_error_message, redact_error, redact_event};
use super::sse::SseDecoder;
use super::{
    Provider, ProviderConfig, ProviderError, ProviderEvent, ProviderMessage, ProviderRequest,
    ProviderStream,
};
use crate::protocol::{MessageRole, ReasoningEffort, TokenUsage, ToolCall};

const OPENAI_REQUEST_TIMEOUT_SECONDS: u64 = 120;
const DEEPSEEK_REQUEST_TIMEOUT_SECONDS: u64 = 300;
const MAX_DEEPSEEK_REASONING_BYTES: usize = 2 * 1024 * 1024;
const MAX_DEEPSEEK_REASONING_CACHE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEEPSEEK_CACHED_TOOL_CALLS: usize = 512;
const LEGACY_DEEPSEEK_TOOL_CALLS_MARKER: &str = "[Historical tool calls]";
const LEGACY_DEEPSEEK_TOOL_RESULT_MARKER: &str = "[Historical tool result for ";
const DEEPSEEK_RETRY_CONTINUATION: &str = "Continue from the preceding observations. If the requested work is already applied and sufficiently verified, return the final answer now. Call another tool only for a specific unresolved fact; do not reread unchanged overlapping file ranges merely to reconfirm them.";

#[derive(Clone, PartialEq, Eq)]
struct CachedDeepSeekReasoning {
    content: Arc<str>,
    thinking_enabled: bool,
}

#[derive(Default)]
struct DeepSeekReasoningCache {
    by_call_id: HashMap<String, CachedDeepSeekReasoning>,
    reasoning_bytes: usize,
    /// Set after a compatible endpoint rejects an otherwise valid-looking
    /// private passback.  The endpoint has demonstrated that its
    /// `reasoning_content` is not replayable, so future thinking tool rounds
    /// must use the safe observation representation until this Provider is
    /// rebuilt.
    force_degraded: bool,
}

#[derive(Clone, Default)]
struct DeepSeekState {
    // DeepSeek requires CoT passback for a thinking-mode tool round. Keep it
    // bounded and process-local so private reasoning never becomes a domain event.
    cache: Arc<Mutex<DeepSeekReasoningCache>>,
}

impl DeepSeekState {
    fn force_degraded(&self) -> Result<(), ProviderError> {
        let mut cache = self.cache.lock().map_err(|_| {
            ProviderError::Request("DeepSeek reasoning cache is unavailable".to_string())
        })?;
        cache.force_degraded = true;
        // Do not retain private reasoning after the endpoint rejected it.
        cache.by_call_id.clear();
        cache.reasoning_bytes = 0;
        Ok(())
    }

    fn is_force_degraded(&self) -> Result<bool, ProviderError> {
        let cache = self.cache.lock().map_err(|_| {
            ProviderError::Request("DeepSeek reasoning cache is unavailable".to_string())
        })?;
        Ok(cache.force_degraded)
    }

    fn remember(
        &self,
        calls: &[ToolCall],
        reasoning: &str,
        thinking_enabled: bool,
    ) -> Result<(), ProviderError> {
        if calls.is_empty() {
            return Ok(());
        }
        let mut cache = self.cache.lock().map_err(|_| {
            ProviderError::Request("DeepSeek reasoning cache is unavailable".to_string())
        })?;
        let new_call_count = calls
            .iter()
            .filter(|call| !cache.by_call_id.contains_key(&call.id))
            .count();
        if cache.by_call_id.len().saturating_add(new_call_count) > MAX_DEEPSEEK_CACHED_TOOL_CALLS {
            return Err(ProviderError::InvalidResponse(format!(
                "DeepSeek returned more than {MAX_DEEPSEEK_CACHED_TOOL_CALLS} tool calls with private reasoning in one turn"
            )));
        }
        if new_call_count > 0
            && cache.reasoning_bytes.saturating_add(reasoning.len())
                > MAX_DEEPSEEK_REASONING_CACHE_BYTES
        {
            return Err(ProviderError::InvalidResponse(format!(
                "DeepSeek private reasoning cache exceeded {MAX_DEEPSEEK_REASONING_CACHE_BYTES} bytes"
            )));
        }

        let content: Arc<str> = Arc::from(reasoning);
        if new_call_count > 0 {
            cache.reasoning_bytes = cache.reasoning_bytes.saturating_add(reasoning.len());
        }
        for call in calls {
            cache.by_call_id.insert(
                call.id.clone(),
                CachedDeepSeekReasoning {
                    content: content.clone(),
                    thinking_enabled,
                },
            );
        }
        Ok(())
    }

    fn reasoning_for_calls(
        &self,
        calls: &[ToolCall],
    ) -> Result<Option<CachedDeepSeekReasoning>, ProviderError> {
        let cache = self.cache.lock().map_err(|_| {
            ProviderError::Request("DeepSeek reasoning cache is unavailable".to_string())
        })?;
        let Some(first) = calls
            .first()
            .and_then(|call| cache.by_call_id.get(&call.id))
            .cloned()
        else {
            return Ok(None);
        };
        Ok(calls
            .iter()
            .all(|call| cache.by_call_id.get(&call.id) == Some(&first))
            .then_some(first))
    }
}

#[derive(Clone)]
enum ChatCompletionsDialect {
    OpenAi,
    DeepSeek(DeepSeekState),
}

pub struct OpenAiChatCompletionsProvider {
    client: Client,
    config: ProviderConfig,
    api_key: String,
    dialect: ChatCompletionsDialect,
}

impl OpenAiChatCompletionsProvider {
    pub fn new(config: ProviderConfig, api_key: String) -> Result<Self, ProviderError> {
        Self::new_with_dialect(config, api_key, ChatCompletionsDialect::OpenAi)
    }

    fn new_with_dialect(
        config: ProviderConfig,
        api_key: String,
        dialect: ChatCompletionsDialect,
    ) -> Result<Self, ProviderError> {
        if api_key.trim().is_empty() {
            return Err(ProviderError::Request(
                "API key is not configured".to_string(),
            ));
        }
        let config = config
            .validate()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        let timeout_seconds = match dialect {
            ChatCompletionsDialect::OpenAi => OPENAI_REQUEST_TIMEOUT_SECONDS,
            ChatCompletionsDialect::DeepSeek(_) => DEEPSEEK_REQUEST_TIMEOUT_SECONDS,
        };
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(timeout_seconds))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        Ok(Self {
            client,
            config,
            api_key,
            dialect,
        })
    }

    fn payload(&self, request: &ProviderRequest) -> Result<Value, ProviderError> {
        let deepseek_thinking_enabled =
            matches!(&self.dialect, ChatCompletionsDialect::DeepSeek(_))
                && deepseek_reasoning(request.reasoning_effort).0;
        let messages = match &self.dialect {
            ChatCompletionsDialect::OpenAi => chat_messages(&request.messages),
            ChatCompletionsDialect::DeepSeek(state) => {
                deepseek_chat_messages(&request.messages, state, deepseek_thinking_enabled)?
            }
        };
        let mut payload = json!({
            "model": request.model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        match &self.dialect {
            ChatCompletionsDialect::OpenAi => {
                if let Some(effort) = request.reasoning_effort.openai_value() {
                    payload["reasoning_effort"] = json!(effort);
                }
            }
            ChatCompletionsDialect::DeepSeek(_) => {
                let (thinking_enabled, effort) = deepseek_reasoning(request.reasoning_effort);
                payload["thinking"] = json!({
                    "type": if thinking_enabled { "enabled" } else { "disabled" }
                });
                if let Some(effort) = effort {
                    payload["reasoning_effort"] = json!(effort);
                }
                if let Some(max_tokens) = self.config.active_model().max_output_tokens {
                    payload["max_tokens"] = json!(max_tokens);
                }
            }
        }
        if !request.tools.is_empty() {
            payload["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.input_schema
                            }
                        })
                    })
                    .collect(),
            );
        }
        Ok(payload)
    }

    async fn send_request(
        &self,
        endpoint: reqwest::Url,
        payload: &Value,
        cancellation: &CancellationToken,
    ) -> Result<reqwest::Response, ProviderError> {
        tokio::select! {
            _ = cancellation.cancelled() => Err(ProviderError::Cancelled),
            response = self.client
                .post(endpoint)
                .bearer_auth(&self.api_key)
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .json(payload)
                .send() => response.map_err(|error| ProviderError::Request(error.to_string())),
        }
    }
}

pub struct DeepSeekChatCompletionsProvider {
    inner: OpenAiChatCompletionsProvider,
}

impl DeepSeekChatCompletionsProvider {
    pub fn new(config: ProviderConfig, api_key: String) -> Result<Self, ProviderError> {
        Ok(Self {
            inner: OpenAiChatCompletionsProvider::new_with_dialect(
                config,
                api_key,
                ChatCompletionsDialect::DeepSeek(DeepSeekState::default()),
            )?,
        })
    }
}

#[derive(Deserialize)]
struct OpenAiChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
    error: Option<OpenAiError>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    #[serde(default)]
    delta: OpenAiDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiToolCallDelta>,
}

#[derive(Deserialize)]
struct OpenAiToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Deserialize)]
struct OpenAiFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct OpenAiError {
    message: String,
    code: Option<String>,
    #[serde(rename = "type")]
    error_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameCompletion {
    None,
    FinishReason,
    DoneMarker,
}

struct ParsedSseData {
    events: Vec<ProviderEvent>,
    reasoning_deltas: Vec<String>,
    tool_deltas: Vec<OpenAiToolCallDelta>,
    completion: FrameCompletion,
    terminal_error: Option<ProviderError>,
}

#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct ToolCallAccumulator {
    calls: BTreeMap<usize, PendingToolCall>,
}

impl ToolCallAccumulator {
    fn push(&mut self, delta: OpenAiToolCallDelta) {
        let pending = self.calls.entry(delta.index).or_default();
        if let Some(id) = delta.id {
            pending.id = id;
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name {
                pending.name.push_str(&name);
            }
            if let Some(arguments) = function.arguments {
                pending.arguments.push_str(&arguments);
            }
        }
    }

    fn take(&mut self) -> Result<Vec<ToolCall>, ProviderError> {
        std::mem::take(&mut self.calls)
            .into_values()
            .map(|pending| {
                if pending.id.is_empty() || pending.name.is_empty() {
                    return Err(ProviderError::InvalidResponse(
                        format!(
                            "Chat Completions returned an incomplete tool call (id: '{}', name: '{}', arguments: '{}')",
                            pending.id, pending.name, pending.arguments
                        ),
                    ));
                }
                let arguments = serde_json::from_str(&pending.arguments).map_err(|error| {
                    ProviderError::InvalidResponse(format!(
                        "tool call {} returned invalid JSON arguments: {error}\nRaw arguments: {}",
                        pending.name, pending.arguments
                    ))
                })?;
                Ok(ToolCall {
                    id: pending.id,
                    name: pending.name,
                    arguments,
                    metadata: json!({}),
                })
            })
            .collect()
    }
}

#[async_trait]
impl Provider for OpenAiChatCompletionsProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        let endpoint = self
            .config
            .chat_completions_url()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        let mut payload = self.payload(&request)?;
        let deepseek = match &self.dialect {
            ChatCompletionsDialect::OpenAi => None,
            ChatCompletionsDialect::DeepSeek(state) => Some((
                state.clone(),
                deepseek_reasoning(request.reasoning_effort).0,
            )),
        };

        let mut response = self
            .send_request(endpoint.clone(), &payload, &cancellation)
            .await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = read_error_message(response, &cancellation, &self.api_key).await?;
            let can_retry_with_degraded_history =
                deepseek.as_ref().is_some_and(|(_, thinking_enabled)| {
                    *thinking_enabled
                        && has_native_deepseek_tool_history(&payload)
                        && is_deepseek_reasoning_passback_error(&message)
                        && status == 400
                });
            if can_retry_with_degraded_history {
                let (state, _) = deepseek
                    .as_ref()
                    .expect("DeepSeek state must exist for compatibility retry");
                state.force_degraded()?;
                payload = self.payload(&request)?;
                response = self
                    .send_request(endpoint.clone(), &payload, &cancellation)
                    .await?;
                if !response.status().is_success() {
                    let retry_status = response.status().as_u16();
                    let retry_message =
                        read_error_message(response, &cancellation, &self.api_key).await?;
                    return Err(ProviderError::Http {
                        status: retry_status,
                        message: retry_message,
                    });
                }
            } else {
                return Err(ProviderError::Http { status, message });
            }
        }

        let secret = self.api_key.clone();
        Ok(Box::pin(async_stream::stream! {
            let mut body = response.bytes_stream();
            let mut decoder = SseDecoder::default();
            let mut tool_calls = ToolCallAccumulator::default();
            let mut deepseek_reasoning_content = String::new();
            let mut saw_finish_reason = false;

            loop {
                let chunk = tokio::select! {
                    _ = cancellation.cancelled() => {
                        yield Err(ProviderError::Cancelled);
                        return;
                    }
                    chunk = body.next() => chunk,
                };
                match chunk {
                    Some(Ok(bytes)) => match decoder.push(&bytes) {
                        Ok(frames) => {
                            for frame in frames {
                                match parse_sse_data(&frame) {
                                    Ok(parsed) => {
                                        for delta in parsed.tool_deltas { tool_calls.push(delta); }
                                        if deepseek.is_some() {
                                            for delta in parsed.reasoning_deltas {
                                                if deepseek_reasoning_content.len().saturating_add(delta.len()) > MAX_DEEPSEEK_REASONING_BYTES {
                                                    yield Err(ProviderError::InvalidResponse(format!(
                                                        "DeepSeek private reasoning exceeded {MAX_DEEPSEEK_REASONING_BYTES} bytes"
                                                    )));
                                                    return;
                                                }
                                                deepseek_reasoning_content.push_str(&delta);
                                            }
                                        }
                                        for event in parsed.events.into_iter().map(|event| redact_event(event, &secret)) { yield Ok(event); }
                                        if let Some(error) = parsed.terminal_error {
                                            yield Err(redact_error(error, &secret));
                                            return;
                                        }
                                        if parsed.completion == FrameCompletion::FinishReason {
                                            saw_finish_reason = true;
                                            match tool_calls.take() {
                                                Ok(calls) => {
                                                    if let Some((state, thinking_enabled)) = &deepseek {
                                                        if let Err(error) = state.remember(&calls, &deepseek_reasoning_content, *thinking_enabled) {
                                                            yield Err(error);
                                                            return;
                                                        }
                                                    }
                                                    for call in calls { yield Ok(ProviderEvent::ToolCall { call }); }
                                                },
                                                Err(error) => { yield Err(error); return; }
                                            }
                                        }
                                        if parsed.completion == FrameCompletion::DoneMarker {
                                            match tool_calls.take() {
                                                Ok(calls) => {
                                                    if let Some((state, thinking_enabled)) = &deepseek {
                                                        if let Err(error) = state.remember(&calls, &deepseek_reasoning_content, *thinking_enabled) {
                                                            yield Err(error);
                                                            return;
                                                        }
                                                    }
                                                    for call in calls { yield Ok(ProviderEvent::ToolCall { call }); }
                                                },
                                                Err(error) => { yield Err(error); return; }
                                            }
                                            yield Ok(ProviderEvent::Completed);
                                            return;
                                        }
                                    }
                                    Err(error) => { yield Err(redact_error(error, &secret)); return; }
                                }
                            }
                        }
                        Err(error) => { yield Err(error); return; }
                    },
                    Some(Err(error)) => {
                        yield Err(redact_error(ProviderError::Request(error.to_string()), &secret));
                        return;
                    }
                    None => {
                        if let Err(error) = decoder.finish() { yield Err(error); return; }
                        if saw_finish_reason {
                            match tool_calls.take() {
                                Ok(calls) => for call in calls { yield Ok(ProviderEvent::ToolCall { call }); },
                                Err(error) => { yield Err(error); return; }
                            }
                            yield Ok(ProviderEvent::Completed);
                        } else {
                            yield Err(ProviderError::Interrupted);
                        }
                        return;
                    }
                }
            }
        }))
    }
}

#[async_trait]
impl Provider for DeepSeekChatCompletionsProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        self.inner.stream(request, cancellation).await
    }
}

fn chat_messages(messages: &[ProviderMessage]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| match message {
            ProviderMessage::Text { role, text } => Some(json!({
                "role": match role { MessageRole::User => "user", MessageRole::Assistant => "assistant", MessageRole::System => "system" },
                "content": text
            })),
            ProviderMessage::UserContent { text, images } => Some(json!({
                "role": "user",
                "content": std::iter::once(json!({ "type": "text", "text": text }))
                    .chain(images.iter().map(|image| json!({
                        "type": "image_url",
                        "image_url": { "url": image.data_url }
                    })))
                    .collect::<Vec<_>>()
            })),
            ProviderMessage::AssistantToolCalls { text, calls } => Some(json!({
                "role": "assistant",
                "content": if text.is_empty() { Value::Null } else { Value::String(text.clone()) },
                "tool_calls": calls.iter().map(|call| json!({
                    "id": call.id,
                    "type": "function",
                    "function": { "name": call.name, "arguments": call.arguments.to_string() }
                })).collect::<Vec<_>>()
            })),
            ProviderMessage::ToolResult { call_id, output, .. } => Some(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": output
            })),
            ProviderMessage::ProviderContext { .. } => None,
        })
        .collect()
}

fn deepseek_reasoning(effort: ReasoningEffort) -> (bool, Option<&'static str>) {
    match effort {
        ReasoningEffort::Off => (false, None),
        ReasoningEffort::Minimal | ReasoningEffort::Low => (true, Some("low")),
        ReasoningEffort::Medium | ReasoningEffort::High => (true, Some("high")),
        ReasoningEffort::XHigh => (true, Some("max")),
    }
}

/// Return whether a request contains the native assistant/tool pair that can
/// trigger DeepSeek's private-CoT passback validation.  The compatibility
/// retry is deliberately narrower than a generic 400 retry: it must have a
/// concrete structured tool history to replace.
fn has_native_deepseek_tool_history(payload: &Value) -> bool {
    payload
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message.get("role").and_then(Value::as_str) == Some("assistant")
                    && message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|calls| !calls.is_empty())
            })
        })
}

/// DeepSeek-compatible gateways use several phrasings for the same protocol
/// violation.  Match only the private field plus a passback/thinking hint so
/// unrelated invalid requests remain non-retryable.
fn is_deepseek_reasoning_passback_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("reasoning_content")
        && (normalized.contains("must be passed back")
            || normalized.contains("must pass back")
            || normalized.contains("pass back to the api")
            || normalized.contains("thinking mode"))
}

fn legacy_deepseek_artifacts(messages: &[ProviderMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            ProviderMessage::AssistantToolCalls { calls, .. } if !calls.is_empty() => {
                let mut artifact = LEGACY_DEEPSEEK_TOOL_CALLS_MARKER.to_string();
                for call in calls {
                    artifact.push_str(&format!(
                        "\n- {} ({}) arguments: {}",
                        call.name, call.id, call.arguments
                    ));
                }
                Some(artifact)
            }
            ProviderMessage::ToolResult {
                call_id,
                name,
                success,
                output,
            } => Some(format!(
                "{LEGACY_DEEPSEEK_TOOL_RESULT_MARKER}{name} ({call_id}); status: {}]\n{output}",
                if *success { "success" } else { "failed" }
            )),
            _ => None,
        })
        .collect()
}

fn sanitize_deepseek_assistant_text(text: &str, legacy_artifacts: &[String]) -> String {
    let cutoff = legacy_artifacts
        .iter()
        .filter_map(|artifact| text.strip_suffix(artifact).map(str::len))
        .filter(|index| *index == 0 || text.as_bytes().get(index - 1) == Some(&b'\n'))
        .min();
    cutoff
        .map(|index| text[..index].trim_end().to_string())
        .unwrap_or_else(|| text.to_string())
}

fn historical_tool_observation(name: &str, success: bool, output: &str) -> String {
    if name == "request_user_input" {
        if success {
            return if output.trim().is_empty() {
                "The user previously completed a clarification request without additional text."
                    .to_string()
            } else {
                format!("The user previously clarified:\n{output}")
            };
        }
        return if output.trim().is_empty() {
            "An earlier clarification request was not completed.".to_string()
        } else {
            format!("An earlier clarification request was not completed:\n{output}")
        };
    }

    match (success, output.trim().is_empty()) {
        (true, true) => format!("An earlier {name} operation completed without a textual result."),
        (true, false) => format!("Earlier information from {name}:\n{output}"),
        (false, true) => format!("An earlier {name} operation failed without a textual result."),
        (false, false) => format!("An earlier {name} operation failed:\n{output}"),
    }
}

fn push_plain_assistant_context(wire: &mut Vec<Value>, text: String) {
    if text.trim().is_empty() {
        return;
    }
    if let Some(object) = wire.last_mut().and_then(Value::as_object_mut) {
        let can_merge = object.len() == 2
            && object.get("role").and_then(Value::as_str) == Some("assistant")
            && matches!(object.get("content"), Some(Value::String(_)));
        if can_merge {
            if let Some(Value::String(content)) = object.get_mut("content") {
                if !content.is_empty() {
                    content.push_str("\n\n");
                }
                content.push_str(&text);
                return;
            }
        }
    }
    wire.push(json!({ "role": "assistant", "content": text }));
}

fn deepseek_chat_messages(
    messages: &[ProviderMessage],
    state: &DeepSeekState,
    thinking_enabled: bool,
) -> Result<Vec<Value>, ProviderError> {
    let mut wire = Vec::new();
    let mut native_call_ids = HashSet::new();
    let mut degraded_tool_history = false;
    let force_degraded = thinking_enabled && state.is_force_degraded()?;
    let legacy_artifacts = legacy_deepseek_artifacts(messages);
    for message in messages {
        match message {
            ProviderMessage::Text { role, text } => {
                if matches!(role, MessageRole::Assistant) {
                    push_plain_assistant_context(
                        &mut wire,
                        sanitize_deepseek_assistant_text(text, &legacy_artifacts),
                    );
                } else {
                    wire.push(json!({
                        "role": match role { MessageRole::User => "user", MessageRole::Assistant => "assistant", MessageRole::System => "system" },
                        "content": text
                    }));
                }
            }
            ProviderMessage::UserContent { text, images } => {
                if !images.is_empty() {
                    return Err(ProviderError::Request(
                        "DeepSeek Chat Completions does not support image content".to_string(),
                    ));
                }
                wire.push(json!({ "role": "user", "content": text }));
            }
            ProviderMessage::AssistantToolCalls { text, calls } => {
                let text = sanitize_deepseek_assistant_text(text, &legacy_artifacts);
                if calls.is_empty() {
                    // A repaired history should never contain this shape, but
                    // keep malformed/legacy empty call records as plain text
                    // instead of manufacturing a continuation boundary.
                    push_plain_assistant_context(&mut wire, text);
                    continue;
                }
                let reasoning = if !thinking_enabled {
                    // Disabled thinking has no private passback requirement;
                    // retain the native tool protocol even after a restart.
                    Some(CachedDeepSeekReasoning {
                        content: Arc::from(""),
                        thinking_enabled: false,
                    })
                } else if force_degraded {
                    None
                } else {
                    state.reasoning_for_calls(calls)?.filter(|reasoning| {
                        // A cache entry collected while thinking was disabled
                        // cannot satisfy a later enabled-thinking request.
                        // Likewise, whitespace-only data is only a placeholder,
                        // not the original private chain required for passback.
                        reasoning.thinking_enabled && !reasoning.content.trim().is_empty()
                    })
                };
                match reasoning {
                    Some(reasoning) => {
                        native_call_ids.extend(calls.iter().map(|call| call.id.clone()));
                        let mut assistant = json!({
                            "role": "assistant",
                            "content": text,
                            "tool_calls": calls.iter().map(|call| json!({
                                "id": call.id,
                                "type": "function",
                                "function": { "name": call.name, "arguments": call.arguments.to_string() }
                            })).collect::<Vec<_>>()
                        });
                        if reasoning.thinking_enabled {
                            assistant["reasoning_content"] = json!(reasoning.content.as_ref());
                        }
                        wire.push(assistant);
                    }
                    _ => {
                        // After restart or in a later Turn the private passback is gone.
                        // Some compatible endpoints also count private reasoning tokens
                        // without returning the reasoning_content needed for passback.
                        // Preserve only natural-language progress and observations;
                        // raw call metadata is model-visible and can be echoed.
                        degraded_tool_history = true;
                        push_plain_assistant_context(&mut wire, text);
                    }
                }
            }
            ProviderMessage::ToolResult {
                call_id,
                name: _,
                success: _,
                output,
            } if native_call_ids.contains(call_id) => wire.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": if output.is_empty() { "(no output)" } else { output }
            })),
            ProviderMessage::ToolResult {
                call_id: _,
                name,
                success,
                output,
            } => push_plain_assistant_context(
                &mut wire,
                historical_tool_observation(name, *success, output),
            ),
            ProviderMessage::ProviderContext { .. } => {}
        }
    }
    if degraded_tool_history
        && wire.last().and_then(|message| message["role"].as_str()) == Some("assistant")
    {
        // A retry has no new persisted user message. DeepSeek otherwise treats a
        // trailing assistant observation as an unfinished thinking response and
        // requires the private reasoning_content that was intentionally discarded.
        wire.push(json!({
            "role": "user",
            "content": DEEPSEEK_RETRY_CONTINUATION
        }));
    }
    Ok(wire)
}

fn parse_sse_data(data: &str) -> Result<ParsedSseData, ProviderError> {
    if data.trim() == "[DONE]" {
        return Ok(ParsedSseData {
            events: Vec::new(),
            reasoning_deltas: Vec::new(),
            tool_deltas: Vec::new(),
            completion: FrameCompletion::DoneMarker,
            terminal_error: None,
        });
    }
    let chunk: OpenAiChunk = serde_json::from_str(data).map_err(|error| {
        ProviderError::InvalidResponse(format!("malformed Chat Completions event: {error}"))
    })?;
    if let Some(error) = chunk.error {
        return Err(classify_event_error(
            error.message,
            error.code.as_deref(),
            error.error_type.as_deref(),
        ));
    }
    let finish_reason = chunk
        .choices
        .iter()
        .find_map(|choice| choice.finish_reason.clone());
    let completion = if finish_reason.is_some() {
        FrameCompletion::FinishReason
    } else {
        FrameCompletion::None
    };
    let mut events = Vec::new();
    let mut reasoning_deltas = Vec::new();
    let mut tool_deltas = Vec::new();
    for choice in chunk.choices {
        if let Some(delta) = choice
            .delta
            .reasoning_content
            .filter(|value| !value.is_empty())
        {
            reasoning_deltas.push(delta);
        }
        if let Some(delta) = choice.delta.content.filter(|value| !value.is_empty()) {
            events.push(ProviderEvent::TextDelta { delta });
        }
        tool_deltas.extend(choice.delta.tool_calls);
    }
    if let Some(usage) = chunk.usage {
        events.push(ProviderEvent::Usage {
            usage: TokenUsage {
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
                total_tokens: usage
                    .total_tokens
                    .unwrap_or(usage.prompt_tokens + usage.completion_tokens),
            },
        });
    }
    Ok(ParsedSseData {
        events,
        reasoning_deltas,
        tool_deltas,
        completion,
        terminal_error: finish_reason
            .as_deref()
            .filter(|reason| !matches!(*reason, "stop" | "tool_calls" | "function_call"))
            .map(|reason| {
                ProviderError::InvalidResponse(format!(
                    "Chat Completions stopped generation with finish_reason {reason}"
                ))
            }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PROTOCOL_VERSION;
    use crate::providers::{ProviderImage, ProviderKind, ProviderModelConfig, ProviderTransport};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;

    fn provider_config(model: &str) -> ProviderConfig {
        ProviderConfig {
            schema_version: PROTOCOL_VERSION,
            id: "test".to_string(),
            kind: ProviderKind::OpenAiCompatible,
            transport: ProviderTransport::DeepSeekChatCompletions,
            name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com".to_string(),
            model: model.to_string(),
            models: Vec::new(),
            endpoints: Vec::new(),
        }
    }

    fn provider_request(
        reasoning_effort: ReasoningEffort,
        messages: Vec<ProviderMessage>,
    ) -> ProviderRequest {
        ProviderRequest {
            schema_version: PROTOCOL_VERSION,
            model: "deepseek-v4-pro-0813".to_string(),
            reasoning_effort,
            messages,
            tools: Vec::new(),
        }
    }

    fn tool_call(id: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "read_file".to_string(),
            arguments: json!({ "path": "README.md" }),
            metadata: json!({}),
        }
    }

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let header_end = loop {
            let mut chunk = [0_u8; 4096];
            let read = stream.read(&mut chunk).await.expect("request should read");
            assert!(read > 0, "request ended before its headers");
            request.extend_from_slice(&chunk[..read]);
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers =
            std::str::from_utf8(&request[..header_end]).expect("request headers should be UTF-8");
        let content_length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("valid content length"))
            })
            .expect("request should include a content length");
        while request.len() < header_end + content_length {
            let mut chunk = [0_u8; 4096];
            let read = stream
                .read(&mut chunk)
                .await
                .expect("request body should read");
            assert!(read > 0, "request ended before its body");
            request.extend_from_slice(&chunk[..read]);
        }
        serde_json::from_slice(&request[header_end..header_end + content_length])
            .expect("request body should be JSON")
    }

    async fn spawn_sse_server(
        responses: Vec<&'static str>,
    ) -> (String, mpsc::Receiver<Value>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let address = listener
            .local_addr()
            .expect("test server should have an address");
        let (requests, received) = mpsc::channel(responses.len());
        let server = tokio::spawn(async move {
            for body in responses {
                let (mut stream, _) = listener.accept().await.expect("request should connect");
                requests
                    .send(read_json_request(&mut stream).await)
                    .await
                    .expect("request receiver should remain open");
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("response should write");
            }
        });
        (format!("http://{address}"), received, server)
    }

    struct MockHttpResponse {
        status: u16,
        content_type: &'static str,
        body: &'static str,
    }

    async fn spawn_http_server(
        responses: Vec<MockHttpResponse>,
    ) -> (String, mpsc::Receiver<Value>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let address = listener
            .local_addr()
            .expect("test server should have an address");
        let (requests, received) = mpsc::channel(responses.len());
        let server = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.expect("request should connect");
                requests
                    .send(read_json_request(&mut stream).await)
                    .await
                    .expect("request receiver should remain open");
                let status_text = match response.status {
                    200 => "OK",
                    400 => "Bad Request",
                    401 => "Unauthorized",
                    500 => "Internal Server Error",
                    _ => "Mock Response",
                };
                let wire = format!(
                    "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response.status,
                    status_text,
                    response.content_type,
                    response.body.len(),
                    response.body,
                );
                stream
                    .write_all(wire.as_bytes())
                    .await
                    .expect("response should write");
            }
        });
        (format!("http://{address}"), received, server)
    }

    #[test]
    fn parses_text_usage_and_fragmented_tool_calls() {
        let text = parse_sse_data(r#"{"choices":[{"delta":{"content":"hello"}}]}"#).unwrap();
        assert!(matches!(&text.events[0], ProviderEvent::TextDelta { delta } if delta == "hello"));
        let first = parse_sse_data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}"#).unwrap();
        let second = parse_sse_data(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"README.md\"}"}}]},"finish_reason":"tool_calls"}]}"#).unwrap();
        let mut accumulator = ToolCallAccumulator::default();
        for delta in first.tool_deltas.into_iter().chain(second.tool_deltas) {
            accumulator.push(delta);
        }
        let calls = accumulator.take().unwrap();
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments, json!({ "path": "README.md" }));
    }

    #[test]
    fn classifies_streamed_server_errors_as_temporary_unavailability() {
        let error = parse_sse_data(
            r#"{"error":{"message":"Our servers are currently overloaded. Please try again later.","type":"server_error","code":"server_error"}}"#,
        )
        .err()
        .expect("the error event should fail the stream");
        assert!(matches!(
            error,
            ProviderError::Unavailable(message) if message.contains("currently overloaded")
        ));

        let invalid = parse_sse_data(
            r#"{"error":{"message":"model does not exist","type":"invalid_request_error","code":"model_not_found"}}"#,
        )
        .err()
        .expect("the error event should fail the stream");
        assert!(matches!(invalid, ProviderError::InvalidResponse(_)));
    }

    #[test]
    fn rejects_incomplete_tool_call_arguments() {
        let mut accumulator = ToolCallAccumulator::default();
        accumulator.push(OpenAiToolCallDelta {
            index: 0,
            id: Some("call".to_string()),
            function: Some(OpenAiFunctionDelta {
                name: Some("read_file".to_string()),
                arguments: Some("{".to_string()),
            }),
        });
        assert!(matches!(
            accumulator.take(),
            Err(ProviderError::InvalidResponse(_))
        ));
    }

    #[test]
    fn length_finish_reason_is_not_a_successful_completion() {
        let parsed = parse_sse_data(
            r#"{"choices":[{"delta":{"content":"partial"},"finish_reason":"length"}],"usage":{"prompt_tokens":4,"completion_tokens":3}}"#,
        )
        .unwrap();
        assert_eq!(parsed.completion, FrameCompletion::FinishReason);
        assert!(matches!(
            parsed.events.last(),
            Some(ProviderEvent::Usage { usage }) if usage.total_tokens == 7
        ));
        assert!(matches!(
            parsed.terminal_error,
            Some(ProviderError::InvalidResponse(message)) if message.contains("length")
        ));
    }

    #[test]
    fn serializes_structured_tool_history() {
        let messages = chat_messages(&[
            ProviderMessage::AssistantToolCalls {
                text: "I will inspect the file.".into(),
                calls: vec![ToolCall {
                    id: "c".to_string(),
                    name: "read_file".to_string(),
                    arguments: json!({"path":"a"}),
                    metadata: json!({}),
                }],
            },
            ProviderMessage::ToolResult {
                call_id: "c".to_string(),
                name: "read_file".to_string(),
                success: true,
                output: "text".to_string(),
            },
        ]);
        assert_eq!(
            messages[0]["tool_calls"][0]["function"]["name"],
            "read_file"
        );
        assert_eq!(messages[0]["content"], "I will inspect the file.");
        assert_eq!(messages[1]["role"], "tool");
    }

    #[test]
    fn serializes_image_content() {
        let messages = chat_messages(&[ProviderMessage::UserContent {
            text: "inspect".into(),
            images: vec![ProviderImage {
                name: "screen.png".into(),
                data_url: "data:image/png;base64,AA==".into(),
            }],
        }]);
        assert_eq!(messages[0]["content"][1]["type"], "image_url");
    }

    #[test]
    fn deepseek_payload_uses_official_thinking_and_effort_fields() {
        let mut config = provider_config("deepseek-v4-pro-0813");
        config.models = vec![ProviderModelConfig {
            id: "deepseek-v4-pro-0813".to_string(),
            display_name: "DeepSeek V4 Pro 0813".to_string(),
            context_window: 1_000_000,
            max_output_tokens: Some(256_000),
            supports_vision: false,
            fallback: false,
        }];
        let provider = DeepSeekChatCompletionsProvider::new(config, "secret".to_string())
            .expect("DeepSeek provider should build");

        let high = provider
            .inner
            .payload(&provider_request(ReasoningEffort::High, Vec::new()))
            .expect("payload should serialize");
        assert_eq!(high["thinking"]["type"], "enabled");
        assert_eq!(high["reasoning_effort"], "high");
        assert_eq!(high["max_tokens"], 256_000);

        let maximum = provider
            .inner
            .payload(&provider_request(ReasoningEffort::XHigh, Vec::new()))
            .expect("payload should serialize");
        assert_eq!(maximum["thinking"]["type"], "enabled");
        assert_eq!(maximum["reasoning_effort"], "max");

        let off = provider
            .inner
            .payload(&provider_request(ReasoningEffort::Off, Vec::new()))
            .expect("payload should serialize");
        assert_eq!(off["thinking"]["type"], "disabled");
        assert!(off.get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_passback_error_detection_does_not_retry_unrelated_bad_requests() {
        assert!(is_deepseek_reasoning_passback_error(
            "The `reasoning_content` in the thinking mode must be passed back to the API."
        ));
        assert!(is_deepseek_reasoning_passback_error(
            "reasoning_content must pass back to the API"
        ));
        assert!(!is_deepseek_reasoning_passback_error(
            "reasoning_content is not a recognized request field"
        ));
        assert!(!is_deepseek_reasoning_passback_error(
            "tool schema is invalid"
        ));
    }

    #[test]
    fn deepseek_thinking_disabled_keeps_native_tool_history_without_private_cache() {
        let call = tool_call("call-disabled-after-restart");
        let messages = deepseek_chat_messages(
            &[
                ProviderMessage::AssistantToolCalls {
                    text: String::new(),
                    calls: vec![call],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-disabled-after-restart".to_string(),
                    name: "read_file".to_string(),
                    success: true,
                    output: "contents".to_string(),
                },
            ],
            &DeepSeekState::default(),
            false,
        )
        .expect("disabled thinking should not need private reasoning cache");

        assert_eq!(messages[0]["role"], "assistant");
        assert!(messages[0].get("tool_calls").is_some());
        assert!(messages[0].get("reasoning_content").is_none());
        assert_eq!(messages[1]["role"], "tool");
    }

    #[test]
    fn deepseek_does_not_reuse_disabled_or_placeholder_reasoning_when_thinking_is_enabled() {
        let disabled_state = DeepSeekState::default();
        let disabled_call = tool_call("call-mode-switch");
        disabled_state
            .remember(std::slice::from_ref(&disabled_call), "", false)
            .expect("disabled thinking cache entry should be accepted");
        let disabled_messages = deepseek_chat_messages(
            &[
                ProviderMessage::AssistantToolCalls {
                    text: "I will inspect the file.".to_string(),
                    calls: vec![disabled_call],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-mode-switch".to_string(),
                    name: "read_file".to_string(),
                    success: true,
                    output: "contents".to_string(),
                },
            ],
            &disabled_state,
            true,
        )
        .expect("mode-switch history should degrade safely");
        assert!(disabled_messages.iter().all(|message| {
            message["role"] != "tool"
                && message.get("tool_calls").is_none()
                && message.get("reasoning_content").is_none()
        }));

        let whitespace_state = DeepSeekState::default();
        let whitespace_call = tool_call("call-whitespace-reasoning");
        whitespace_state
            .remember(std::slice::from_ref(&whitespace_call), " \n\t", true)
            .expect("placeholder reasoning should remain bounded state");
        let whitespace_messages = deepseek_chat_messages(
            &[
                ProviderMessage::AssistantToolCalls {
                    text: String::new(),
                    calls: vec![whitespace_call],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-whitespace-reasoning".to_string(),
                    name: "read_file".to_string(),
                    success: true,
                    output: "contents".to_string(),
                },
            ],
            &whitespace_state,
            true,
        )
        .expect("placeholder reasoning history should degrade safely");
        assert!(whitespace_messages.iter().all(|message| {
            message["role"] != "tool"
                && message.get("tool_calls").is_none()
                && message.get("reasoning_content").is_none()
        }));
    }

    #[test]
    fn ordinary_openai_payload_keeps_its_existing_reasoning_fields() {
        let provider =
            OpenAiChatCompletionsProvider::new(provider_config("gpt-test"), "secret".to_string())
                .expect("OpenAI provider should build");
        let payload = provider
            .payload(&ProviderRequest {
                model: "gpt-test".to_string(),
                ..provider_request(ReasoningEffort::XHigh, Vec::new())
            })
            .expect("payload should serialize");

        assert_eq!(payload["reasoning_effort"], "xhigh");
        assert!(payload.get("thinking").is_none());
    }

    #[test]
    fn deepseek_reasoning_is_private_but_replayed_for_current_tool_calls() {
        let parsed =
            parse_sse_data(r#"{"choices":[{"delta":{"reasoning_content":"private chain"}}]}"#)
                .expect("reasoning delta should parse");
        assert!(parsed.events.is_empty());
        assert_eq!(parsed.reasoning_deltas, vec!["private chain"]);

        let state = DeepSeekState::default();
        let call = tool_call("call-current");
        state
            .remember(std::slice::from_ref(&call), "private chain", true)
            .expect("reasoning should fit the bounded cache");
        let messages = deepseek_chat_messages(
            &[
                ProviderMessage::AssistantToolCalls {
                    text: String::new(),
                    calls: vec![call],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-current".to_string(),
                    name: "read_file".to_string(),
                    success: true,
                    output: String::new(),
                },
            ],
            &state,
            true,
        )
        .expect("current tool history should serialize");

        assert_eq!(messages[0]["content"], "");
        assert_eq!(messages[0]["reasoning_content"], "private chain");
        assert_eq!(messages[0]["tool_calls"][0]["id"], "call-current");
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["content"], "(no output)");
    }

    #[test]
    fn deepseek_degrades_when_thinking_tool_call_has_no_private_reasoning() {
        let state = DeepSeekState::default();
        let call = tool_call("call-empty-reasoning");
        state
            .remember(std::slice::from_ref(&call), "", true)
            .expect("missing private reasoning should remain bounded state");

        let messages = deepseek_chat_messages(
            &[
                ProviderMessage::AssistantToolCalls {
                    text: String::new(),
                    calls: vec![call],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-empty-reasoning".to_string(),
                    name: "request_user_input".to_string(),
                    success: false,
                    output: "user skipped the question".to_string(),
                },
            ],
            &state,
            true,
        )
        .expect("thinking-mode tool history should degrade without exact passback");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "assistant");
        assert!(
            messages[0]["content"]
                .as_str()
                .expect("observation should be textual")
                .contains("user skipped the question")
        );
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], DEEPSEEK_RETRY_CONTINUATION);
        assert!(messages.iter().all(|message| {
            message["role"] != "tool"
                && message.get("tool_calls").is_none()
                && message.get("reasoning_content").is_none()
        }));

        let disabled_state = DeepSeekState::default();
        let disabled_call = tool_call("call-thinking-disabled");
        disabled_state
            .remember(std::slice::from_ref(&disabled_call), "", false)
            .expect("disabled thinking should keep tool-call pairing");
        let disabled_messages = deepseek_chat_messages(
            &[ProviderMessage::AssistantToolCalls {
                text: String::new(),
                calls: vec![disabled_call],
            }],
            &disabled_state,
            false,
        )
        .expect("disabled thinking history should serialize");

        assert_eq!(disabled_messages[0]["role"], "assistant");
        assert!(disabled_messages[0].get("tool_calls").is_some());
        assert!(disabled_messages[0].get("reasoning_content").is_none());
    }

    #[test]
    fn deepseek_degrades_tool_history_when_private_reasoning_is_unavailable() {
        let call = ToolCall {
            id: "call-old".to_string(),
            name: "request_user_input".to_string(),
            arguments: json!({
                "questions": [{
                    "question": "internal raw question",
                    "options": ["internal raw option", "another option"]
                }]
            }),
            metadata: json!({}),
        };
        let messages = deepseek_chat_messages(
            &[
                ProviderMessage::AssistantToolCalls {
                    text: "I need one clarification before continuing.".to_string(),
                    calls: vec![call],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-old".to_string(),
                    name: "request_user_input".to_string(),
                    success: true,
                    output: "Q: Which field represents invalid data?\nA: IsDeleted = 1".to_string(),
                },
            ],
            &DeepSeekState::default(),
            true,
        )
        .expect("historical tool facts should serialize without private reasoning");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "assistant");
        assert!(messages[0].get("tool_calls").is_none());
        assert!(messages[0].get("reasoning_content").is_none());
        let content = messages[0]["content"]
            .as_str()
            .expect("fallback context should remain textual");
        assert!(content.contains("I need one clarification"));
        assert!(content.contains("The user previously clarified:"));
        assert!(content.contains("A: IsDeleted = 1"));
        for internal in [
            LEGACY_DEEPSEEK_TOOL_CALLS_MARKER,
            LEGACY_DEEPSEEK_TOOL_RESULT_MARKER,
            "call-old",
            "arguments:",
            "internal raw question",
            "internal raw option",
        ] {
            assert!(
                !content.contains(internal),
                "leaked internal text: {internal}"
            );
        }
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], DEEPSEEK_RETRY_CONTINUATION);
        assert!(
            !messages[1]["content"]
                .as_str()
                .expect("continuation should be textual")
                .contains("IsDeleted")
        );
        assert!(messages.iter().all(|message| message["role"] != "tool"));
    }

    #[test]
    fn deepseek_keeps_failed_historical_results_as_low_privilege_observations() {
        let call = ToolCall {
            id: "call-failed".to_string(),
            name: "read_file".to_string(),
            arguments: json!({"path": "private/raw/path.rs"}),
            metadata: json!({}),
        };
        let messages = deepseek_chat_messages(
            &[
                ProviderMessage::AssistantToolCalls {
                    text: String::new(),
                    calls: vec![call],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-failed".to_string(),
                    name: "read_file".to_string(),
                    success: false,
                    output: "path was not found".to_string(),
                },
            ],
            &DeepSeekState::default(),
            true,
        )
        .expect("failed historical tool facts should remain usable context");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(
            messages[0]["content"],
            "An earlier read_file operation failed:\npath was not found"
        );
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], DEEPSEEK_RETRY_CONTINUATION);
        let serialized = serde_json::to_string(&messages).expect("messages should serialize");
        assert!(!serialized.contains("call-failed"));
        assert!(!serialized.contains("private/raw/path.rs"));
    }

    #[test]
    fn deepseek_removes_only_legacy_artifacts_matching_structured_history() {
        let unrelated_text = concat!(
            "Please explain this exact record:\n",
            "[Historical tool calls]\n",
            "- request_user_input (call-user) arguments: {\"questions\":[]}"
        );
        let call = ToolCall {
            id: "call-old".to_string(),
            name: "request_user_input".to_string(),
            arguments: json!({"questions": []}),
            metadata: json!({}),
        };
        let messages = deepseek_chat_messages(
            &[
                ProviderMessage::AssistantToolCalls {
                    text: String::new(),
                    calls: vec![call],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-old".to_string(),
                    name: "request_user_input".to_string(),
                    success: true,
                    output: "raw result".to_string(),
                },
                ProviderMessage::Text {
                    role: MessageRole::Assistant,
                    text: concat!(
                        "Keep this answer.\n\n",
                        "[Historical tool calls]\n",
                        "- request_user_input (call-old) arguments: {\"questions\":[]}"
                    )
                    .to_string(),
                },
                ProviderMessage::Text {
                    role: MessageRole::Assistant,
                    text: concat!(
                        "Keep this result context.\n",
                        "[Historical tool result for request_user_input (call-old); status: success]\n",
                        "raw result"
                    )
                    .to_string(),
                },
                ProviderMessage::Text {
                    role: MessageRole::Assistant,
                    text: unrelated_text.to_string(),
                },
                ProviderMessage::Text {
                    role: MessageRole::User,
                    text: unrelated_text.to_string(),
                },
            ],
            &DeepSeekState::default(),
            true,
        )
        .expect("legacy assistant artifacts should be sanitized at the request boundary");

        assert_eq!(messages.len(), 2);
        let assistant = messages[0]["content"]
            .as_str()
            .expect("assistant context should remain textual");
        assert!(assistant.contains("The user previously clarified:\nraw result"));
        assert!(assistant.contains("Keep this answer."));
        assert!(assistant.contains("Keep this result context."));
        assert!(assistant.contains(unrelated_text));
        assert!(!assistant.contains("call-old"));
        assert!(!assistant.contains(LEGACY_DEEPSEEK_TOOL_RESULT_MARKER));
        assert_eq!(messages[1]["content"], unrelated_text);
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn deepseek_rejects_images_before_network_io() {
        let error = deepseek_chat_messages(
            &[ProviderMessage::UserContent {
                text: "inspect".to_string(),
                images: vec![ProviderImage {
                    name: "screen.png".to_string(),
                    data_url: "data:image/png;base64,AA==".to_string(),
                }],
            }],
            &DeepSeekState::default(),
            true,
        )
        .expect_err("DeepSeek text-only transport must reject images");

        assert!(matches!(error, ProviderError::Request(message) if message.contains("image")));
    }

    #[test]
    fn deepseek_private_reasoning_cache_is_bounded() {
        let state = DeepSeekState::default();
        let error = state
            .remember(
                &[tool_call("call-too-large")],
                &"x".repeat(MAX_DEEPSEEK_REASONING_CACHE_BYTES + 1),
                true,
            )
            .expect_err("oversized private reasoning must be rejected");

        assert!(
            matches!(error, ProviderError::InvalidResponse(message) if message.contains("cache exceeded"))
        );

        let calls = (0..=MAX_DEEPSEEK_CACHED_TOOL_CALLS)
            .map(|index| tool_call(&format!("call-{index}")))
            .collect::<Vec<_>>();
        let error = DeepSeekState::default()
            .remember(&calls, "private", true)
            .expect_err("too many tool calls must be rejected");
        assert!(
            matches!(error, ProviderError::InvalidResponse(message) if message.contains("more than"))
        );
    }

    #[tokio::test]
    async fn deepseek_replays_private_reasoning_across_real_stream_requests() {
        let first_response = concat!(
            r#"data: {"choices":[{"delta":{"reasoning_content":"private chain"}}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-live","function":{"name":"read_file","arguments":"{\"path\":\"README.md\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let second_response = concat!(
            r#"data: {"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, mut received, server) =
            spawn_sse_server(vec![first_response, second_response]).await;
        let mut config = provider_config("deepseek-v4-pro-0813");
        config.base_url = base_url;
        let provider = DeepSeekChatCompletionsProvider::new(config, "secret".to_string())
            .expect("DeepSeek provider should build");

        let first_request = provider_request(
            ReasoningEffort::High,
            vec![ProviderMessage::Text {
                role: MessageRole::User,
                text: "inspect the readme".to_string(),
            }],
        );
        let mut first_stream = provider
            .stream(first_request, CancellationToken::new())
            .await
            .expect("first request should connect");
        let mut returned_call = None;
        while let Some(event) = first_stream.next().await {
            match event.expect("first stream should be valid") {
                ProviderEvent::ToolCall { call } => returned_call = Some(call),
                ProviderEvent::Completed => {}
                event => panic!("private reasoning must not become a public event: {event:?}"),
            }
        }
        let returned_call = returned_call.expect("first request should yield a tool call");

        let second_request = provider_request(
            ReasoningEffort::High,
            vec![
                ProviderMessage::Text {
                    role: MessageRole::User,
                    text: "inspect the readme".to_string(),
                },
                ProviderMessage::AssistantToolCalls {
                    text: String::new(),
                    calls: vec![returned_call],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-live".to_string(),
                    name: "read_file".to_string(),
                    success: true,
                    output: "contents".to_string(),
                },
            ],
        );
        let mut second_stream = provider
            .stream(second_request, CancellationToken::new())
            .await
            .expect("second request should connect");
        while let Some(event) = second_stream.next().await {
            event.expect("second stream should be valid");
        }

        let first_payload = received.recv().await.expect("first payload should arrive");
        assert_eq!(first_payload["thinking"]["type"], "enabled");
        assert_eq!(first_payload["reasoning_effort"], "high");
        let second_payload = received.recv().await.expect("second payload should arrive");
        assert_eq!(second_payload["messages"][1]["role"], "assistant");
        assert_eq!(second_payload["messages"][1]["content"], "");
        assert_eq!(
            second_payload["messages"][1]["reasoning_content"],
            "private chain"
        );
        assert_eq!(
            second_payload["messages"][1]["tool_calls"][0]["id"],
            "call-live"
        );
        assert_eq!(second_payload["messages"][2]["role"], "tool");
        assert_eq!(second_payload["messages"][2]["tool_call_id"], "call-live");
        server.await.expect("test server should stop cleanly");
    }

    #[tokio::test]
    async fn deepseek_degrades_live_tool_round_when_endpoint_withholds_reasoning() {
        let first_response = concat!(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-withheld","function":{"name":"read_file","arguments":"{\"path\":\"README.md\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let second_response = concat!(
            r#"data: {"choices":[{"delta":{"content":"continued safely"},"finish_reason":"stop"}]}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, mut received, server) =
            spawn_sse_server(vec![first_response, second_response]).await;
        let mut config = provider_config("deepseek-v4-pro-0813");
        config.base_url = base_url;
        let provider = DeepSeekChatCompletionsProvider::new(config, "secret".to_string())
            .expect("DeepSeek provider should build");

        let mut first_stream = provider
            .stream(
                provider_request(
                    ReasoningEffort::High,
                    vec![ProviderMessage::Text {
                        role: MessageRole::User,
                        text: "inspect the readme".to_string(),
                    }],
                ),
                CancellationToken::new(),
            )
            .await
            .expect("first request should connect");
        let mut returned_call = None;
        while let Some(event) = first_stream.next().await {
            match event.expect("first stream should be valid") {
                ProviderEvent::ToolCall { call } => returned_call = Some(call),
                ProviderEvent::Completed => {}
                event => panic!("withheld reasoning must not become a public event: {event:?}"),
            }
        }

        let mut second_stream = provider
            .stream(
                provider_request(
                    ReasoningEffort::High,
                    vec![
                        ProviderMessage::Text {
                            role: MessageRole::User,
                            text: "inspect the readme".to_string(),
                        },
                        ProviderMessage::AssistantToolCalls {
                            text: "I will inspect the file.".to_string(),
                            calls: vec![
                                returned_call.expect("first request should yield a tool call"),
                            ],
                        },
                        ProviderMessage::ToolResult {
                            call_id: "call-withheld".to_string(),
                            name: "read_file".to_string(),
                            success: true,
                            output: "contents-marker".to_string(),
                        },
                    ],
                ),
                CancellationToken::new(),
            )
            .await
            .expect("degraded continuation should connect");
        let mut continuation = String::new();
        while let Some(event) = second_stream.next().await {
            match event.expect("degraded continuation stream should be valid") {
                ProviderEvent::TextDelta { delta } => continuation.push_str(&delta),
                ProviderEvent::Completed => {}
                event => panic!("verification should be followed by final text: {event:?}"),
            }
        }
        assert_eq!(continuation, "continued safely");

        received.recv().await.expect("first payload should arrive");
        let second_payload = received.recv().await.expect("second payload should arrive");
        let messages = second_payload["messages"]
            .as_array()
            .expect("messages should be an array");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        let observation = messages[1]["content"]
            .as_str()
            .expect("degraded observation should be textual");
        assert!(observation.contains("I will inspect the file."));
        assert!(observation.contains("contents-marker"));
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], DEEPSEEK_RETRY_CONTINUATION);
        let finalization_boundary = messages[2]["content"]
            .as_str()
            .expect("continuation boundary should be textual");
        assert!(finalization_boundary.contains("return the final answer now"));
        assert!(finalization_boundary.contains("specific unresolved fact"));
        assert!(finalization_boundary.contains("do not reread unchanged overlapping"));
        assert!(messages.iter().all(|message| {
            message["role"] != "tool"
                && message.get("tool_calls").is_none()
                && message.get("reasoning_content").is_none()
        }));
        server.await.expect("test server should stop cleanly");
    }

    #[tokio::test]
    async fn deepseek_retries_a_rejected_private_passback_with_safe_history() {
        let first_response = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"private chain\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-rejected\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let recovered_response = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"recovered safely\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, mut received, server) = spawn_http_server(vec![
            MockHttpResponse {
                status: 200,
                content_type: "text/event-stream",
                body: first_response,
            },
            MockHttpResponse {
                status: 400,
                content_type: "application/json",
                body: r#"{"error":{"message":"The `reasoning_content` in the thinking mode must be passed back to the API."}}"#,
            },
            MockHttpResponse {
                status: 200,
                content_type: "text/event-stream",
                body: recovered_response,
            },
        ])
        .await;
        let mut config = provider_config("deepseek-v4-pro-0813");
        config.base_url = base_url;
        let provider = DeepSeekChatCompletionsProvider::new(config, "secret".to_string())
            .expect("DeepSeek provider should build");

        let mut first_stream = provider
            .stream(
                provider_request(
                    ReasoningEffort::High,
                    vec![ProviderMessage::Text {
                        role: MessageRole::User,
                        text: "inspect the readme".to_string(),
                    }],
                ),
                CancellationToken::new(),
            )
            .await
            .expect("first request should connect");
        let mut returned_call = None;
        while let Some(event) = first_stream.next().await {
            match event.expect("first stream should be valid") {
                ProviderEvent::ToolCall { call } => returned_call = Some(call),
                ProviderEvent::Completed => {}
                event => panic!("private reasoning must remain private: {event:?}"),
            }
        }
        let returned_call = returned_call.expect("first request should yield a tool call");

        let mut recovered_stream = provider
            .stream(
                provider_request(
                    ReasoningEffort::High,
                    vec![
                        ProviderMessage::Text {
                            role: MessageRole::User,
                            text: "inspect the readme".to_string(),
                        },
                        ProviderMessage::AssistantToolCalls {
                            text: "I will inspect the file.".to_string(),
                            calls: vec![returned_call],
                        },
                        ProviderMessage::ToolResult {
                            call_id: "call-rejected".to_string(),
                            name: "read_file".to_string(),
                            success: true,
                            output: "contents-marker".to_string(),
                        },
                    ],
                ),
                CancellationToken::new(),
            )
            .await
            .expect("the compatibility retry should return a stream");
        let mut text = String::new();
        while let Some(event) = recovered_stream.next().await {
            match event.expect("recovered stream should be valid") {
                ProviderEvent::TextDelta { delta } => text.push_str(&delta),
                ProviderEvent::Completed => {}
                event => panic!("unexpected recovered event: {event:?}"),
            }
        }
        assert_eq!(text, "recovered safely");

        let first_payload = received.recv().await.expect("first payload should arrive");
        assert!(!has_native_deepseek_tool_history(&first_payload));
        let rejected_payload = received
            .recv()
            .await
            .expect("rejected payload should arrive");
        assert!(has_native_deepseek_tool_history(&rejected_payload));
        let recovered_payload = received
            .recv()
            .await
            .expect("recovered payload should arrive");
        let messages = recovered_payload["messages"]
            .as_array()
            .expect("recovered messages should be an array");
        assert!(messages.iter().all(|message| {
            message["role"] != "tool"
                && message.get("tool_calls").is_none()
                && message.get("reasoning_content").is_none()
        }));
        assert_eq!(
            messages.last().and_then(|message| message["role"].as_str()),
            Some("user")
        );
        assert_eq!(
            messages
                .last()
                .and_then(|message| message["content"].as_str()),
            Some(DEEPSEEK_RETRY_CONTINUATION)
        );
        let serialized =
            serde_json::to_string(messages).expect("recovered messages should serialize");
        assert!(!serialized.contains("call-rejected"));
        assert!(!serialized.contains("private chain"));
        assert!(serialized.contains("contents-marker"));
        assert!(
            messages
                .last()
                .and_then(|message| message["content"].as_str())
                .is_some_and(|content| !content.contains("contents-marker"))
        );
        server.await.expect("test server should stop cleanly");
    }

    #[tokio::test]
    async fn deepseek_retry_after_provider_rebuild_uses_a_safe_continuation_boundary() {
        let first_response = concat!(
            r#"data: {"choices":[{"delta":{"reasoning_content":"private chain"}}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-rebuild","function":{"name":"read_file","arguments":"{\"path\":\"private/raw/path.rs\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let retry_response = concat!(
            r#"data: {"choices":[{"delta":{"content":"continued"},"finish_reason":"stop"}]}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, mut received, server) =
            spawn_sse_server(vec![first_response, retry_response]).await;
        let mut config = provider_config("deepseek-v4-pro-0813");
        config.base_url = base_url;
        let first_provider =
            DeepSeekChatCompletionsProvider::new(config.clone(), "secret".to_string())
                .expect("first DeepSeek provider should build");

        let mut first_stream = first_provider
            .stream(
                provider_request(
                    ReasoningEffort::High,
                    vec![ProviderMessage::Text {
                        role: MessageRole::User,
                        text: "inspect the workspace".to_string(),
                    }],
                ),
                CancellationToken::new(),
            )
            .await
            .expect("first request should connect");
        let mut returned_call = None;
        while let Some(event) = first_stream.next().await {
            match event.expect("first stream should be valid") {
                ProviderEvent::ToolCall { call } => returned_call = Some(call),
                ProviderEvent::Completed => {}
                event => panic!("private reasoning must not become a public event: {event:?}"),
            }
        }
        let returned_call = returned_call.expect("first request should yield a tool call");

        // A retry command builds a fresh Provider, so the private reasoning cache
        // from the failed Turn is deliberately unavailable.
        let retry_provider = DeepSeekChatCompletionsProvider::new(config, "secret".to_string())
            .expect("retry DeepSeek provider should build");
        let mut retry_stream = retry_provider
            .stream(
                provider_request(
                    ReasoningEffort::High,
                    vec![
                        ProviderMessage::Text {
                            role: MessageRole::User,
                            text: "inspect the workspace".to_string(),
                        },
                        ProviderMessage::AssistantToolCalls {
                            text: String::new(),
                            calls: vec![returned_call],
                        },
                        ProviderMessage::ToolResult {
                            call_id: "call-rebuild".to_string(),
                            name: "read_file".to_string(),
                            success: true,
                            output: "result-marker".to_string(),
                        },
                    ],
                ),
                CancellationToken::new(),
            )
            .await
            .expect("retry request should connect");
        while let Some(event) = retry_stream.next().await {
            event.expect("retry stream should be valid");
        }

        received.recv().await.expect("first payload should arrive");
        let retry_payload = received.recv().await.expect("retry payload should arrive");
        let retry_messages = retry_payload["messages"]
            .as_array()
            .expect("retry messages should be an array");
        assert_eq!(retry_messages.len(), 3);
        assert_eq!(retry_messages[0]["role"], "user");
        assert_eq!(retry_messages[1]["role"], "assistant");
        assert!(
            retry_messages[1]["content"]
                .as_str()
                .expect("observation should be textual")
                .contains("result-marker")
        );
        assert_eq!(retry_messages[2]["role"], "user");
        assert_eq!(retry_messages[2]["content"], DEEPSEEK_RETRY_CONTINUATION);
        assert!(retry_messages.iter().all(|message| {
            message["role"] != "tool"
                && message.get("tool_calls").is_none()
                && message.get("reasoning_content").is_none()
        }));
        let serialized = serde_json::to_string(retry_messages).expect("messages should serialize");
        assert!(!serialized.contains("call-rebuild"));
        assert!(!serialized.contains("private/raw/path.rs"));
        assert!(!serialized.contains("private chain"));
        assert!(
            !retry_messages[2]["content"]
                .as_str()
                .expect("continuation should be textual")
                .contains("result-marker")
        );
        server.await.expect("test server should stop cleanly");
    }
}
