use std::collections::HashSet;

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

pub struct OpenAiResponsesProvider {
    client: Client,
    config: ProviderConfig,
    api_key: String,
}

impl OpenAiResponsesProvider {
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
struct ResponsesStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<String>,
    item_id: Option<String>,
    text: Option<String>,
    response: Option<ResponsesObject>,
    item: Option<Value>,
    error: Option<ResponsesError>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesObject {
    usage: Option<ResponsesUsage>,
    error: Option<ResponsesError>,
}

#[derive(Deserialize)]
struct ResponsesUsage {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct ResponsesError {
    message: String,
}

struct ParsedResponsesEvent {
    events: Vec<ProviderEvent>,
    completed: bool,
}

#[async_trait]
impl Provider for OpenAiResponsesProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        let endpoint = self
            .config
            .responses_url()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        let mut payload = json!({
            "model": request.model,
            "input": responses_input(&request.messages),
            "stream": true,
        });
        if let Some(effort) = request.reasoning_effort.openai_value() {
            payload["reasoning"] = json!({ "effort": effort, "summary": "auto" });
        }
        if !request.tools.is_empty() {
            payload["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.input_schema
                        })
                    })
                    .collect(),
            );
        }

        let mut response = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            response = self.client
                .post(endpoint.clone())
                .bearer_auth(&self.api_key)
                .header("accept", "text/event-stream")
                .json(&payload)
                .send() => response.map_err(|error| ProviderError::Request(error.to_string()))?,
        };
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = read_error_message(response, &cancellation, &self.api_key).await?;
            if should_retry_without_reasoning_summary(status, &message) {
                if let Some(payload) = payload.as_object_mut() {
                    payload.remove("reasoning");
                }
                response = tokio::select! {
                    _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
                    response = self.client
                        .post(endpoint)
                        .bearer_auth(&self.api_key)
                        .header("accept", "text/event-stream")
                        .json(&payload)
                        .send() => response.map_err(|error| ProviderError::Request(error.to_string()))?,
                };
                if !response.status().is_success() {
                    let status = response.status().as_u16();
                    let message =
                        read_error_message(response, &cancellation, &self.api_key).await?;
                    return Err(ProviderError::Http { status, message });
                }
            } else {
                return Err(ProviderError::Http { status, message });
            }
        }

        let secret = self.api_key.clone();
        Ok(Box::pin(async_stream::stream! {
            let mut body = response.bytes_stream();
            let mut decoder = SseDecoder::default();
            let mut emitted_calls = HashSet::new();
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
                                    for event in parsed.events.into_iter().map(|event| redact_event(event, &secret)) {
                                        if let ProviderEvent::ToolCall { call } = &event {
                                            if !emitted_calls.insert(call.id.clone()) { continue; }
                                        }
                                        yield Ok(event);
                                    }
                                    if parsed.completed { yield Ok(ProviderEvent::Completed); return; }
                                }
                                Err(error) => { yield Err(redact_error(error, &secret)); return; }
                            }
                        },
                        Err(error) => { yield Err(error); return; }
                    },
                    Some(Err(error)) => {
                        yield Err(redact_error(ProviderError::Request(error.to_string()), &secret));
                        return;
                    }
                    None => {
                        yield Err(decoder.finish().err().unwrap_or(ProviderError::Interrupted));
                        return;
                    }
                }
            }
        }))
    }
}

fn should_retry_without_reasoning_summary(status: u16, message: &str) -> bool {
    matches!(status, 400 | 422) && {
        let message = message.to_ascii_lowercase();
        message.contains("reasoning") || message.contains("summary")
    }
}

