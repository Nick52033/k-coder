use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    pub thread_id: String,
    pub call_id: String,
    pub workspace_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),
    #[error("tool execution denied: {0}")]
    Denied(String),
    #[error("tool execution failed: {0}")]
    Execution(String),
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn execute(
        &self,
        context: &ToolContext,
        arguments: serde_json::Value,
    ) -> Result<ToolResult, ToolError>;
}
