mod anthropic;
mod common;
mod config;
mod credentials;
mod gemini;
mod openai;
mod responses;
mod sse;

#[cfg(test)]
pub mod testing;

use std::pin::Pin;

use async_trait::async_trait;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::protocol::{MessageRole, TokenUsage, ToolCall, ToolDefinition};

pub use anthropic::AnthropicMessagesProvider;
pub use config::{
    ProviderConfig, ProviderConfigError, ProviderConfigStore, ProviderConfigView, ProviderKind,
    ProviderTransport, SaveProviderConfigRequest,
};
pub use credentials::{CredentialError, CredentialStore, OsCredentialStore};
pub use gemini::GoogleGeminiProvider;
pub use openai::OpenAiChatCompletionsProvider;
pub use responses::OpenAiResponsesProvider;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRequest {
    pub schema_version: u32,
    pub model: String,
    pub messages: Vec<ProviderMessage>,
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ProviderMessage {
    Text {
        role: MessageRole,
        text: String,
    },
    AssistantToolCalls {
        calls: Vec<ToolCall>,
    },
    ToolResult {
        call_id: String,
        name: String,
        success: bool,
        output: String,
    },
    ProviderContext {
        provider: String,
        item: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    TextDelta {
        delta: String,
    },
    ToolCall {
        call: ToolCall,
    },
    ProviderContext {
        provider: String,
        item: serde_json::Value,
    },
    Usage {
        usage: TokenUsage,
    },
    Completed,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("provider request was cancelled")]
    Cancelled,
    #[error("provider request failed: {0}")]
    Request(String),
    #[error("provider returned HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("provider response was invalid: {0}")]
    InvalidResponse(String),
    #[error("provider stream ended before completion")]
    Interrupted,
}

pub type ProviderStream =
    Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send + 'static>>;

#[async_trait]
pub trait Provider: Send + Sync {
    async fn stream(
        &self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream, ProviderError>;
}
