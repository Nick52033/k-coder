use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    pub event_id: String,
    pub thread_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage I/O failed: {0}")]
    Io(String),
    #[error("stored data is invalid: {0}")]
    InvalidData(String),
}

#[async_trait]
pub trait ThreadRepository: Send + Sync {
    async fn append(&self, event: StoredEvent) -> Result<(), StorageError>;
    async fn load(&self, thread_id: &str) -> Result<Vec<StoredEvent>, StorageError>;
}
