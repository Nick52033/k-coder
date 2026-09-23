use async_trait::async_trait;
use base64::Engine;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::common::{
    build_client, read_error_details, redact_error, redact_event, require_api_key,
};
use super::sse::SseDecoder;
use super::{
    Provider, ProviderConfig, ProviderError, ProviderEvent, ProviderMessage, ProviderRequest,
    ProviderStream,
};
use crate::protocol::{MessageRole, ReasoningEffort, TokenUsage, TokenUsageDetails, ToolCall};

const MAX_GENERATED_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_GEMINI_IMAGE_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub struct GoogleGeminiProvider {
    client: Client,
    config: ProviderConfig,
    api_key: String,
}

impl GoogleGeminiProvider {
    pub fn new(config: ProviderConfig, api_key: String) -> Result<Self, ProviderError> {
        require_api_key(&api_key)?;
        let config = config
            .validate()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        Ok(Self {
            client: build_client()?,
            config,
            api_key,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponsePart {
    text: Option<String>,
    function_call: Option<GeminiFunctionCall>,
    inline_data: Option<GeminiInlineData>,
    thought_signature: Option<String>,
    #[serde(default)]
    thought: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiInlineData {
    mime_type: Option<String>,
    data: String,
}

#[derive(Deserialize)]
struct GeminiFunctionCall {
    id: Option<String>,
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    usage_metadata: Option<GeminiUsage>,
    prompt_feedback: Option<GeminiPromptFeedback>,
    error: Option<GeminiError>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiResponseContent>,
    finish_reason: Option<String>,
    finish_message: Option<String>,
}

#[derive(Deserialize)]
struct GeminiResponseContent {
    #[serde(default)]
    parts: Vec<GeminiResponsePart>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsage {
    #[serde(default)]
    prompt_token_count: u64,
    #[serde(default)]
    candidates_token_count: u64,
    cached_content_token_count: Option<u64>,
    thoughts_token_count: Option<u64>,
    total_token_count: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPromptFeedback {
    block_reason: Option<String>,
}

#[derive(Deserialize)]
struct GeminiError {
    message: String,
}

struct ParsedGeminiEvent {
    events: Vec<ProviderEvent>,
    completed: bool,
    terminal_error: Option<ProviderError>,
}

#[async_trait]
impl Provider for GoogleGeminiProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        let image_generation = is_image_generation_model(&request.model);
        let endpoint = if image_generation {
            self.config.gemini_generate_url()
        } else {
            self.config.gemini_stream_url()
        }
        .map_err(|error| ProviderError::Request(error.to_string()))?;
        let contents = if image_generation {
            gemini_image_contents(&request)?
        } else {
            gemini_contents(&request.messages)
        };
        let mut payload = json!({ "contents": contents });
        // Gemini 的 system 消息需要放在顶层 systemInstruction 字段
        let system_text: Vec<&str> = request
            .messages
            .iter()
            .filter_map(|m| match m {
                ProviderMessage::Text {
                    role: MessageRole::System,
                    text,
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if !system_text.is_empty() {
            payload["systemInstruction"] = json!({
                "parts": [{ "text": system_text.join("\n\n") }]
            });
        }
        if !request.tools.is_empty() && !image_generation {
            payload["tools"] = json!([{
                "functionDeclarations": request.tools.iter().map(|tool| json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": gemini_schema(tool.input_schema.clone())
                })).collect::<Vec<_>>()
            }]);
        }
        if image_generation {
            payload["generationConfig"] = json!({
                "responseModalities": ["TEXT", "IMAGE"]
            });
        } else if let Some(thinking_config) = gemini_thinking_config(request.reasoning_effort) {
            payload["generationConfig"] = json!({ "thinkingConfig": thinking_config });
        }

        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            response = self.client
                .post(endpoint)
                .header("x-goog-api-key", &self.api_key)
                .header(
                    "accept",
                    if image_generation {
                        "application/json"
                    } else {
                        "text/event-stream"
                    },
                )
                .json(&payload)
                .send() => response.map_err(|error| ProviderError::Request(error.to_string()))?,
        };
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = super::common::retry_after(&response);
            let details = read_error_details(response, &cancellation, &self.api_key).await?;
            return Err(ProviderError::from_http_with_diagnostics(
                status,
                details.message,
                retry_after,
                details.code,
                details.request_id,
            ));
        }

        let secret = self.api_key.clone();
        if image_generation {
            Ok(Box::pin(async_stream::stream! {
                let reasoning_item_id = format!("gemini-reasoning-{}", Uuid::new_v4());
                let parsed = read_bounded_image_response(response, &cancellation, &reasoning_item_id).await;
                match parsed {
                    Ok(parsed) => {
                        for event in parsed.events {
                            yield Ok(redact_event(event, &secret));
                        }
                        if let Some(error) = parsed.terminal_error {
                            yield Err(redact_error(error, &secret));
                            return;
                        }
                        if parsed.completed {
                            yield Ok(ProviderEvent::Completed);
                        } else {
                            yield Err(ProviderError::Interrupted);
                        }
                    }
                    Err(error) => yield Err(redact_error(error, &secret)),
                }
            }))
        } else {
            Ok(Box::pin(async_stream::stream! {
                let mut body = response.bytes_stream();
                let mut decoder = SseDecoder::default();
                let reasoning_item_id = format!("gemini-reasoning-{}", Uuid::new_v4());
                let mut reasoning_summary = String::new();
                loop {
                    let chunk = tokio::select! {
                        _ = cancellation.cancelled() => { yield Err(ProviderError::Cancelled); return; }
                        chunk = body.next() => chunk,
                    };
                    match chunk {
                        Some(Ok(bytes)) => match decoder.push(&bytes) {
                            Ok(frames) => for frame in frames {
                                match parse_sse_data(&frame, &reasoning_item_id) {
                                    Ok(parsed) => {
                                        for event in parsed.events {
                                            if let ProviderEvent::ReasoningSummaryDelta { delta, .. } = &event {
                                                reasoning_summary.push_str(delta);
                                            }
                                            yield Ok(redact_event(event, &secret));
                                        }
                                        if let Some(error) = parsed.terminal_error {
                                            yield Err(redact_error(error, &secret));
                                            return;
                                        }
                                        if parsed.completed {
                                            if !reasoning_summary.is_empty() {
                                                yield Ok(redact_event(ProviderEvent::ReasoningSummaryCompleted {
                                                    item_id: reasoning_item_id.clone(),
                                                    summary: reasoning_summary,
                                                }, &secret));
                                            }
                                            yield Ok(ProviderEvent::Completed);
                                            return;
                                        }
                                    }
                                    Err(error) => { yield Err(redact_error(error, &secret)); return; }
                                }
                            },
                            Err(error) => { yield Err(error); return; }
                        },
                        Some(Err(error)) => { yield Err(redact_error(ProviderError::Request(error.to_string()), &secret)); return; }
                        None => { yield Err(decoder.finish().err().unwrap_or(ProviderError::Interrupted)); return; }
                    }
                }
            }))
        }
    }
}

fn is_image_generation_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.contains("image-generation") || model.contains("-image-") || model.ends_with("-image")
}

fn gemini_thinking_config(reasoning_effort: ReasoningEffort) -> Option<Value> {
    reasoning_effort.gemini_budget_tokens().map(|budget| {
        json!({
            "thinkingBudget": budget,
            "includeThoughts": true
        })
    })
}

fn gemini_contents(messages: &[ProviderMessage]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| match message {
            ProviderMessage::Text { role, text } => match role {
                MessageRole::System => None, // System 消息通过 systemInstruction 注入
                _ => Some(json!({
                    "role": match role { MessageRole::User => "user", MessageRole::Assistant => "model", _ => "user" },
                    "parts": [{ "text": text }]
                })),
            },
            ProviderMessage::UserContent { text, images } => Some(json!({
                "role": "user",
                "parts": std::iter::once(json!({ "text": text }))
                    .chain(images.iter().filter_map(|image| {
                        let (mime_type, data) = super::split_image_data_url(&image.data_url)?;
                        Some(json!({ "inlineData": { "mimeType": mime_type, "data": data } }))
                    }))
                    .collect::<Vec<_>>()
            })),
            ProviderMessage::AssistantImageReference { text, .. } => Some(json!({
                "role": "model",
                "parts": [{ "text": text }]
            })),
            ProviderMessage::AssistantToolCalls { text, calls } => {
                let parts = std::iter::once(text).filter(|text| !text.is_empty()).map(|text| {
                    json!({ "text": text })
                }).chain(calls.iter().map(|call| {
                    let mut part = json!({
                        "functionCall": { "id": call.id, "name": call.name, "args": call.arguments }
                    });
                    if let Some(signature) = call.metadata["thoughtSignature"].as_str() {
                        part["thoughtSignature"] = Value::String(signature.to_string());
                    }
                    part
                })).collect::<Vec<_>>();
                Some(json!({ "role": "model", "parts": parts }))
            },
            ProviderMessage::ToolResult { call_id, name, success, output } => Some(json!({
                "role": "user",
                "parts": [{
                    "functionResponse": {
                        "id": call_id,
                        "name": name,
                        "response": { "success": success, "output": output }
                    }
                }]
            })),
            ProviderMessage::ProviderContext { .. } => None,
        })
        .collect()
}

fn parse_sse_data(data: &str, reasoning_item_id: &str) -> Result<ParsedGeminiEvent, ProviderError> {
    let response: GeminiResponse = serde_json::from_str(data).map_err(|error| {
        ProviderError::InvalidResponse(format!("malformed Gemini event: {error}"))
    })?;
    if let Some(error) = response.error {
        return Err(ProviderError::InvalidResponse(error.message));
    }
    let prompt_block = response
        .prompt_feedback
        .and_then(|feedback| feedback.block_reason)
        .filter(|reason| !reason.is_empty() && reason != "BLOCK_REASON_UNSPECIFIED");
    let mut events = Vec::new();
    let candidate = response.candidates.into_iter().next();
    if let Some(candidate) = candidate.as_ref() {
        if let Some(content) = &candidate.content {
            for part in &content.parts {
                if let Some(delta) = part.text.clone().filter(|delta| !delta.is_empty()) {
                    if part.thought {
                        events.push(ProviderEvent::ReasoningSummaryDelta {
                            item_id: reasoning_item_id.to_string(),
                            delta,
                        });
                    } else {
                        events.push(ProviderEvent::TextDelta { delta });
                    }
                }
                if !part.thought {
                    if let Some(inline_data) = &part.inline_data {
                        if !inline_data.data.is_empty() {
                            let decoded = base64::engine::general_purpose::STANDARD
                                .decode(&inline_data.data)
                                .map_err(|_| {
                                    ProviderError::InvalidResponse(
                                        "Gemini returned invalid base64 image data".to_string(),
                                    )
                                })?;
                            if decoded.len() > MAX_GENERATED_IMAGE_BYTES {
                                return Err(ProviderError::InvalidResponse(format!(
                                    "Gemini generated image exceeds {MAX_GENERATED_IMAGE_BYTES} bytes"
                                )));
                            }
                            let mime_type = inline_data.mime_type.as_deref().unwrap_or("image/png");
                            if !matches!(
                                mime_type,
                                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                            ) {
                                return Err(ProviderError::InvalidResponse(
                                    "Gemini returned an unsupported image MIME type".to_string(),
                                ));
                            }
                            events.push(ProviderEvent::Image {
                                mime_type: mime_type.to_string(),
                                data: inline_data.data.clone(),
                            });
                        }
                    }
                }
                if let Some(call) = &part.function_call {
                    events.push(ProviderEvent::ToolCall {
                        call: ToolCall {
                            id: call
                                .id
                                .clone()
                                .unwrap_or_else(|| Uuid::new_v4().to_string()),
                            name: call.name.clone(),
                            arguments: call.args.clone(),
                            metadata: match &part.thought_signature {
                                Some(signature) => json!({ "thoughtSignature": signature }),
                                None => json!({}),
                            },
                        },
                    });
                }
            }
        }
    }
    if let Some(usage) = response.usage_metadata {
        let output_tokens = usage
            .candidates_token_count
            .saturating_add(usage.thoughts_token_count.unwrap_or(0));
        let token_usage = TokenUsage {
            input_tokens: usage.prompt_token_count,
            output_tokens,
            total_tokens: usage
                .total_token_count
                .unwrap_or(usage.prompt_token_count.saturating_add(output_tokens)),
        };
        let details = TokenUsageDetails {
            cached_input_tokens: usage.cached_content_token_count,
            uncached_input_tokens: usage
                .cached_content_token_count
                .and_then(|cached| usage.prompt_token_count.checked_sub(cached)),
            cache_write_input_tokens: None,
            reasoning_output_tokens: usage.thoughts_token_count,
        };
        events.push(if details.is_empty() {
            ProviderEvent::Usage { usage: token_usage }
        } else {
            ProviderEvent::DetailedUsage {
                usage: token_usage,
                details,
            }
        });
    }
    let finish_reason = candidate
        .as_ref()
        .and_then(|candidate| candidate.finish_reason.as_deref())
        .filter(|reason| !reason.is_empty() && *reason != "FINISH_REASON_UNSPECIFIED");
    let completed = finish_reason == Some("STOP");
    let terminal_error = if let Some(reason) = prompt_block {
        Some(ProviderError::InvalidResponse(format!(
            "Gemini blocked the prompt: {reason}"
        )))
    } else {
        finish_reason
            .filter(|reason| *reason != "STOP")
            .map(|reason| {
                ProviderError::InvalidResponse(
                    candidate
                        .as_ref()
                        .and_then(|candidate| candidate.finish_message.clone())
                        .unwrap_or_else(|| format!("Gemini stopped generation: {reason}")),
                )
            })
    };
    Ok(ParsedGeminiEvent {
        events,
        completed,
        terminal_error,
    })
}

fn gemini_image_contents(request: &ProviderRequest) -> Result<Vec<Value>, ProviderError> {
    let image_request =
        super::image::OpenAiImageGenerationsProvider::request_from_provider(request)?;
    let mut parts = vec![json!({ "text": image_request.prompt })];
    if let Some(image) = image_request.image {
        let (mime_type, data) = super::split_image_data_url(&image).ok_or_else(|| {
            ProviderError::InvalidToolArguments(
                "Gemini image reference must be a base64 data URL".into(),
            )
        })?;
        parts.push(json!({ "inlineData": { "mimeType": mime_type, "data": data } }));
    }
    Ok(vec![json!({ "role": "user", "parts": parts })])
}

fn read_image_response_body(
    bytes: &[u8],
    reasoning_item_id: &str,
) -> Result<ParsedGeminiEvent, ProviderError> {
    if bytes.len() > MAX_GEMINI_IMAGE_RESPONSE_BYTES {
        return Err(ProviderError::InvalidResponse(format!(
            "Gemini image response exceeds {MAX_GEMINI_IMAGE_RESPONSE_BYTES} bytes"
        )));
    }
    let data = std::str::from_utf8(bytes).map_err(|_| {
        ProviderError::InvalidResponse("Gemini image response is not valid UTF-8".into())
    })?;
    parse_sse_data(data, reasoning_item_id)
}

async fn read_bounded_image_response(
    response: reqwest::Response,
    cancellation: &CancellationToken,
    reasoning_item_id: &str,
) -> Result<ParsedGeminiEvent, ProviderError> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = tokio::select! {
        _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
        chunk = stream.next() => chunk,
    } {
        let chunk = chunk.map_err(|error| ProviderError::Request(error.to_string()))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_GEMINI_IMAGE_RESPONSE_BYTES {
            return Err(ProviderError::InvalidResponse(format!(
                "Gemini image response exceeds {MAX_GEMINI_IMAGE_RESPONSE_BYTES} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    read_image_response_body(&bytes, reasoning_item_id)
}

fn gemini_schema(mut schema: Value) -> Value {
    match &mut schema {
        Value::Object(values) => {
            values.remove("$schema");
            values.remove("additionalProperties");
            for value in values.values_mut() {
                *value = gemini_schema(std::mem::take(value));
            }
        }
        Value::Array(values) => {
            for value in values {
                *value = gemini_schema(std::mem::take(value));
            }
        }
        _ => {}
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderImage;

    #[test]
    fn parses_text_usage_and_function_calls() {
        let parsed = parse_sse_data(
            r#"{"candidates":[{"content":{"parts":[{"text":"hello"},{"functionCall":{"id":"call-1","name":"read_file","args":{"path":"README.md"}},"thoughtSignature":"opaque"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}}"#,
            "reasoning-1",
        )
        .unwrap();
        assert!(
            matches!(&parsed.events[0], ProviderEvent::TextDelta { delta } if delta == "hello")
        );
        assert!(
            matches!(&parsed.events[1], ProviderEvent::ToolCall { call } if call.name == "read_file")
        );
        assert!(
            matches!(&parsed.events[1], ProviderEvent::ToolCall { call } if call.id == "call-1" && call.metadata["thoughtSignature"] == "opaque")
        );
        assert!(parsed.completed);
    }

    #[test]
    fn parses_gemini_cache_and_thought_usage() {
        let parsed = parse_sse_data(
            r#"{"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":30,"candidatesTokenCount":20,"thoughtsTokenCount":10,"totalTokenCount":130}}"#,
            "reasoning-1",
        )
        .unwrap();

        assert!(matches!(
            parsed.events.as_slice(),
            [ProviderEvent::DetailedUsage { usage, details }]
                if *usage == TokenUsage { input_tokens: 100, output_tokens: 30, total_tokens: 130 }
                    && details.cached_input_tokens == Some(30)
                    && details.uncached_input_tokens == Some(70)
                    && details.cache_write_input_tokens.is_none()
                    && details.reasoning_output_tokens == Some(10)
        ));
    }

    #[test]
    fn parses_only_marked_gemini_thought_summaries_as_reasoning() {
        let parsed = parse_sse_data(
            r#"{"candidates":[{"content":{"parts":[{"text":"Checking the repository.","thought":true},{"text":"Final answer."}]}}]}"#,
            "reasoning-1",
        )
        .unwrap();

        assert!(matches!(
            &parsed.events[0],
            ProviderEvent::ReasoningSummaryDelta { item_id, delta }
                if item_id == "reasoning-1" && delta == "Checking the repository."
        ));
        assert!(matches!(
            &parsed.events[1],
            ProviderEvent::TextDelta { delta } if delta == "Final answer."
        ));
    }

    #[test]
    fn parses_generated_inline_images() {
        let parsed = parse_sse_data(
            r#"{"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":"AA=="}}]},"finishReason":"STOP"}]}"#,
            "reasoning-1",
        )
        .unwrap();
        assert!(matches!(
            &parsed.events[0],
            ProviderEvent::Image { mime_type, data }
                if mime_type == "image/png" && data == "AA=="
        ));
        assert!(parsed.completed);
    }

    #[test]
    fn gemini_image_generation_uses_bounded_context_and_reference_image() {
        let request = ProviderRequest {
            schema_version: crate::protocol::PROTOCOL_VERSION,
            model: "gemini-3.1-flash-image".into(),
            reasoning_effort: ReasoningEffort::Off,
            messages: vec![
                ProviderMessage::Text {
                    role: MessageRole::User,
                    text: "Draw a red umbrella".into(),
                },
                ProviderMessage::AssistantImageReference {
                    text: String::new(),
                    images: vec![ProviderImage {
                        name: "previous.png".into(),
                        data_url: "data:image/png;base64,AA==".into(),
                    }],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-1".into(),
                    name: "read_file".into(),
                    success: true,
                    output: "private tool output".into(),
                },
                ProviderMessage::Text {
                    role: MessageRole::User,
                    text: "Make the sky blue".into(),
                },
            ],
            tools: vec![],
        };

        let contents = gemini_image_contents(&request).unwrap();
        assert_eq!(contents[0]["role"], "user");
        assert!(
            contents[0]["parts"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Draw a red umbrella")
        );
        assert!(
            contents[0]["parts"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Make the sky blue")
        );
        assert!(
            !contents[0]["parts"][0]["text"]
                .as_str()
                .unwrap()
                .contains("private tool output")
        );
        assert_eq!(
            contents[0]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
    }

    #[test]
    fn rejects_invalid_or_oversized_generated_images() {
        let invalid = parse_sse_data(
            r#"{"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":"not-base64"}}]},"finishReason":"STOP"}]}"#,
            "reasoning-1",
        );
        assert!(matches!(invalid, Err(ProviderError::InvalidResponse(_))));

        let unsupported = parse_sse_data(
            r#"{"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"text/plain","data":"AA=="}}]},"finishReason":"STOP"}]}"#,
            "reasoning-1",
        );
        assert!(matches!(
            unsupported,
            Err(ProviderError::InvalidResponse(_))
        ));

        let oversized_data = vec![b'a'; MAX_GENERATED_IMAGE_BYTES + 1];
        let oversized = format!(
            r#"{{"candidates":[{{"content":{{"parts":[{{"inlineData":{{"mimeType":"image/png","data":"{}"}}}}]}},"finishReason":"STOP"}}]}}"#,
            String::from_utf8(oversized_data).unwrap()
        );
        assert!(matches!(
            parse_sse_data(&oversized, "reasoning-1"),
            Err(ProviderError::InvalidResponse(_))
        ));
    }

    #[test]
    fn bounds_gemini_image_response_body_before_json_parsing() {
        let body = vec![b' '; MAX_GEMINI_IMAGE_RESPONSE_BYTES + 1];
        assert!(matches!(
            read_image_response_body(&body, "reasoning-1"),
            Err(ProviderError::InvalidResponse(message)) if message.contains("exceeds")
        ));
    }

    #[test]
    fn detects_gemini_image_generation_models() {
        assert!(is_image_generation_model("gemini-3-pro-image-preview"));
        assert!(is_image_generation_model(
            "gemini-2.0-flash-preview-image-generation"
        ));
        assert!(!is_image_generation_model("gemini-2.5-flash"));
    }

    #[test]
    fn requests_thought_summaries_only_when_reasoning_is_enabled() {
        assert_eq!(gemini_thinking_config(ReasoningEffort::Off), None);
        assert_eq!(
            gemini_thinking_config(ReasoningEffort::High),
            Some(json!({
                "thinkingBudget": 8192,
                "includeThoughts": true
            }))
        );
    }

    #[test]
    fn serializes_gemini_function_results() {
        let contents = gemini_contents(&[
            ProviderMessage::AssistantToolCalls {
                text: "I will inspect it.".into(),
                calls: vec![ToolCall {
                    id: "call".to_string(),
                    name: "read_file".to_string(),
                    arguments: json!({ "path": "README.md" }),
                    metadata: json!({ "thoughtSignature": "opaque" }),
                }],
            },
            ProviderMessage::ToolResult {
                call_id: "call".to_string(),
                name: "read_file".to_string(),
                success: true,
                output: "docs".to_string(),
            },
        ]);
        assert_eq!(contents[0]["parts"][0]["text"], "I will inspect it.");
        assert_eq!(contents[0]["parts"][1]["thoughtSignature"], "opaque");
        assert_eq!(
            contents[1]["parts"][0]["functionResponse"]["name"],
            "read_file"
        );
        assert_eq!(contents[1]["parts"][0]["functionResponse"]["id"], "call");
    }

    #[test]
    fn removes_schema_keywords_unsupported_by_gemini_declarations() {
        let schema = gemini_schema(json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "path": { "type": "string" } }
        }));
        assert!(schema.get("additionalProperties").is_none());
    }

    #[test]
    fn serializes_image_content() {
        let contents = gemini_contents(&[ProviderMessage::UserContent {
            text: "inspect".into(),
            images: vec![ProviderImage {
                name: "screen.png".into(),
                data_url: "data:image/png;base64,AA==".into(),
            }],
        }]);
        assert_eq!(
            contents[0]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
    }

    #[test]
    fn max_tokens_is_an_incomplete_response_with_usage() {
        let parsed = parse_sse_data(
            r#"{"candidates":[{"content":{"parts":[{"text":"partial"}]},"finishReason":"MAX_TOKENS"}],"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":3}}"#,
            "reasoning",
        )
        .unwrap();
        assert!(!parsed.completed);
        assert!(matches!(
            parsed.events.last(),
            Some(ProviderEvent::Usage { usage }) if usage.total_tokens == 7
        ));
        assert!(matches!(
            parsed.terminal_error,
            Some(ProviderError::InvalidResponse(message)) if message.contains("MAX_TOKENS")
        ));
    }
}