fn responses_input(messages: &[ProviderMessage]) -> Vec<Value> {
    messages
        .iter()
        .flat_map(|message| match message {
            ProviderMessage::Text { role, text } => vec![json!({
                "role": match role { MessageRole::User => "user", MessageRole::Assistant => "assistant", MessageRole::System => "developer" },
                "content": text
            })],
            ProviderMessage::UserContent { text, images } => vec![json!({
                "role": "user",
                "content": std::iter::once(json!({ "type": "input_text", "text": text }))
                    .chain(images.iter().map(|image| json!({
                        "type": "input_image",
                        "image_url": image.data_url
                    })))
                    .collect::<Vec<_>>()
            })],
            ProviderMessage::AssistantToolCalls { text, calls } => std::iter::once(text)
                .filter(|text| !text.is_empty())
                .map(|text| json!({ "role": "assistant", "content": text }))
                .chain(calls.iter().map(|call| {
                    let mut item = json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": call.arguments.to_string()
                    });
                    if let Some(id) = call.metadata["itemId"].as_str() {
                        item["id"] = Value::String(id.to_string());
                    }
                    item
                }))
                .collect(),
            ProviderMessage::ToolResult { call_id, output, .. } => vec![json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output
            })],
            ProviderMessage::ProviderContext { provider, item }
                if provider == "openai_responses" => vec![item.clone()],
            ProviderMessage::ProviderContext { .. } => Vec::new(),
        })
        .collect()
}

