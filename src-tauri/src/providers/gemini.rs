use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::common::{
    build_client, read_error_message, redact_error, redact_event, require_api_key,
};
use super::sse::SseDecoder;
use super::{
    Provider, ProviderConfig, ProviderError, ProviderEvent, ProviderMessage, ProviderRequest,
    ProviderStream,
};
use crate::protocol::{MessageRole, TokenUsage, ToolCall};

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
    thought_signature: Option<String>,
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
}

#[async_trait]
impl Provider for GoogleGeminiProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        let endpoint = self
            .config
            .gemini_stream_url()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        let mut payload = json!({ "contents": gemini_contents(&request.messages) });
        if !request.tools.is_empty() {
            payload["tools"] = json!([{
                "functionDeclarations": request.tools.iter().map(|tool| json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": gemini_schema(tool.input_schema.clone())
                })).collect::<Vec<_>>()
            }]);
        }

        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            response = self.client
                .post(endpoint)
                .header("x-goog-api-key", &self.api_key)
                .header("accept", "text/event-stream")
                .json(&payload)
                .send() => response.map_err(|error| ProviderError::Request(error.to_string()))?,
        };
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = read_error_message(response, &cancellation, &self.api_key).await?;
            return Err(ProviderError::Http { status, message });
        }

        let secret = self.api_key.clone();
        Ok(Box::pin(async_stream::stream! {
            let mut body = response.bytes_stream();
            let mut decoder = SseDecoder::default();
            loop {
                let chunk = tokio::select! {
                    _ = cancellation.cancelled() => { yield Err(ProviderError::Cancelled); return; }
                    chunk = body.next() => chunk,
                };
                match chunk {
                    Some(Ok(bytes)) => match decoder.push(&bytes) {
                        Ok(frames) => for frame in frames {
                            match parse_sse_data(&frame) {
                                Ok(parsed) => {
                                    for event in parsed.events.into_iter().map(|event| redact_event(event, &secret)) { yield Ok(event); }
                                    if parsed.completed { yield Ok(ProviderEvent::Completed); return; }
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

fn gemini_contents(messages: &[ProviderMessage]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| match message {
            ProviderMessage::Text { role, text } => Some(json!({
                "role": match role { MessageRole::User => "user", MessageRole::Assistant => "model" },
                "parts": [{ "text": text }]
            })),
            ProviderMessage::UserContent { text, images } => Some(json!({
                "role": "user",
                "parts": std::iter::once(json!({ "text": text }))
                    .chain(images.iter().filter_map(|image| {
                        let (mime_type, data) = super::split_image_data_url(&image.data_url)?;
                        Some(json!({ "inlineData": { "mimeType": mime_type, "data": data } }))
                    }))
                    .collect::<Vec<_>>()
            })),
            ProviderMessage::AssistantToolCalls { calls } => {
                let parts = calls.iter().map(|call| {
                    let mut part = json!({
                        "functionCall": { "id": call.id, "name": call.name, "args": call.arguments }
                    });
                    if let Some(signature) = call.metadata["thoughtSignature"].as_str() {
                        part["thoughtSignature"] = Value::String(signature.to_string());
                    }
                    part
                }).collect::<Vec<_>>();
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

fn parse_sse_data(data: &str) -> Result<ParsedGeminiEvent, ProviderError> {
    let response: GeminiResponse = serde_json::from_str(data).map_err(|error| {
        ProviderError::InvalidResponse(format!("malformed Gemini event: {error}"))
    })?;
    if let Some(error) = response.error {
        return Err(ProviderError::InvalidResponse(error.message));
    }
    if let Some(reason) = response
        .prompt_feedback
        .and_then(|feedback| feedback.block_reason)
        .filter(|reason| !reason.is_empty() && reason != "BLOCK_REASON_UNSPECIFIED")
    {
        return Err(ProviderError::InvalidResponse(format!(
            "Gemini blocked the prompt: {reason}"
        )));
    }
    let mut events = Vec::new();
    let candidate = response.candidates.into_iter().next();
    if let Some(candidate) = candidate.as_ref() {
        if let Some(reason) = candidate.finish_reason.as_deref().filter(|reason| {
            !reason.is_empty()
                && !matches!(*reason, "FINISH_REASON_UNSPECIFIED" | "STOP" | "MAX_TOKENS")
        }) {
            return Err(ProviderError::InvalidResponse(
                candidate
                    .finish_message
                    .clone()
                    .unwrap_or_else(|| format!("Gemini stopped generation: {reason}")),
            ));
        }
        if let Some(content) = &candidate.content {
            for part in &content.parts {
                if let Some(delta) = part.text.clone().filter(|delta| !delta.is_empty()) {
                    events.push(ProviderEvent::TextDelta { delta });
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
        events.push(ProviderEvent::Usage {
            usage: TokenUsage {
                input_tokens: usage.prompt_token_count,
                output_tokens: usage.candidates_token_count,
                total_tokens: usage
                    .total_token_count
                    .unwrap_or(usage.prompt_token_count + usage.candidates_token_count),
            },
        });
    }
    let completed = candidate
        .and_then(|candidate| candidate.finish_reason)
        .is_some_and(|reason| !reason.is_empty() && reason != "FINISH_REASON_UNSPECIFIED");
    Ok(ParsedGeminiEvent { events, completed })
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
        let parsed = parse_sse_data(r#"{"candidates":[{"content":{"parts":[{"text":"hello"},{"functionCall":{"id":"call-1","name":"read_file","args":{"path":"README.md"}},"thoughtSignature":"opaque"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}}"#).unwrap();
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
    fn serializes_gemini_function_results() {
        let contents = gemini_contents(&[
            ProviderMessage::AssistantToolCalls {
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
        assert_eq!(contents[0]["parts"][0]["thoughtSignature"], "opaque");
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
}
