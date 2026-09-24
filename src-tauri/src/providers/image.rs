use base64::Engine;
use futures_util::StreamExt;
use reqwest::{Client, Response};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::common::{read_error_details, redact_error, require_api_key};
use super::{
    Provider, ProviderConfig, ProviderError, ProviderEvent, ProviderMessage, ProviderRequest,
    ProviderStream,
};
use crate::protocol::MessageRole;

const IMAGE_REQUEST_TIMEOUT_SECONDS: u64 = 300;
const MAX_PROMPT_BYTES: usize = 32 * 1024;
const IMAGE_CONTEXT_HEADER: &str = "Previous conversation context (oldest first):\n";
const CURRENT_IMAGE_REQUEST_HEADER: &str = "\nCurrent image request:\n";
const MAX_IMAGE_INPUT_BYTES: usize = 12 * 1024 * 1024;
const MAX_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_GENERATED_IMAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageResponseFormat {
    B64Json,
}

impl ImageResponseFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::B64Json => "b64_json",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageGenerationRequest {
    pub model: String,
    pub prompt: String,
    pub response_format: ImageResponseFormat,
    pub cfg_scale: Option<f64>,
    pub steps: Option<u32>,
    pub seed: Option<u64>,
    pub text_mode: Option<bool>,
    pub image: Option<String>,
    pub mask: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedImage {
    pub mime_type: String,
    pub data: String,
}

pub struct OpenAiImageGenerationsProvider {
    client: Client,
    config: ProviderConfig,
    api_key: String,
}

impl OpenAiImageGenerationsProvider {
    pub fn new(config: ProviderConfig, api_key: String) -> Result<Self, ProviderError> {
        require_api_key(&api_key)?;
        let config = config
            .validate()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(
                IMAGE_REQUEST_TIMEOUT_SECONDS,
            ))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        Ok(Self {
            client,
            config,
            api_key,
        })
    }

    async fn send_request(
        &self,
        endpoint: reqwest::Url,
        request: &ImageGenerationRequest,
        cancellation: &CancellationToken,
    ) -> Result<Response, ProviderError> {
        let mut builder = self
            .client
            .post(endpoint)
            .bearer_auth(&self.api_key)
            .header(reqwest::header::ACCEPT, "application/json");
        if request.image.is_some() || request.mask.is_some() {
            builder = builder.multipart(build_edit_form(request)?);
        } else {
            builder = builder.json(&build_payload(request)?);
        }
        tokio::select! {
            _ = cancellation.cancelled() => Err(ProviderError::Cancelled),
            response = builder.send() => response.map_err(|error| ProviderError::Request(error.to_string())),
        }
    }

    /// Send one OpenAI Images-compatible request and normalize its base64
    /// results.  Chat turns use this through the `Provider` implementation,
    /// while direct callers can reuse the same adapter without duplicating
    /// the wire protocol.
    pub async fn generate(
        &self,
        request: ImageGenerationRequest,
        cancellation: &CancellationToken,
    ) -> Result<Vec<GeneratedImage>, ProviderError> {
        let endpoint = if request.image.is_some() || request.mask.is_some() {
            self.config.images_edits_url()
        } else {
            self.config.images_generations_url()
        }
        .map_err(|error| ProviderError::Request(error.to_string()))?;
        let response = self.send_request(endpoint, &request, cancellation).await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = super::common::retry_after(&response);
            let details = read_error_details(response, cancellation, &self.api_key).await?;
            return Err(ProviderError::from_http_with_diagnostics(
                status,
                details.message,
                retry_after,
                details.code,
                details.request_id,
            ));
        }
        let body = read_body(response, cancellation).await?;
        let value = serde_json::from_slice::<Value>(&body).map_err(|error| {
            ProviderError::InvalidResponse(format!(
                "image generations returned invalid JSON: {error}"
            ))
        })?;
        parse_response(value).map_err(|error| redact_error(error, &self.api_key))
    }

    pub(crate) fn request_from_provider(
        request: &ProviderRequest,
    ) -> Result<ImageGenerationRequest, ProviderError> {
        let current_message_index = request.messages.iter().rposition(|message| match message {
            ProviderMessage::Text {
                role: MessageRole::User,
                text,
            } => !text.trim().is_empty(),
            ProviderMessage::UserContent { text, .. } => !text.trim().is_empty(),
            _ => false,
        });
        let (current_request, current_images) = if let Some(index) = current_message_index {
            match &request.messages[index] {
                ProviderMessage::Text {
                    role: MessageRole::User,
                    text,
                } => (Some(text.as_str()), None),
                ProviderMessage::UserContent { text, images } => {
                    (Some(text.as_str()), Some(images.as_slice()))
                }
                _ => (None, None),
            }
        } else {
            (None, None)
        };
        let current_request = current_request.ok_or_else(|| {
            ProviderError::InvalidToolArguments(
                "image generation requires a non-empty user prompt".to_string(),
            )
        })?;
        let history = &request.messages[..current_message_index.unwrap_or_default()];
        let prompt = image_prompt_with_context(history, current_request);
        let image = current_images
            .and_then(|images| images.first())
            .map(|value| value.data_url.clone())
            .or_else(|| {
                history.iter().rev().find_map(|message| match message {
                    ProviderMessage::UserContent { images, .. }
                    | ProviderMessage::AssistantImageReference { images, .. } => {
                        images.first().map(|value| value.data_url.clone())
                    }
                    _ => None,
                })
            });
        Ok(ImageGenerationRequest {
            model: request.model.clone(),
            prompt,
            response_format: ImageResponseFormat::B64Json,
            cfg_scale: None,
            steps: None,
            seed: None,
            text_mode: None,
            image,
            mask: None,
        })
    }
}

