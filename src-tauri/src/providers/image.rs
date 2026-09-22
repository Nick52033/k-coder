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
        payload: &Value,
        cancellation: &CancellationToken,
    ) -> Result<Response, ProviderError> {
        tokio::select! {
            _ = cancellation.cancelled() => Err(ProviderError::Cancelled),
            response = self.client
                .post(endpoint)
                .bearer_auth(&self.api_key)
                .header(reqwest::header::ACCEPT, "application/json")
                .json(payload)
                .send() => response.map_err(|error| ProviderError::Request(error.to_string())),
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
        let payload = build_payload(&request)?;
        let endpoint = self
            .config
            .images_generations_url()
            .map_err(|error| ProviderError::Request(error.to_string()))?;
        let response = self.send_request(endpoint, &payload, cancellation).await?;
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

    fn request_from_provider(
        request: &ProviderRequest,
    ) -> Result<ImageGenerationRequest, ProviderError> {
        let mut prompt = None;
        let mut image = None;
        for message in request.messages.iter().rev() {
            match message {
                ProviderMessage::Text {
                    role: MessageRole::User,
                    text,
                } => {
                    prompt = Some(text.clone());
                    break;
                }
                ProviderMessage::UserContent { text, images } => {
                    prompt = Some(text.clone());
                    image = images.first().map(|value| value.data_url.clone());
                    break;
                }
                _ => {}
            }
        }
        let prompt = prompt
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ProviderError::InvalidToolArguments(
                    "image generation requires a non-empty user prompt".to_string(),
                )
            })?;
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
}
