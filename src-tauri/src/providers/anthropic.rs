use std::collections::BTreeMap;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::common::{
    build_client, read_error_message, redact_error, redact_event, require_api_key,
};
use super::sse::SseDecoder;
use super::{
    Provider, ProviderConfig, ProviderError, ProviderEvent, ProviderMessage, ProviderRequest,
    ProviderStream,
};
use crate::protocol::{MessageRole, TokenUsage, ToolCall};

const DEFAULT_MAX_TOKENS: u64 = 8192;

pub struct AnthropicMessagesProvider {
    client: Client,
    config: ProviderConfig,
    api_key: String,
}

impl AnthropicMessagesProvider {
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
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    index: Option<u64>,
    delta: Option<AnthropicDelta>,
    content_block: Option<AnthropicContentBlock>,
    message: Option<AnthropicStartedMessage>,
    usage: Option<AnthropicUsage>,
    error: Option<AnthropicError>,
}

#[derive(Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type")]
    delta_type: Option<String>,
    text: Option<String>,
    partial_json: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    id: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicStartedMessage {
    usage: Option<AnthropicUsage>,
}

#[derive(Default, Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct AnthropicError {
    message: String,
}

struct ToolStart {
    index: u64,
    id: String,
    name: String,
}

struct ParsedAnthropicEvent {
    delta: Option<String>,
    tool_start: Option<ToolStart>,
    tool_json_delta: Option<(u64, String)>,
    tool_stop: Option<u64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    completed: bool,
}

struct PendingTool {
    id: String,
    name: String,
    arguments: String,
}

#[async_trait]
impl Provider for AnthropicMessagesProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        let endpoint = self
            .config
            .anthropic_messages_url()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        let mut payload = json!({
            "model": request.model,
            "messages": anthropic_messages(&request.messages),
            "max_tokens": DEFAULT_MAX_TOKENS,
            "stream": true
        });
        if !request.tools.is_empty() {
            payload["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool.name,
                            "description": tool.description,
                            "input_schema": tool.input_schema
                        })
                    })
                    .collect(),
            );
        }

        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            response = self.client
                .post(endpoint)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
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
            let mut usage = TokenUsage::default();
            let mut pending_tools = BTreeMap::<u64, PendingTool>::new();
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
                                    if let Some(delta) = parsed.delta { yield Ok(redact_event(ProviderEvent::TextDelta { delta }, &secret)); }
                                    if let Some(start) = parsed.tool_start {
                                        pending_tools.insert(start.index, PendingTool { id: start.id, name: start.name, arguments: String::new() });
                                    }
                                    if let Some((index, delta)) = parsed.tool_json_delta {
                                        let Some(tool) = pending_tools.get_mut(&index) else {
                                            yield Err(ProviderError::InvalidResponse("Anthropic sent tool arguments before tool_use start".to_string()));
                                            return;
                                        };
                                        tool.arguments.push_str(&delta);
                                    }
                                    if let Some(index) = parsed.tool_stop {
                                        if let Some(tool) = pending_tools.remove(&index) {
                                            let arguments = if tool.arguments.is_empty() { json!({}) } else {
                                                match serde_json::from_str(&tool.arguments) {
                                                    Ok(arguments) => arguments,
                                                    Err(error) => { yield Err(ProviderError::InvalidResponse(format!("tool call {} returned invalid JSON arguments: {error}", tool.name))); return; }
                                                }
                                            };
                                            yield Ok(ProviderEvent::ToolCall { call: ToolCall {
                                                id: tool.id,
                                                name: tool.name,
                                                arguments,
                                                metadata: json!({}),
                                            } });
                                        }
                                    }
                                    if parsed.input_tokens.is_some() || parsed.output_tokens.is_some() {
                                        if let Some(value) = parsed.input_tokens { usage.input_tokens = value; }
                                        if let Some(value) = parsed.output_tokens { usage.output_tokens = value; }
                                        usage.total_tokens = usage.input_tokens + usage.output_tokens;
                                        yield Ok(ProviderEvent::Usage { usage });
                                    }
                                    if parsed.completed {
                                        if !pending_tools.is_empty() {
                                            yield Err(ProviderError::InvalidResponse("Anthropic completed with an incomplete tool call".to_string()));
                                            return;
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

fn anthropic_messages(messages: &[ProviderMessage]) -> Vec<Value> {
    let mut result = Vec::<Value>::new();
    for message in messages {
        match message {
            ProviderMessage::Text { role, text } => result.push(json!({
                "role": match role { MessageRole::User => "user", MessageRole::Assistant => "assistant" },
                "content": text
            })),
            ProviderMessage::UserContent { text, images } => result.push(json!({
                "role": "user",
                "content": std::iter::once(json!({ "type": "text", "text": text }))
                    .chain(images.iter().filter_map(|image| {
                        let (media_type, data) = super::split_image_data_url(&image.data_url)?;
                        Some(json!({
                            "type": "image",
                            "source": { "type": "base64", "media_type": media_type, "data": data }
                        }))
                    }))
                    .collect::<Vec<_>>()
            })),
            ProviderMessage::AssistantToolCalls { calls } => result.push(json!({
                "role": "assistant",
                "content": calls.iter().map(|call| json!({
                    "type": "tool_use", "id": call.id, "name": call.name, "input": call.arguments
                })).collect::<Vec<_>>()
            })),
            ProviderMessage::ToolResult { call_id, success, output, .. } => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": output,
                    "is_error": !success
                });
                if let Some(last) = result.last_mut().filter(|last| {
                    last["role"] == "user" && last["content"].is_array()
                }) {
                    last["content"].as_array_mut().unwrap().push(block);
                } else {
                    result.push(json!({ "role": "user", "content": [block] }));
                }
            }
            ProviderMessage::ProviderContext { .. } => {}
        }
    }
    result
}

