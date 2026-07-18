use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::sse::SseDecoder;
use super::{
    Provider, ProviderConfig, ProviderError, ProviderEvent, ProviderRequest, ProviderStream,
};
use crate::protocol::{MessageRole, TokenUsage};

const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;

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

#[derive(Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage>,
    stream: bool,
    stream_options: StreamOptions,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: &'static str,
    content: String,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
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
    delta: OpenAiDelta,
}

#[derive(Default, Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
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
        let payload = OpenAiRequest {
            model: &request.model,
            messages: request
                .messages
                .iter()
                .map(|message| OpenAiMessage {
                    role: match message.role {
                        MessageRole::User => "user",
                        MessageRole::Assistant => "assistant",
                    },
                    content: message.text(),
                })
                .collect(),
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
        };

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

        let redaction_secret = self.api_key.clone();
        Ok(Box::pin(async_stream::stream! {
            let mut body = response.bytes_stream();
            let mut decoder = SseDecoder::default();

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
                                    Ok(events) => {
                                        for event in events.into_iter().map(|event| redact_event(event, &redaction_secret)) {
                                            let completed = matches!(event, ProviderEvent::Completed);
                                            yield Ok(event);
                                            if completed {
                                                return;
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        yield Err(redact_error(error, &redaction_secret));
                                        return;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            yield Err(error);
                            return;
                        }
                    },
                    Some(Err(error)) => {
                        yield Err(redact_error(ProviderError::Request(error.to_string()), &redaction_secret));
                        return;
                    }
                    None => {
                        match decoder.finish() {
                            Ok(()) => yield Err(ProviderError::Interrupted),
                            Err(error) => yield Err(error),
                        }
                        return;
                    }
                }
            }
        }))
    }
}

fn parse_sse_data(data: &str) -> Result<Vec<ProviderEvent>, ProviderError> {
    if data.trim() == "[DONE]" {
        return Ok(vec![ProviderEvent::Completed]);
    }

    let chunk: OpenAiChunk = serde_json::from_str(data).map_err(|error| {
        ProviderError::InvalidResponse(format!("malformed Chat Completions event: {error}"))
    })?;
    if let Some(error) = chunk.error {
        return Err(ProviderError::InvalidResponse(error.message));
    }

    let mut events = chunk
        .choices
        .into_iter()
        .filter_map(|choice| choice.delta.content)
        .filter(|content| !content.is_empty())
        .map(|delta| ProviderEvent::TextDelta { delta })
        .collect::<Vec<_>>();
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
    Ok(events)
}

async fn read_error_message(
    mut response: reqwest::Response,
    cancellation: &CancellationToken,
    secret: &str,
) -> Result<String, ProviderError> {
    let mut bytes = Vec::new();
    while bytes.len() < MAX_ERROR_BODY_BYTES {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            chunk = response.chunk() => chunk,
        };
        match chunk {
            Ok(Some(chunk)) => {
                let remaining = MAX_ERROR_BODY_BYTES - bytes.len();
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }

    let text = String::from_utf8_lossy(&bytes).trim().to_string();
    let message = serde_json::from_str::<OpenAiChunk>(&text)
        .ok()
        .and_then(|body| body.error)
        .map(|error| error.message)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| {
            if text.is_empty() {
                "provider returned an empty error response".to_string()
            } else {
                text
            }
        });
    Ok(redact(&message, secret))
}

fn redact_event(event: ProviderEvent, secret: &str) -> ProviderEvent {
    match event {
        ProviderEvent::TextDelta { delta } => ProviderEvent::TextDelta {
            delta: redact(&delta, secret),
        },
        other => other,
    }
}

fn redact_error(error: ProviderError, secret: &str) -> ProviderError {
    match error {
        ProviderError::Request(message) => ProviderError::Request(redact(&message, secret)),
        ProviderError::Http { status, message } => ProviderError::Http {
            status,
            message: redact(&message, secret),
        },
        ProviderError::InvalidResponse(message) => {
            ProviderError::InvalidResponse(redact(&message, secret))
        }
        other => other,
    }
}

fn redact(value: &str, secret: &str) -> String {
    if secret.is_empty() {
        value.to_string()
    } else {
        value.replace(secret, "[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_usage_and_completion_fixtures() {
        let text =
            parse_sse_data(r#"{"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#)
                .expect("text fixture should parse");
        let usage = parse_sse_data(
            r#"{"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#,
        )
        .expect("usage fixture should parse");
        let completed = parse_sse_data("[DONE]").expect("completion fixture should parse");

        assert_eq!(
            text,
            vec![ProviderEvent::TextDelta {
                delta: "hello".to_string()
            }]
        );
        assert_eq!(
            usage,
            vec![ProviderEvent::Usage {
                usage: TokenUsage {
                    input_tokens: 3,
                    output_tokens: 2,
                    total_tokens: 5,
                }
            }]
        );
        assert_eq!(completed, vec![ProviderEvent::Completed]);
    }

    #[test]
    fn rejects_malformed_and_error_payloads() {
        assert!(matches!(
            parse_sse_data("{not-json"),
            Err(ProviderError::InvalidResponse(_))
        ));
        assert_eq!(
            parse_sse_data(r#"{"error":{"message":"bad request"},"choices":[]}"#),
            Err(ProviderError::InvalidResponse("bad request".to_string()))
        );
    }

    #[test]
    fn redacts_api_keys_from_provider_controlled_content() {
        let secret = "secret-value";
        assert_eq!(
            redact_event(
                ProviderEvent::TextDelta {
                    delta: "echo secret-value".to_string()
                },
                secret
            ),
            ProviderEvent::TextDelta {
                delta: "echo [REDACTED]".to_string()
            }
        );
        assert_eq!(
            redact_error(
                ProviderError::InvalidResponse("secret-value leaked".to_string()),
                secret
            ),
            ProviderError::InvalidResponse("[REDACTED] leaked".to_string())
        );
    }
}
