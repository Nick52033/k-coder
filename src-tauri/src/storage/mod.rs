use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::protocol::{
    ApprovalRequest, ApprovalResolution, ApprovalSnapshot, ChangeSet, ChatMessage, MessageRole,
    PROTOCOL_VERSION, TokenUsage, ToolCall, ToolResult, TurnState,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub created_at_ms: u64,
    #[serde(flatten)]
    pub kind: StoredEventKind,
}

impl StoredEvent {
    pub fn new(
        thread_id: impl Into<String>,
        turn_id: Option<String>,
        kind: StoredEventKind,
    ) -> Self {
        Self {
            schema_version: PROTOCOL_VERSION,
            event_id: Uuid::new_v4().to_string(),
            thread_id: thread_id.into(),
            turn_id,
            created_at_ms: now_ms(),
            kind,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum StoredEventKind {
    ThreadCreated {
        title: String,
    },
    UserMessage {
        message: ChatMessage,
    },
    TurnStarted,
    AssistantMessage {
        message: ChatMessage,
    },
    AssistantToolCalls {
        calls: Vec<ToolCall>,
    },
    ToolResult {
        call_id: String,
        name: String,
        result: ToolResult,
    },
    ProviderContext {
        provider: String,
        item: serde_json::Value,
    },
    ApprovalRequested {
        request: ApprovalRequest,
    },
    ApprovalResolved {
        request_id: String,
        resolution: ApprovalResolution,
    },
    ChangeApplied {
        change_set: ChangeSet,
    },
    ChangeUndone {
        change_id: String,
    },
    TurnCompleted {
        usage: Option<TokenUsage>,
    },
    TurnFailed {
        message: String,
    },
    TurnCancelled,
    ThreadArchived,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub schema_version: u32,
    pub id: String,
    pub title: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnSnapshot {
    pub turn_id: String,
    pub state: TurnState,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetail {
    pub schema_version: u32,
    pub summary: ThreadSummary,
    pub messages: Vec<ChatMessage>,
    pub last_turn: Option<TurnSnapshot>,
    pub tool_activities: Vec<ToolActivitySnapshot>,
    pub approvals: Vec<ApprovalSnapshot>,
    pub changes: Vec<ChangeSet>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolActivityState {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolActivitySnapshot {
    pub turn_id: String,
    pub call: ToolCall,
    pub state: ToolActivityState,
    pub result: Option<ToolResult>,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage I/O failed: {0}")]
    Io(String),
    #[error("stored data is invalid: {0}")]
    InvalidData(String),
    #[error("thread was not found: {0}")]
    NotFound(String),
}

#[async_trait]
pub trait ThreadRepository: Send + Sync {
    async fn append(&self, event: StoredEvent) -> Result<(), StorageError>;
    async fn load(&self, thread_id: &str) -> Result<Vec<StoredEvent>, StorageError>;
}

#[derive(Debug, Clone)]
pub struct JsonlThreadRepository {
    sessions_dir: PathBuf,
    append_lock: Arc<Mutex<()>>,
}

impl JsonlThreadRepository {
    pub fn new(data_root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let sessions_dir = data_root.as_ref().join("sessions");
        fs::create_dir_all(&sessions_dir).map_err(|error| StorageError::Io(error.to_string()))?;
        Ok(Self {
            sessions_dir,
            append_lock: Arc::new(Mutex::new(())),
        })
    }

    pub async fn create_thread(&self) -> Result<ThreadSummary, StorageError> {
        let thread_id = Uuid::new_v4().to_string();
        self.append(StoredEvent::new(
            &thread_id,
            None,
            StoredEventKind::ThreadCreated {
                title: "新会话".to_string(),
            },
        ))
        .await?;
        Ok(self.read_thread(&thread_id).await?.summary)
    }

    pub async fn list_threads(&self) -> Result<Vec<ThreadSummary>, StorageError> {
        let sessions_dir = self.sessions_dir.clone();
        tokio::task::spawn_blocking(move || {
            let mut summaries = Vec::new();
            for entry in
                fs::read_dir(&sessions_dir).map_err(|error| StorageError::Io(error.to_string()))?
            {
                let entry = entry.map_err(|error| StorageError::Io(error.to_string()))?;
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                    continue;
                }
                let Some(thread_id) = path.file_stem().and_then(|value| value.to_str()) else {
                    continue;
                };
                if Uuid::parse_str(thread_id).is_err() {
                    continue;
                }
                let events = load_path(&path)?;
                let detail = project_thread(thread_id, &events)?;
                if !detail.summary.archived {
                    summaries.push(detail.summary);
                }
            }
            summaries.sort_by_key(|summary| std::cmp::Reverse(summary.updated_at_ms));
            Ok(summaries)
        })
        .await
        .map_err(|error| StorageError::Io(error.to_string()))?
    }

    pub async fn read_thread(&self, thread_id: &str) -> Result<ThreadDetail, StorageError> {
        let events = self.load(thread_id).await?;
        project_thread(thread_id, &events)
    }

    pub async fn archive_thread(&self, thread_id: &str) -> Result<(), StorageError> {
        let detail = self.read_thread(thread_id).await?;
        if !detail.summary.archived {
            self.append(StoredEvent::new(
                thread_id,
                None,
                StoredEventKind::ThreadArchived,
            ))
            .await?;
        }
        Ok(())
    }

    fn session_path(&self, thread_id: &str) -> Result<PathBuf, StorageError> {
        let id = Uuid::parse_str(thread_id)
            .map_err(|_| StorageError::InvalidData("thread ID must be a UUID".to_string()))?;
        Ok(self.sessions_dir.join(format!("{id}.jsonl")))
    }
}

#[async_trait]
impl ThreadRepository for JsonlThreadRepository {
    async fn append(&self, event: StoredEvent) -> Result<(), StorageError> {
        if event.schema_version != PROTOCOL_VERSION {
            return Err(StorageError::InvalidData(format!(
                "unsupported event schema version {}",
                event.schema_version
            )));
        }
        let path = self.session_path(&event.thread_id)?;
        let line = serde_json::to_vec(&event)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let _guard = self.append_lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|error| StorageError::Io(error.to_string()))?;
            file.write_all(&line)
                .and_then(|_| file.write_all(b"\n"))
                .and_then(|_| file.sync_data())
                .map_err(|error| StorageError::Io(error.to_string()))
        })
        .await
        .map_err(|error| StorageError::Io(error.to_string()))?
    }

    async fn load(&self, thread_id: &str) -> Result<Vec<StoredEvent>, StorageError> {
        let path = self.session_path(thread_id)?;
        let thread_id = thread_id.to_string();
        tokio::task::spawn_blocking(move || {
            if !path.exists() {
                return Err(StorageError::NotFound(thread_id));
            }
            load_path(&path)
        })
        .await
        .map_err(|error| StorageError::Io(error.to_string()))?
    }
}

fn load_path(path: &Path) -> Result<Vec<StoredEvent>, StorageError> {
    let bytes = fs::read(path).map_err(|error| StorageError::Io(error.to_string()))?;
    let ends_with_newline = bytes.ends_with(b"\n");
    let nonempty = bytes
        .split(|byte| *byte == b'\n')
        .enumerate()
        .filter(|(_, line)| !line.iter().all(u8::is_ascii_whitespace))
        .collect::<Vec<_>>();
    let last_nonempty_index = nonempty.last().map(|(index, _)| *index);
    let mut events = Vec::with_capacity(nonempty.len());

    for (index, line) in nonempty {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        match serde_json::from_slice::<StoredEvent>(line) {
            Ok(event) => {
                if event.schema_version != PROTOCOL_VERSION {
                    return Err(StorageError::InvalidData(format!(
                        "unsupported event schema version {}",
                        event.schema_version
                    )));
                }
                events.push(event);
            }
            Err(_) if Some(index) == last_nonempty_index && !ends_with_newline => break,
            Err(error) => return Err(StorageError::InvalidData(error.to_string())),
        }
    }
    Ok(events)
}

fn project_thread(thread_id: &str, events: &[StoredEvent]) -> Result<ThreadDetail, StorageError> {
    let created = events
        .iter()
        .find_map(|event| match &event.kind {
            StoredEventKind::ThreadCreated { title } => Some((title.clone(), event.created_at_ms)),
            _ => None,
        })
        .ok_or_else(|| StorageError::InvalidData("thread_created event is missing".to_string()))?;

    let mut title = created.0;
    let mut messages = Vec::new();
    let mut archived = false;
    let mut last_turn = None;
    let mut tool_activities: Vec<ToolActivitySnapshot> = Vec::new();
    let mut approvals: Vec<ApprovalSnapshot> = Vec::new();
    let mut changes: Vec<ChangeSet> = Vec::new();
    let mut updated_at_ms = created.1;

    for event in events {
        if event.thread_id != thread_id {
            return Err(StorageError::InvalidData(
                "event thread ID does not match its session file".to_string(),
            ));
        }
        updated_at_ms = updated_at_ms.max(event.created_at_ms);
        match &event.kind {
            StoredEventKind::UserMessage { message } => {
                if title == "新会话" && message.role == MessageRole::User {
                    title = title_from_message(&message.text());
                }
                messages.push(message.clone());
            }
            StoredEventKind::AssistantMessage { message } => messages.push(message.clone()),
            StoredEventKind::AssistantToolCalls { calls } => {
                if let Some(turn_id) = &event.turn_id {
                    tool_activities.extend(calls.iter().cloned().map(|call| {
                        ToolActivitySnapshot {
                            turn_id: turn_id.clone(),
                            call,
                            state: ToolActivityState::Running,
                            result: None,
                        }
                    }));
                }
            }
            StoredEventKind::ToolResult {
                call_id, result, ..
            } => {
                if let Some(activity) = tool_activities
                    .iter_mut()
                    .rev()
                    .find(|activity| activity.call.id == *call_id)
                {
                    activity.state = if result.success {
                        ToolActivityState::Completed
                    } else {
                        ToolActivityState::Failed
                    };
                    activity.result = Some(result.clone());
                }
            }
            StoredEventKind::ProviderContext { .. } => {}
            StoredEventKind::ApprovalRequested { request } => {
                approvals.push(ApprovalSnapshot {
                    request: request.clone(),
                    resolution: None,
                });
                update_turn(&mut last_turn, event, TurnState::AwaitingApproval, None);
            }
            StoredEventKind::ApprovalResolved {
                request_id,
                resolution,
            } => {
                if let Some(approval) = approvals
                    .iter_mut()
                    .rev()
                    .find(|approval| approval.request.id == *request_id)
                {
                    approval.resolution = Some(resolution.clone());
                }
                update_turn(&mut last_turn, event, TurnState::Streaming, None);
            }
            StoredEventKind::ChangeApplied { change_set } => changes.push(change_set.clone()),
            StoredEventKind::ChangeUndone { change_id } => {
                if let Some(change) = changes
                    .iter_mut()
                    .rev()
                    .find(|change| change.id == *change_id)
                {
                    change.undone = true;
                }
            }
            StoredEventKind::TurnStarted => {
                if let Some(turn_id) = &event.turn_id {
                    last_turn = Some(TurnSnapshot {
                        turn_id: turn_id.clone(),
                        state: TurnState::Streaming,
                        error: None,
                    });
                }
            }
            StoredEventKind::TurnCompleted { .. } => {
                update_turn(&mut last_turn, event, TurnState::Completed, None)
            }
            StoredEventKind::TurnFailed { message } => update_turn(
                &mut last_turn,
                event,
                TurnState::Failed,
                Some(message.clone()),
            ),
            StoredEventKind::TurnCancelled => {
                update_turn(&mut last_turn, event, TurnState::Cancelled, None)
            }
            StoredEventKind::ThreadArchived => archived = true,
            StoredEventKind::ThreadCreated { .. } => {}
        }
    }

    Ok(ThreadDetail {
        schema_version: PROTOCOL_VERSION,
        summary: ThreadSummary {
            schema_version: PROTOCOL_VERSION,
            id: thread_id.to_string(),
            title,
            created_at_ms: created.1,
            updated_at_ms,
            archived,
        },
        messages,
        last_turn,
        tool_activities,
        approvals,
        changes,
    })
}

fn update_turn(
    last_turn: &mut Option<TurnSnapshot>,
    event: &StoredEvent,
    state: TurnState,
    error: Option<String>,
) {
    if let Some(turn_id) = &event.turn_id {
        *last_turn = Some(TurnSnapshot {
            turn_id: turn_id.clone(),
            state,
            error,
        });
    }
}

fn title_from_message(message: &str) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let title = chars.by_ref().take(28).collect::<String>();
    if chars.next().is_some() {
        format!("{title}...")
    } else if title.is_empty() {
        "新会话".to_string()
    } else {
        title
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        ApprovalAction, ChangeFileSnapshot, ContentBlock, ExpectedFileHash, FileOperation,
        MessageRole, PatchFilePreview, PatchPreview, ToolRisk,
    };

    fn message(role: MessageRole, text: &str) -> ChatMessage {
        ChatMessage {
            schema_version: PROTOCOL_VERSION,
            id: Uuid::new_v4().to_string(),
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            created_at_ms: now_ms(),
        }
    }

    #[tokio::test]
    async fn creates_projects_and_archives_a_thread() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let repository =
            JsonlThreadRepository::new(directory.path()).expect("repository should be created");
        let thread = repository
            .create_thread()
            .await
            .expect("thread should be created");
        repository
            .append(StoredEvent::new(
                &thread.id,
                None,
                StoredEventKind::UserMessage {
                    message: message(MessageRole::User, "Explain this repository"),
                },
            ))
            .await
            .expect("message should append");

        let detail = repository
            .read_thread(&thread.id)
            .await
            .expect("thread should load");
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.summary.title, "Explain this repository");
        assert_eq!(repository.list_threads().await.unwrap().len(), 1);

        repository
            .archive_thread(&thread.id)
            .await
            .expect("thread should archive");
        assert!(repository.list_threads().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn ignores_only_a_truncated_final_jsonl_record() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let repository =
            JsonlThreadRepository::new(directory.path()).expect("repository should be created");
        let thread = repository
            .create_thread()
            .await
            .expect("thread should be created");
        let path = directory
            .path()
            .join("sessions")
            .join(format!("{}.jsonl", thread.id));
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(b"{\"schemaVersion\":1,\"eventId\":")
            .unwrap();
        file.sync_data().unwrap();

        let events = repository
            .load(&thread.id)
            .await
            .expect("valid prefix should recover");

        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn rejects_a_malformed_complete_record() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let repository =
            JsonlThreadRepository::new(directory.path()).expect("repository should be created");
        let thread = repository
            .create_thread()
            .await
            .expect("thread should be created");
        let path = directory
            .path()
            .join("sessions")
            .join(format!("{}.jsonl", thread.id));
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(b"not-json\n").unwrap();
        file.sync_data().unwrap();

        assert!(matches!(
            repository.load(&thread.id).await,
            Err(StorageError::InvalidData(_))
        ));
    }

    #[tokio::test]
    async fn rebuilds_approval_and_change_history_from_events() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let turn_id = "turn-review".to_string();
        let preview_file = PatchFilePreview {
            path: "src/main.rs".to_string(),
            destination_path: None,
            operation: FileOperation::Modify,
            before_hash: Some("before-hash".to_string()),
            after_hash: Some("after-hash".to_string()),
            before_content: Some("before\n".to_string()),
            after_content: Some("after\n".to_string()),
            unified_diff: "-before\n+after\n".to_string(),
        };
        let request = ApprovalRequest {
            id: "approval-1".to_string(),
            thread_id: thread.id.clone(),
            turn_id: turn_id.clone(),
            tool_call_id: "call-1".to_string(),
            tool_name: "apply_patch".to_string(),
            reason: "review change".to_string(),
            risk: ToolRisk::Write,
            arguments: serde_json::json!({ "patch": "strict patch" }),
            preview: Some(PatchPreview {
                patch: "strict patch".to_string(),
                files: vec![preview_file.clone()],
                total_snapshot_bytes: 13,
            }),
            created_at_ms: now_ms(),
            expires_at_ms: now_ms() + 60_000,
        };
        let resolution = ApprovalResolution {
            action: ApprovalAction::Approved,
            patch: None,
            selected_paths: vec!["src/main.rs".to_string()],
            expected_hashes: vec![ExpectedFileHash {
                path: "src/main.rs".to_string(),
                before_hash: Some("before-hash".to_string()),
            }],
        };
        let change = ChangeSet {
            id: "change-1".to_string(),
            thread_id: thread.id.clone(),
            turn_id: turn_id.clone(),
            tool_call_id: "call-1".to_string(),
            created_at_ms: now_ms(),
            files: vec![ChangeFileSnapshot {
                path: preview_file.path,
                destination_path: preview_file.destination_path,
                operation: preview_file.operation,
                before_hash: preview_file.before_hash,
                after_hash: preview_file.after_hash,
                before_content: preview_file.before_content,
                after_content: preview_file.after_content,
                unified_diff: preview_file.unified_diff,
            }],
            undone: false,
        };
        for kind in [
            StoredEventKind::TurnStarted,
            StoredEventKind::ApprovalRequested {
                request: request.clone(),
            },
            StoredEventKind::ApprovalResolved {
                request_id: request.id.clone(),
                resolution: resolution.clone(),
            },
            StoredEventKind::ChangeApplied {
                change_set: change.clone(),
            },
            StoredEventKind::ChangeUndone {
                change_id: change.id.clone(),
            },
        ] {
            repository
                .append(StoredEvent::new(&thread.id, Some(turn_id.clone()), kind))
                .await
                .unwrap();
        }

        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.approvals.len(), 1);
        assert_eq!(detail.approvals[0].request, request);
        assert_eq!(detail.approvals[0].resolution, Some(resolution));
        assert_eq!(detail.changes.len(), 1);
        assert!(detail.changes[0].undone);
    }
}