fn parse_sse_data(data: &str) -> Result<ParsedResponsesEvent, ProviderError> {
    let event: ResponsesStreamEvent = serde_json::from_str(data).map_err(|error| {
        ProviderError::InvalidResponse(format!("malformed Responses API event: {error}"))
    })?;
    if event.event_type == "error" || event.event_type == "response.failed" {
        let message = event
            .error
            .or_else(|| event.response.and_then(|response| response.error))
            .map(|error| error.message)
            .or(event.message)
            .unwrap_or_else(|| "Responses API reported an error".to_string());
        return Err(ProviderError::InvalidResponse(message));
    }

    let mut events = Vec::new();
    if event.event_type == "response.output_text.delta" {
        if let Some(delta) = event.delta.as_ref().filter(|delta| !delta.is_empty()) {
            let delta = delta.clone();
            events.push(ProviderEvent::TextDelta { delta });
        }
    }
    if event.event_type == "response.reasoning_summary_text.delta" {
        let item_id = event.item_id.clone().ok_or_else(|| {
            ProviderError::InvalidResponse(
                "Responses API reasoning summary delta omitted item_id".to_string(),
            )
        })?;
        if let Some(delta) = event.delta.as_ref().filter(|delta| !delta.is_empty()) {
            let delta = delta.clone();
            events.push(ProviderEvent::ReasoningSummaryDelta { item_id, delta });
        }
    }
    if event.event_type == "response.reasoning_summary_text.done" {
        let item_id = event.item_id.clone().ok_or_else(|| {
            ProviderError::InvalidResponse(
                "Responses API reasoning summary completion omitted item_id".to_string(),
            )
        })?;
        let summary = event.text.unwrap_or_default();
        events.push(ProviderEvent::ReasoningSummaryCompleted { item_id, summary });
    }
    if event.event_type == "response.output_item.done" {
        let item = event.item.ok_or_else(|| {
            ProviderError::InvalidResponse(
                "Responses API output_item.done event omitted its item".to_string(),
            )
        })?;
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let call_id = item.get("call_id").and_then(Value::as_str).ok_or_else(|| {
                    ProviderError::InvalidResponse(
                        "Responses API function call omitted call_id".to_string(),
                    )
                })?;
                let name = item.get("name").and_then(Value::as_str).ok_or_else(|| {
                    ProviderError::InvalidResponse(
                        "Responses API function call omitted name".to_string(),
                    )
                })?;
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ProviderError::InvalidResponse(
                            "Responses API function call omitted arguments".to_string(),
                        )
                    })?;
                let arguments = serde_json::from_str(&arguments).map_err(|error| {
                    ProviderError::InvalidResponse(format!(
                        "function call {name} returned invalid JSON arguments: {error}"
                    ))
                })?;
                events.push(ProviderEvent::ToolCall {
                    call: ToolCall {
                        id: call_id.to_string(),
                        name: name.to_string(),
                        arguments,
                        metadata: item
                            .get("id")
                            .and_then(Value::as_str)
                            .map(|id| json!({ "itemId": id }))
                            .unwrap_or_else(|| json!({})),
                    },
                });
            }
            Some("reasoning") => events.push(ProviderEvent::ProviderContext {
                provider: "openai_responses".to_string(),
                item,
            }),
            _ => {}
        }
    }
    if let Some(usage) = event
        .response
        .as_ref()
        .and_then(|response| response.usage.as_ref())
    {
        events.push(ProviderEvent::Usage {
            usage: TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                total_tokens: usage
                    .total_tokens
                    .unwrap_or(usage.input_tokens + usage.output_tokens),
            },
        });
    }
    let completed = matches!(
        event.event_type.as_str(),
        "response.completed" | "response.incomplete"
    );
    Ok(ParsedResponsesEvent { events, completed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderImage;

    #[test]
    fn parses_text_completion_and_tool_calls() {
        let delta =
            parse_sse_data(r#"{"type":"response.output_text.delta","delta":"hello"}"#).unwrap();
        assert!(matches!(&delta.events[0], ProviderEvent::TextDelta { delta } if delta == "hello"));
        let tool = parse_sse_data(r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"call-1","name":"read_file","arguments":"{\"path\":\"README.md\"}"}}"#).unwrap();
        assert!(
            matches!(&tool.events[0], ProviderEvent::ToolCall { call } if call.name == "read_file")
        );
        let reasoning = parse_sse_data(r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","encrypted_content":"opaque"}}"#).unwrap();
        assert!(matches!(
            &reasoning.events[0],
            ProviderEvent::ProviderContext { provider, .. } if provider == "openai_responses"
        ));
        assert!(parse_sse_data(r#"{"type":"response.completed","response":{"usage":{"input_tokens":3,"output_tokens":2}}}"#).unwrap().completed);
    }

    #[test]
    fn parses_only_safe_reasoning_summary_events() {
        let delta = parse_sse_data(
            r#"{"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"Checking files"}"#,
        )
        .unwrap();
        assert!(matches!(
            &delta.events[0],
            ProviderEvent::ReasoningSummaryDelta { item_id, delta }
                if item_id == "rs_1" && delta == "Checking files"
        ));

        let done = parse_sse_data(
            r#"{"type":"response.reasoning_summary_text.done","item_id":"rs_1","text":"Checked the relevant files."}"#,
        )
        .unwrap();
        assert!(matches!(
            &done.events[0],
            ProviderEvent::ReasoningSummaryCompleted { item_id, summary }
                if item_id == "rs_1" && summary == "Checked the relevant files."
        ));

        let raw_reasoning = parse_sse_data(
            r#"{"type":"response.reasoning_text.delta","item_id":"rs_1","delta":"private chain of thought"}"#,
        )
        .unwrap();
        assert!(raw_reasoning.events.is_empty());
    }

    #[test]
    fn only_retries_parameter_errors_that_name_reasoning() {
        assert!(should_retry_without_reasoning_summary(
            400,
            "Unsupported parameter: reasoning.summary"
        ));
        assert!(should_retry_without_reasoning_summary(
            422,
            "reasoning is not supported"
        ));
        assert!(!should_retry_without_reasoning_summary(
            401,
            "reasoning is not supported"
        ));
        assert!(!should_retry_without_reasoning_summary(
            400,
            "model is invalid"
        ));
    }

    #[test]
    fn serializes_responses_tool_history() {
        let input = responses_input(&[
            ProviderMessage::AssistantToolCalls {
                text: "I will inspect it.".into(),
                calls: vec![ToolCall {
                    id: "call".into(),
                    name: "read_file".into(),
                    arguments: json!({ "path": "README.md" }),
                    metadata: json!({}),
                }],
            },
            ProviderMessage::ToolResult {
                call_id: "call".to_string(),
                name: "read_file".to_string(),
                success: true,
                output: "docs".to_string(),
            },
        ]);
        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[2]["type"], "function_call_output");
    }

    #[test]
    fn serializes_image_content() {
        let input = responses_input(&[ProviderMessage::UserContent {
            text: "inspect".into(),
            images: vec![ProviderImage {
                name: "screen.png".into(),
                data_url: "data:image/png;base64,AA==".into(),
            }],
        }]);
        assert_eq!(input[0]["content"][1]["type"], "input_image");
    }
}
