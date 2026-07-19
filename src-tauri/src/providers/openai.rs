use std::collections::BTreeMap;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::common::{read_error_message, redact_error, redact_event};
use super::sse::SseDecoder;
use super::{
    Provider, ProviderConfig, ProviderError, ProviderEvent, ProviderMessage, ProviderRequest,
    ProviderStream,
};
use crate::protocol::{MessageRole, TokenUsage, ToolCall};

pub struct OpenAiChatCompletionsProvider {
    client: Client,
    config: ProviderConfig,
    api_key: String,
}

impl OpenAiChatCompletionsProvider {
    pub fn new(config: ProviderConfig, api_key: String) -> Result<Self, ProviderError> {
        if api_key.trim().is_empty() {
            return Err(ProviderError::Request(
                "API key is not configured".to_string(),
            ));
        }
        let config = config
            .validate()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        Ok(Self {
            client,
            config,
            api_key,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameCompletion {
    None,
    FinishReason,
    DoneMarker,
}

struct ParsedSseData {
    events: Vec<ProviderEvent>,
    tool_deltas: Vec<OpenAiToolCallDelta>,
    completion: FrameCompletion,
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
                        "Chat Completions returned an incomplete tool call".to_string(),
                    ));
                }
                let arguments = serde_json::from_str(&pending.arguments).map_err(|error| {
                    ProviderError::InvalidResponse(format!(
                        "tool call {} returned invalid JSON arguments: {error}",
                        pending.name
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
        let mut payload = json!({
            "model": request.model,
            "messages": chat_messages(&request.messages),
            "stream": true,
            "stream_options": { "include_usage": true }
        });
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

        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            response = self.client
                .post(endpoint)
                .bearer_auth(&self.api_key)
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
            let mut tool_calls = ToolCallAccumulator::default();
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
                                        for event in parsed.events.into_iter().map(|event| redact_event(event, &secret)) { yield Ok(event); }
                                        if parsed.completion == FrameCompletion::FinishReason {
                                            saw_finish_reason = true;
                                            match tool_calls.take() {
                                                Ok(calls) => for call in calls { yield Ok(ProviderEvent::ToolCall { call }); },
                                                Err(error) => { yield Err(error); return; }
                                            }
                                        }
                                        if parsed.completion == FrameCompletion::DoneMarker {
                                            match tool_calls.take() {
                                                Ok(calls) => for call in calls { yield Ok(ProviderEvent::ToolCall { call }); },
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

fn chat_messages(messages: &[ProviderMessage]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| match message {
            ProviderMessage::Text { role, text } => Some(json!({
                "role": match role { MessageRole::User => "user", MessageRole::Assistant => "assistant" },
                "content": text
            })),
            ProviderMessage::AssistantToolCalls { calls } => Some(json!({
                "role": "assistant",
                "content": null,
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

fn parse_sse_data(data: &str) -> Result<ParsedSseData, ProviderError> {
    if data.trim() == "[DONE]" {
        return Ok(ParsedSseData {
            events: Vec::new(),
            tool_deltas: Vec::new(),
            completion: FrameCompletion::DoneMarker,
        });
    }
    let chunk: OpenAiChunk = serde_json::from_str(data).map_err(|error| {
        ProviderError::InvalidResponse(format!("malformed Chat Completions event: {error}"))
    })?;
    if let Some(error) = chunk.error {
        return Err(ProviderError::InvalidResponse(error.message));
    }
    let completion = if chunk
        .choices
        .iter()
        .any(|choice| choice.finish_reason.is_some())
    {
        FrameCompletion::FinishReason
    } else {
        FrameCompletion::None
    };
    let mut events = Vec::new();
    let mut tool_deltas = Vec::new();
    for choice in chunk.choices {
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
        tool_deltas,
        completion,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn serializes_structured_tool_history() {
        let messages = chat_messages(&[
            ProviderMessage::AssistantToolCalls {
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
        assert_eq!(messages[1]["role"], "tool");
    }
}