fn image_prompt_with_context(messages: &[ProviderMessage], current_request: &str) -> String {
    let current_request = if current_request.len() > MAX_PROMPT_BYTES {
        format!(
            "{}…",
            truncate_utf8(current_request, MAX_PROMPT_BYTES - '…'.len_utf8())
        )
    } else {
        current_request.to_string()
    };
    let context = messages
        .iter()
        .filter_map(|message| match message {
            ProviderMessage::Text {
                role: MessageRole::User,
                text,
            }
            | ProviderMessage::UserContent { text, .. }
                if !text.trim().is_empty() =>
            {
                Some(("User: ", text.as_str()))
            }
            ProviderMessage::Text {
                role: MessageRole::Assistant,
                text,
            }
            | ProviderMessage::AssistantToolCalls { text, .. }
                if !text.trim().is_empty() =>
            {
                Some(("Assistant: ", text.as_str()))
            }
            ProviderMessage::AssistantImageReference { text, .. } if !text.trim().is_empty() => {
                Some(("Assistant: ", text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if context.is_empty() {
        return current_request;
    }

    let framing_bytes = IMAGE_CONTEXT_HEADER.len() + CURRENT_IMAGE_REQUEST_HEADER.len();
    let Some(mut remaining) =
        MAX_PROMPT_BYTES.checked_sub(current_request.len().saturating_add(framing_bytes))
    else {
        return current_request;
    };
    let mut selected = Vec::new();
    for (role, text) in context.into_iter().rev() {
        let required_bytes = role.len().saturating_add(text.len()).saturating_add(1);
        if required_bytes <= remaining {
            selected.push(format!("{role}{text}"));
            remaining -= required_bytes;
            continue;
        }

        // Preserve the newest text context. If it is too long, keep its leading
        // portion and stop before older text messages.
        let text_budget = remaining.saturating_sub(role.len().saturating_add(1));
        if selected.is_empty() && text_budget > '…'.len_utf8() {
            let prefix = truncate_utf8(text, text_budget - '…'.len_utf8());
            if !prefix.is_empty() {
                selected.push(format!("{role}{prefix}…"));
            }
        }
        break;
    }
    if selected.is_empty() {
        return current_request;
    }
    selected.reverse();

    let mut prompt = String::with_capacity(MAX_PROMPT_BYTES - remaining);
    prompt.push_str(IMAGE_CONTEXT_HEADER);
    for line in selected {
        prompt.push_str(&line);
        prompt.push('\n');
    }
    prompt.push_str(CURRENT_IMAGE_REQUEST_HEADER);
    prompt.push_str(&current_request);
    prompt
}

fn build_edit_form(
    request: &ImageGenerationRequest,
) -> Result<reqwest::multipart::Form, ProviderError> {
    validate_request(request)?;
    let mut form = reqwest::multipart::Form::new()
        .text("model", request.model.clone())
        .text("prompt", request.prompt.clone())
        .text(
            "response_format",
            request.response_format.as_str().to_string(),
        );
    if let Some(value) = request.cfg_scale {
        form = form.text("cfg_scale", value.to_string());
    }
    if let Some(value) = request.steps {
        form = form.text("steps", value.to_string());
    }
    if let Some(value) = request.seed {
        form = form.text("seed", value.to_string());
    }
    if let Some(value) = request.text_mode {
        form = form.text("text_mode", value.to_string());
    }
    if let Some(value) = &request.image {
        form = form.part("image", image_part(value, "reference")?);
    }
    if let Some(value) = &request.mask {
        form = form.part("mask", image_part(value, "mask")?);
    }
    Ok(form)
}

fn image_part(value: &str, label: &str) -> Result<reqwest::multipart::Part, ProviderError> {
    let (mime_type, encoded) = super::split_image_data_url(value).ok_or_else(|| {
        ProviderError::InvalidToolArguments(format!(
            "image generation {label} input must be a base64 data URL"
        ))
    })?;
    let extension = match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => {
            return Err(ProviderError::InvalidToolArguments(format!(
                "image generation {label} input must be PNG, JPEG, or WebP"
            )));
        }
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| {
            ProviderError::InvalidToolArguments(format!(
                "image generation {label} input contains invalid base64"
            ))
        })?;
    reqwest::multipart::Part::bytes(bytes)
        .file_name(format!("{label}.{extension}"))
        .mime_str(mime_type)
        .map_err(|error| ProviderError::InvalidToolArguments(error.to_string()))
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[async_trait::async_trait]
impl Provider for OpenAiImageGenerationsProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError> {
        let image_request = Self::request_from_provider(&request)?;
        let images = self.generate(image_request, &cancellation).await?;
        Ok(Box::pin(async_stream::stream! {
            for image in images {
                if cancellation.is_cancelled() {
                    yield Err(ProviderError::Cancelled);
                    return;
                }
                yield Ok(ProviderEvent::Image {
                    mime_type: image.mime_type,
                    data: image.data,
                });
            }
            yield Ok(ProviderEvent::Completed);
        }))
    }
}

fn build_payload(request: &ImageGenerationRequest) -> Result<Value, ProviderError> {
    validate_request(request)?;
    let mut payload = json!({
        "model": request.model,
        "prompt": request.prompt,
        "response_format": request.response_format.as_str(),
    });
    if let Some(value) = request.cfg_scale {
        payload["cfg_scale"] = json!(value);
    }
    if let Some(value) = request.steps {
        payload["steps"] = json!(value);
    }
    if let Some(value) = request.seed {
        payload["seed"] = json!(value);
    }
    if let Some(value) = request.text_mode {
        payload["text_mode"] = json!(value);
    }
    if let Some(value) = &request.image {
        payload["image"] = json!(value);
    }
    if let Some(value) = &request.mask {
        payload["mask"] = json!(value);
    }
    Ok(payload)
}

fn validate_request(request: &ImageGenerationRequest) -> Result<(), ProviderError> {
    if request.model.trim().is_empty() || request.model.len() > 200 {
        return Err(ProviderError::InvalidToolArguments(
            "image generation model must contain between 1 and 200 characters".to_string(),
        ));
    }
    if request.prompt.trim().is_empty() || request.prompt.len() > MAX_PROMPT_BYTES {
        return Err(ProviderError::InvalidToolArguments(format!(
            "image generation prompt must contain between 1 and {MAX_PROMPT_BYTES} bytes"
        )));
    }
    if request
        .cfg_scale
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(ProviderError::InvalidToolArguments(
            "image generation cfg_scale must be a finite non-negative number".to_string(),
        ));
    }
    if request
        .steps
        .is_some_and(|value| !(1..=1_000).contains(&value))
    {
        return Err(ProviderError::InvalidToolArguments(
            "image generation steps must be between 1 and 1000".to_string(),
        ));
    }
    for (label, value) in [("image", &request.image), ("mask", &request.mask)] {
        if value
            .as_ref()
            .is_some_and(|value| value.len() > MAX_IMAGE_INPUT_BYTES)
        {
            return Err(ProviderError::InvalidToolArguments(format!(
                "image generation {label} input exceeds {MAX_IMAGE_INPUT_BYTES} bytes"
            )));
        }
    }
    Ok(())
}

fn parse_response(value: Value) -> Result<Vec<GeneratedImage>, ProviderError> {
    let data = value.get("data").and_then(Value::as_array).ok_or_else(|| {
        ProviderError::InvalidResponse(
            "image generations response is missing a data array".to_string(),
        )
    })?;
    if data.is_empty() {
        return Err(ProviderError::InvalidResponse(
            "image generations response contains no images".to_string(),
        ));
    }

    data.iter()
        .map(|item| {
            let object = item.as_object().ok_or_else(|| {
                ProviderError::InvalidResponse(
                    "image generations response contains an invalid image item".to_string(),
                )
            })?;
            let data = object
                .get("b64_json")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    ProviderError::InvalidResponse(
                        "image generations response must contain b64_json results".to_string(),
                    )
                })?;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| {
                    ProviderError::InvalidResponse(
                        "image generations response contains invalid base64".to_string(),
                    )
                })?;
            if decoded.len() > MAX_GENERATED_IMAGE_BYTES {
                return Err(ProviderError::InvalidResponse(format!(
                    "generated image exceeds {MAX_GENERATED_IMAGE_BYTES} bytes"
                )));
            }
            let mime_type = object
                .get("mime_type")
                .or_else(|| object.get("mimeType"))
                .and_then(Value::as_str)
                .unwrap_or("image/png");
            if !matches!(
                mime_type,
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            ) {
                return Err(ProviderError::InvalidResponse(
                    "image generations response contains an unsupported image MIME type"
                        .to_string(),
                ));
            }
            Ok(GeneratedImage {
                mime_type: mime_type.to_string(),
                data: data.to_string(),
            })
        })
        .collect()
}

