use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub model: String,
    pub messages: Vec<serde_json::Value>,
    pub tools: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    TextDelta {
        delta: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Completed,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider request failed: {0}")]
    Request(String),
    #[error("provider response was invalid: {0}")]
    InvalidResponse(String),
}

#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, request: ProviderRequest)
    -> Result<Vec<ProviderEvent>, ProviderError>;
}
