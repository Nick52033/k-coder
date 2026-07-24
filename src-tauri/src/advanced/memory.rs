use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::store::{append_json_line, read_json_lines};
use crate::protocol::{PROTOCOL_VERSION, ToolDefinition, ToolResult};
use crate::storage::now_ms;
use crate::tools::{ToolContext, ToolError, ToolHandler};

const MAX_MEMORY_CONTENT: usize = 4_000;
const MAX_ACTIVE_MEMORIES: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemorySettings {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    pub schema_version: u32,
    pub id: String,
    pub content: String,
    pub source: String,
    pub expires_at_ms: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub deleted: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryUpsertRequest {
    #[serde(default)]
    pub id: Option<String>,
    pub content: String,
    pub source: String,
    pub retention_days: u32,
}

#[derive(Clone)]
pub struct MemoryStore {
    log_path: PathBuf,
    settings_path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl MemoryStore {
    pub fn new(data_root: &std::path::Path) -> Result<Self, String> {
        Ok(Self {
            log_path: data_root.join("advanced/memories.jsonl"),
            settings_path: data_root.join("advanced/memory-settings.json"),
            lock: Arc::new(Mutex::new(())),
        })
    }
    pub fn settings(&self) -> Result<MemorySettings, String> {
        match std::fs::read(&self.settings_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(MemorySettings { enabled: false })
            }
            Err(error) => Err(error.to_string()),
        }
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<MemorySettings, String> {
        let _guard = self.lock.lock().map_err(|_| "memory lock poisoned")?;
        let settings = MemorySettings { enabled };
        let parent = self
            .settings_path
            .parent()
            .ok_or("memory settings path has no parent")?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        std::fs::write(
            &self.settings_path,
            serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        Ok(settings)
    }
    fn latest_unlocked(&self) -> Result<Vec<MemoryView>, String> {
        let mut latest = std::collections::HashMap::<String, MemoryView>::new();
        for memory in read_json_lines::<MemoryView>(&self.log_path)? {
            if latest
                .get(&memory.id)
                .is_none_or(|current| current.revision < memory.revision)
            {
                latest.insert(memory.id.clone(), memory);
            }
        }
        Ok(latest.into_values().collect())
    }
    pub fn list(&self) -> Result<Vec<MemoryView>, String> {
        let _guard = self.lock.lock().map_err(|_| "memory lock poisoned")?;
        let now = now_ms();
        let mut memories = self
            .latest_unlocked()?
            .into_iter()
            .filter(|memory| !memory.deleted && memory.expires_at_ms > now)
            .collect::<Vec<_>>();
        memories.sort_by_key(|memory| std::cmp::Reverse(memory.updated_at_ms));
        Ok(memories)
    }
    pub fn upsert(&self, request: MemoryUpsertRequest) -> Result<MemoryView, String> {
        if !self.settings()?.enabled {
            return Err("memory is disabled; enable it explicitly in settings".into());
        }
        let content = request.content.trim();
        let source = request.source.trim();
        if content.is_empty() || content.len() > MAX_MEMORY_CONTENT {
            return Err(format!(
                "memory content must contain 1 to {MAX_MEMORY_CONTENT} characters"
            ));
        }
        if source.is_empty() || source.len() > 240 {
            return Err("memory source must contain 1 to 240 characters".into());
        }
        if !(1..=365).contains(&request.retention_days) {
            return Err("memory retention must contain 1 to 365 days".into());
        }
        let _guard = self.lock.lock().map_err(|_| "memory lock poisoned")?;
        let latest = self.latest_unlocked()?;
        if request.id.is_none()
            && latest
                .iter()
                .filter(|memory| !memory.deleted && memory.expires_at_ms > now_ms())
                .count()
                >= MAX_ACTIVE_MEMORIES
        {
            return Err("memory reached its 100 item limit".into());
        }
        let previous = request
            .id
            .as_ref()
            .and_then(|id| latest.iter().find(|memory| &memory.id == id));
        if request.id.is_some() && previous.is_none() {
            return Err("memory was not found".into());
        }
        let now = now_ms();
        let memory = MemoryView {
            schema_version: PROTOCOL_VERSION,
            id: request.id.unwrap_or_else(|| Uuid::new_v4().to_string()),
            content: content.into(),
            source: source.into(),
            expires_at_ms: now.saturating_add(request.retention_days as u64 * 86_400_000),
            created_at_ms: previous.map_or(now, |value| value.created_at_ms),
            updated_at_ms: now,
            deleted: false,
            revision: previous.map_or(1, |value| value.revision + 1),
        };
        append_json_line(&self.log_path, &memory)?;
        Ok(memory)
    }
    pub fn delete(&self, id: &str) -> Result<MemoryView, String> {
        let _guard = self.lock.lock().map_err(|_| "memory lock poisoned")?;
        let mut memory = self
            .latest_unlocked()?
            .into_iter()
            .find(|memory| memory.id == id)
            .ok_or("memory was not found")?;
        memory.deleted = true;
        memory.updated_at_ms = now_ms();
        memory.revision += 1;
        append_json_line(&self.log_path, &memory)?;
        Ok(memory)
    }
    pub fn context(&self) -> Result<String, String> {
        if !self.settings()?.enabled {
            return Ok(String::new());
        }
        let mut output = String::from("[User-enabled memory with provenance]\n");
        for memory in self.list()?.into_iter().take(20) {
            let line = format!(
                "- source={} expires={} content={}\n",
                memory.source,
                memory.expires_at_ms,
                memory.content.replace('\n', " ")
            );
            if output.len() + line.len() > 16_000 {
                break;
            }
            output.push_str(&line);
        }
        Ok(output)
    }
}

pub struct RecallMemoryTool {
    store: MemoryStore,
}
impl RecallMemoryTool {
    pub fn new(store: MemoryStore) -> Self {
        Self { store }
    }
}
#[async_trait]
impl ToolHandler for RecallMemoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "recall_memory".into(),
            description: "Read user-enabled, unexpired memories with their provenance.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        }
    }
    async fn execute(
        &self,
        _context: &ToolContext,
        _arguments: Value,
        _cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if !self.store.settings().map_err(ToolError::Execution)?.enabled {
            return Err(ToolError::Execution(
                "memory is disabled; enable it explicitly in settings".into(),
            ));
        }
        let memories = self.store.list().map_err(ToolError::Execution)?;
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string(&memories)
                .map_err(|error| ToolError::Execution(error.to_string()))?,
            metadata: json!({"count":memories.len()}),
        })
    }
}