fn parse_sse_data(data: &str) -> Result<ParsedAnthropicEvent, ProviderError> {
    let event: AnthropicStreamEvent = serde_json::from_str(data).map_err(|error| {
        ProviderError::InvalidResponse(format!("malformed Anthropic event: {error}"))
    })?;
    if event.event_type == "error" {
        return Err(ProviderError::InvalidResponse(
            event
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "Anthropic API reported an error".to_string()),
        ));
    }
    let delta = event.delta.as_ref().and_then(|delta| {
        (delta.delta_type.as_deref() == Some("text_delta"))
            .then(|| delta.text.clone())
            .flatten()
            .filter(|text| !text.is_empty())
    });
    let tool_start = match (event.event_type.as_str(), event.index, event.content_block) {
        ("content_block_start", Some(index), Some(block)) if block.block_type == "tool_use" => {
            match (block.id, block.name) {
                (Some(id), Some(name)) => Some(ToolStart { index, id, name }),
                _ => {
                    return Err(ProviderError::InvalidResponse(
                        "Anthropic returned an incomplete tool_use block".to_string(),
                    ));
                }
            }
        }
        _ => None,
    };
    let tool_json_delta = event.delta.as_ref().and_then(|delta| {
        (delta.delta_type.as_deref() == Some("input_json_delta"))
            .then(|| Some((event.index?, delta.partial_json.clone()?)))
            .flatten()
    });
    let tool_stop = (event.event_type == "content_block_stop")
        .then_some(event.index)
        .flatten();
    let started_usage = event.message.and_then(|message| message.usage);
    let input_tokens = started_usage
        .as_ref()
        .and_then(|usage| usage.input_tokens)
        .or_else(|| event.usage.as_ref().and_then(|usage| usage.input_tokens));
    let output_tokens = event
        .usage
        .as_ref()
        .and_then(|usage| usage.output_tokens)
        .or_else(|| started_usage.and_then(|usage| usage.output_tokens));
    Ok(ParsedAnthropicEvent {
        delta,
        tool_start,
        tool_json_delta,
        tool_stop,
        input_tokens,
        output_tokens,
        completed: event.event_type == "message_stop",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderImage;

    #[test]
    fn parses_text_and_tool_lifecycle() {
        let delta = parse_sse_data(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#).unwrap();
        assert_eq!(delta.delta.as_deref(), Some("hello"));
        let start = parse_sse_data(r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call","name":"read_file","input":{}}}"#).unwrap();
        assert_eq!(start.tool_start.unwrap().name, "read_file");
        let args = parse_sse_data(r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a\"}"}}"#).unwrap();
        assert_eq!(args.tool_json_delta.unwrap().1, r#"{"path":"a"}"#);
        assert!(
            parse_sse_data(r#"{"type":"message_stop"}"#)
                .unwrap()
                .completed
        );
    }

    #[test]
    fn serializes_anthropic_tool_results_as_user_blocks() {
        let messages = anthropic_messages(&[ProviderMessage::ToolResult {
            call_id: "call".to_string(),
            name: "read_file".to_string(),
            success: false,
            output: "denied".to_string(),
        }]);
        assert_eq!(messages[0]["content"][0]["type"], "tool_result");
        assert_eq!(messages[0]["content"][0]["is_error"], true);
    }

    #[test]
    fn serializes_image_content() {
        let messages = anthropic_messages(&[ProviderMessage::UserContent {
            text: "inspect".into(),
            images: vec![ProviderImage {
                name: "screen.png".into(),
                data_url: "data:image/png;base64,AA==".into(),
            }],
        }]);
        assert_eq!(
            messages[0]["content"][1]["source"]["media_type"],
            "image/png"
        );
    }
}
