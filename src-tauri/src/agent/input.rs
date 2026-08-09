use uuid::Uuid;

use crate::protocol::{ChatMessage, ContentBlock, ImageAttachment, MessageRole, PROTOCOL_VERSION};
use crate::providers::{ProviderImage, ProviderMessage};
use crate::storage::now_ms;

use super::AgentRuntimeError;

const MAX_INPUT_BYTES: usize = 100_000;
const MAX_IMAGE_COUNT: usize = 4;
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_OCR_TEXT_BYTES: usize = 16 * 1024;
const MAX_TOTAL_IMAGE_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn user_message(
    text: String,
    attachments: Vec<ImageAttachment>,
    supports_vision: bool,
) -> Result<ChatMessage, AgentRuntimeError> {
    if attachments.len() > MAX_IMAGE_COUNT {
        return Err(AgentRuntimeError::InvalidInput(format!(
            "at most {MAX_IMAGE_COUNT} images may be attached"
        )));
    }
    let mut total = 0usize;
    let mut content = if text.is_empty() {
        vec![ContentBlock::Context {
            text: if supports_vision {
                "请分析用户提供的图片。".into()
            } else {
                "请根据本地图片文字识别结果回答。".into()
            },
        }]
    } else {
        vec![ContentBlock::Text { text }]
    };
    for attachment in attachments {
        let name: String = attachment.name.chars().take(255).collect();
        let (_, encoded) = parse_image_data_url(&attachment.data_url)?;
        let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .map_err(|_| {
            AgentRuntimeError::InvalidInput("image data is not valid base64".into())
        })?;
        if decoded.len() > MAX_IMAGE_BYTES {
            return Err(AgentRuntimeError::InvalidInput(
                "an attached image exceeds the 4 MiB limit".into(),
            ));
        }
        total = total.saturating_add(decoded.len());
        if total > MAX_TOTAL_IMAGE_BYTES {
            return Err(AgentRuntimeError::InvalidInput(
                "attached images exceed the 8 MiB total limit".into(),
            ));
        }
        let ocr_text = attachment
            .ocr_text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        if !supports_vision && ocr_text.is_none() {
            return Err(AgentRuntimeError::InvalidInput(format!(
                "the selected model does not support images and local OCR produced no text for {name}"
            )));
        }
        if let Some(ocr_text) = ocr_text {
            let ocr_text = truncate_utf8(ocr_text, MAX_OCR_TEXT_BYTES);
            content.push(ContentBlock::Context {
                text: format!("\n\n[图片文字识别: {name}]\n{ocr_text}"),
            });
        }
        content.push(ContentBlock::Image {
            name,
            data_url: attachment.data_url,
        });
    }
    Ok(ChatMessage {
        schema_version: PROTOCOL_VERSION,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content,
        created_at_ms: now_ms(),
    })
}

pub(crate) fn build_user_message(
    input: &str,
    attachments: Vec<ImageAttachment>,
    supports_vision: bool,
) -> Result<ChatMessage, AgentRuntimeError> {
    let input = validate_input(input, !attachments.is_empty())?;
    user_message(input, attachments, supports_vision)
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

fn parse_image_data_url(value: &str) -> Result<(&str, &str), AgentRuntimeError> {
    let (metadata, encoded) = value.split_once(',').ok_or_else(|| {
        AgentRuntimeError::InvalidInput("image attachment must be a data URL".into())
    })?;
    let media_type = metadata
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .ok_or_else(|| AgentRuntimeError::InvalidInput("image data URL must use base64".into()))?;
    if !matches!(
        media_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) {
        return Err(AgentRuntimeError::InvalidInput(
            "image type must be PNG, JPEG, GIF, or WebP".into(),
        ));
    }
    Ok((media_type, encoded))
}

pub(super) fn chat_to_provider(
    message: ChatMessage,
    supports_vision: bool,
) -> Option<ProviderMessage> {
    let text = message.text();
    let images = message
        .content
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Image { name, data_url } if supports_vision => {
                Some(ProviderImage { name, data_url })
            }
            ContentBlock::Image { .. } => None,
            ContentBlock::Text { .. } | ContentBlock::Context { .. } => None,
        })
        .collect::<Vec<_>>();
    if message.role == MessageRole::User && !images.is_empty() {
        Some(ProviderMessage::UserContent { text, images })
    } else {
        Some(ProviderMessage::Text {
            role: message.role,
            text,
        })
    }
}

fn validate_input(input: &str, allow_empty: bool) -> Result<String, AgentRuntimeError> {
    let input = input.trim();
    if input.is_empty() && !allow_empty {
        return Err(AgentRuntimeError::InvalidInput(
            "input must not be empty".to_string(),
        ));
    }
    if input.len() > MAX_INPUT_BYTES {
        return Err(AgentRuntimeError::InvalidInput(format!(
            "input exceeds the {MAX_INPUT_BYTES} byte limit"
        )));
    }
    Ok(input.to_string())
}