pub struct RememberTool {
    store: MemoryStore,
}
impl RememberTool {
    pub fn new(store: MemoryStore) -> Self {
        Self { store }
    }
}
#[async_trait]
impl ToolHandler for RememberTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "remember".into(),
            description: "Save a user-enabled memory with explicit provenance and retention."
                .into(),
            input_schema: json!({"type":"object","properties":{"content":{"type":"string","minLength":1,"maxLength":MAX_MEMORY_CONTENT},"source":{"type":"string","minLength":1,"maxLength":240},"retentionDays":{"type":"integer","minimum":1,"maximum":365}},"required":["content","source","retentionDays"],"additionalProperties":false}),
        }
    }
    async fn execute(
        &self,
        _context: &ToolContext,
        arguments: Value,
        _cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let request: MemoryUpsertRequest = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let memory = self.store.upsert(request).map_err(ToolError::Execution)?;
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string(&memory)
                .map_err(|error| ToolError::Execution(error.to_string()))?,
            metadata: json!({"memoryId":memory.id}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn memory_requires_opt_in_and_preserves_provenance_and_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path()).unwrap();
        let request = MemoryUpsertRequest {
            id: None,
            content: "Use pnpm".into(),
            source: "user instruction".into(),
            retention_days: 30,
        };
        assert!(store.upsert(request.clone()).is_err());
        let context = ToolContext {
            thread_id: "thread".into(),
            turn_id: "turn".into(),
            call_id: "call".into(),
            workspace_root: dir.path().to_path_buf(),
            approval: None,
        };
        let error = RecallMemoryTool::new(store.clone())
            .execute(&context, json!({}), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, ToolError::Execution(_)));
        store.set_enabled(true).unwrap();
        let memory = store.upsert(request).unwrap();
        assert_eq!(store.list().unwrap()[0].source, "user instruction");
        store.delete(&memory.id).unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