async fn read_body(
    response: Response,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, ProviderError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = tokio::select! {
        _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
        chunk = stream.next() => chunk,
    } {
        let chunk = chunk.map_err(|error| ProviderError::Request(error.to_string()))?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BODY_BYTES {
            return Err(ProviderError::InvalidResponse(format!(
                "image generations response exceeds {MAX_RESPONSE_BODY_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MessageRole, PROTOCOL_VERSION};
    use crate::providers::{ProviderKind, ProviderMessage, ProviderTransport};
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn stepfun_payload_uses_openai_image_fields_and_keeps_optional_values() {
        let request = ProviderRequest {
            schema_version: PROTOCOL_VERSION,
            model: "step-image-edit-2".into(),
            reasoning_effort: Default::default(),
            messages: vec![ProviderMessage::Text {
                role: MessageRole::User,
                text: "采菊东篱下，悠然见南山".into(),
            }],
            tools: vec![],
        };
        let image_request = ImageGenerationRequest {
            model: request.model.clone(),
            prompt: "采菊东篱下，悠然见南山".into(),
            response_format: ImageResponseFormat::B64Json,
            cfg_scale: Some(1.0),
            steps: Some(8),
            seed: Some(1),
            text_mode: Some(true),
            image: None,
            mask: None,
        };

        let payload = build_payload(&image_request).expect("payload should serialize");

        assert_eq!(
            payload,
            serde_json::json!({
                "model": "step-image-edit-2",
                "prompt": "采菊东篱下，悠然见南山",
                "response_format": "b64_json",
                "cfg_scale": 1.0,
                "steps": 8,
                "seed": 1,
                "text_mode": true,
            })
        );
    }

    #[test]
    fn provider_request_includes_recent_conversation_and_previous_generated_image() {
        let request = ProviderRequest {
            schema_version: PROTOCOL_VERSION,
            model: "gpt-image-1".into(),
            reasoning_effort: Default::default(),
            messages: vec![
                ProviderMessage::Text {
                    role: MessageRole::User,
                    text: "画一位戴红围巾的宇航员，水彩风格".into(),
                },
                ProviderMessage::AssistantImageReference {
                    text: String::new(),
                    images: vec![crate::providers::ProviderImage {
                        name: "generated.png".into(),
                        data_url: "data:image/png;base64,AA==".into(),
                    }],
                },
                ProviderMessage::ToolResult {
                    call_id: "call-1".into(),
                    name: "read_file".into(),
                    success: true,
                    output: "must not be sent to an image model".into(),
                },
                ProviderMessage::Text {
                    role: MessageRole::User,
                    text: "把背景改成月球表面".into(),
                },
            ],
            tools: vec![],
        };

        let image_request = OpenAiImageGenerationsProvider::request_from_provider(&request)
            .expect("image request should be assembled");

        assert!(
            image_request
                .prompt
                .contains("画一位戴红围巾的宇航员，水彩风格")
        );
        assert!(image_request.prompt.contains("Current image request:"));
        assert!(image_request.prompt.contains("把背景改成月球表面"));
        assert!(!image_request.prompt.contains("must not be sent"));
        assert_eq!(
            image_request.image.as_deref(),
            Some("data:image/png;base64,AA==")
        );
    }

    #[test]
    fn image_context_is_bounded_and_keeps_the_newest_text_messages() {
        let messages = vec![
            ProviderMessage::Text {
                role: MessageRole::User,
                text: "oldest context".into(),
            },
            ProviderMessage::Text {
                role: MessageRole::Assistant,
                text: "older reply".into(),
            },
            ProviderMessage::Text {
                role: MessageRole::User,
                text: "x".repeat(MAX_PROMPT_BYTES),
            },
        ];

        let prompt = image_prompt_with_context(&messages, "current request");

        assert!(prompt.len() <= MAX_PROMPT_BYTES);
        assert!(prompt.contains("current request"));
        assert!(!prompt.contains("oldest context"));
    }

    #[test]
    fn image_context_bounds_an_oversized_current_request() {
        let prompt = image_prompt_with_context(&[], &"x".repeat(MAX_PROMPT_BYTES + 1));

        assert!(prompt.len() <= MAX_PROMPT_BYTES);
        assert!(prompt.ends_with('…'));
    }

    #[test]
    fn parses_b64_json_results_and_defaults_mime_type_to_png() {
        let images = parse_response(serde_json::json!({
            "data": [{ "b64_json": "AA==" }, { "b64_json": "AQ==", "mime_type": "image/jpeg" }]
        }))
        .expect("response should parse");

        assert_eq!(
            images,
            vec![
                GeneratedImage {
                    mime_type: "image/png".into(),
                    data: "AA==".into(),
                },
                GeneratedImage {
                    mime_type: "image/jpeg".into(),
                    data: "AQ==".into(),
                },
            ]
        );
    }

    #[test]
    fn rejects_urls_and_invalid_base64_in_the_base64_response_mode() {
        let url_error = parse_response(serde_json::json!({
            "data": [{ "url": "https://example.com/generated.png" }]
        }))
        .expect_err("url-only results cannot enter ProviderEvent::Image");
        assert!(url_error.to_string().contains("b64_json"));

        let base64_error = parse_response(serde_json::json!({
            "data": [{ "b64_json": "not base64" }]
        }))
        .expect_err("invalid base64 must be rejected");
        assert!(base64_error.to_string().contains("base64"));
    }

    #[tokio::test]
    async fn provider_posts_to_images_generations_and_emits_image_events() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let address = listener
            .local_addr()
            .expect("server should have an address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request should connect");
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 4096];
                let read = socket.read(&mut chunk).await.expect("request should read");
                assert!(read > 0, "request ended before headers");
                request.extend_from_slice(&chunk[..read]);
                if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let header_end = index + 4;
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    assert!(headers.contains("POST /step_plan/v1/images/generations HTTP/1.1"));
                    assert!(headers
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-key")));
                    let content_length = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find_map(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("valid length"))
                        })
                        .expect("request should include content length");
                    while request.len() < header_end + content_length {
                        let read = socket
                            .read(&mut chunk)
                            .await
                            .expect("request body should read");
                        assert!(read > 0, "request ended before body");
                        request.extend_from_slice(&chunk[..read]);
                    }
                    let body: Value =
                        serde_json::from_slice(&request[header_end..header_end + content_length])
                            .expect("request body should be JSON");
                    assert_eq!(body["model"], "step-image-edit-2");
                    assert_eq!(body["prompt"], "山居秋暝");
                    assert_eq!(body["response_format"], "b64_json");
                    break;
                }
            }
            let body = r#"{"data":[{"b64_json":"AA==","mime_type":"image/png"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("response should write");
        });

        let config = ProviderConfig {
            schema_version: PROTOCOL_VERSION,
            id: "stepfun-image".into(),
            kind: ProviderKind::OpenAiCompatible,
            transport: ProviderTransport::OpenAiImageGenerations,
            name: "StepFun Images".into(),
            base_url: format!("http://{address}/step_plan/v1"),
            model: "step-image-edit-2".into(),
            models: Vec::new(),
            endpoints: Vec::new(),
            fallback_provider_ids: Vec::new(),
        };
        let provider = OpenAiImageGenerationsProvider::new(config, "test-key".into())
            .expect("image provider should build");
        let request = ProviderRequest {
            schema_version: PROTOCOL_VERSION,
            model: "step-image-edit-2".into(),
            reasoning_effort: Default::default(),
            messages: vec![ProviderMessage::Text {
                role: MessageRole::User,
                text: "山居秋暝".into(),
            }],
            tools: Vec::new(),
        };
        let mut stream = provider
            .stream(request, CancellationToken::new())
            .await
            .expect("provider stream should start");
        let first = stream.next().await.expect("image event should arrive");
        assert!(matches!(
            first,
            Ok(ProviderEvent::Image { mime_type, data })
                if mime_type == "image/png" && data == "AA=="
        ));
        assert!(matches!(
            stream.next().await.expect("completion should arrive"),
            Ok(ProviderEvent::Completed)
        ));
        assert!(stream.next().await.is_none());
        server.await.expect("test server should finish");
    }

    #[tokio::test]
    async fn provider_sends_conversation_context_and_prior_image_to_edits_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let address = listener
            .local_addr()
            .expect("server should have an address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request should connect");
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 4096];
                let read = socket.read(&mut chunk).await.expect("request should read");
                assert!(read > 0, "request ended before headers");
                request.extend_from_slice(&chunk[..read]);
                if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let header_end = index + 4;
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    assert!(headers.contains("POST /v1/images/edits HTTP/1.1"));
                    assert!(headers.lines().any(|line| {
                        line.eq_ignore_ascii_case("authorization: Bearer test-key")
                    }));
                    assert!(
                        headers.lines().any(|line| {
                            line.to_ascii_lowercase().contains("multipart/form-data")
                        })
                    );
                    let content_length = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find_map(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("valid length"))
                        })
                        .expect("multipart request should include content length");
                    while request.len() < header_end + content_length {
                        let read = socket
                            .read(&mut chunk)
                            .await
                            .expect("request body should read");
                        assert!(read > 0, "request ended before body");
                        request.extend_from_slice(&chunk[..read]);
                    }
                    let body =
                        String::from_utf8_lossy(&request[header_end..header_end + content_length]);
                    assert!(body.contains("name=\"image\""));
                    assert!(!body.contains("name=\"image[]\""));
                    assert!(body.contains("name=\"model\""));
                    assert!(body.contains("gpt-image-1"));
                    assert!(body.contains("name=\"prompt\""));
                    assert!(body.contains("戴红围巾的宇航员"));
                    assert!(body.contains("Current image request:"));
                    assert!(body.contains("把背景改成月球表面"));
                    assert!(!body.contains("private tool output"));
                    break;
                }
            }
            let body = r#"{"data":[{"b64_json":"AA==","mime_type":"image/png"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("response should write");
        });

        let config = ProviderConfig {
            schema_version: PROTOCOL_VERSION,
            id: "openai-image".into(),
            kind: ProviderKind::OpenAiCompatible,
            transport: ProviderTransport::OpenAiImageGenerations,
            name: "OpenAI Images".into(),
            base_url: format!("http://{address}/v1"),
            model: "gpt-image-1".into(),
            models: Vec::new(),
            endpoints: Vec::new(),
            fallback_provider_ids: Vec::new(),
        };
        let provider = OpenAiImageGenerationsProvider::new(config, "test-key".into())
            .expect("image provider should build");
        let request = ProviderRequest {
            schema_version: PROTOCOL_VERSION,
            model: "gpt-image-1".into(),
            reasoning_effort: Default::default(),
            messages: vec![
                ProviderMessage::Text {
                    role: MessageRole::User,
                    text: "画一位戴红围巾的宇航员，水彩风格".into(),
                },
                ProviderMessage::AssistantImageReference {
                    text: String::new(),
                    images: vec![crate::providers::ProviderImage {
                        name: "generated.png".into(),
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
                    text: "把背景改成月球表面".into(),
                },
            ],
            tools: Vec::new(),
        };
        let mut stream = provider
            .stream(request, CancellationToken::new())
            .await
            .expect("provider should return a stream");
        assert!(matches!(
            stream
                .next()
                .await
                .expect("image event should exist")
                .unwrap(),
            ProviderEvent::Image { .. }
        ));
        assert!(matches!(
            stream
                .next()
                .await
                .expect("completion event should exist")
                .unwrap(),
            ProviderEvent::Completed
        ));
        server.await.expect("server should finish");
    }
}
