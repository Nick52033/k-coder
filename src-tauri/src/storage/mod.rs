use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use std::fs::OpenOptions;
#[cfg(test)]
use std::io::Write;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::context::CompactionSummary;
use crate::persistence::{
    HistoryIndexItem, HistoryIndexMetadata, HistoryIndexOrder, HistoryIndexTurn, ProjectionDb,
};
use crate::protocol::{
    AgentItemStatus, AgentItemType, AgentMessagePhase, AgentMode, ApprovalAction, ApprovalRequest,
    ApprovalResolution, ApprovalSnapshot, ChangeSet, ChatMessage, ContentBlock,
    HistorySortDirection, MessageRole, PROTOCOL_VERSION, ThreadHistorySnapshot, ThreadItem,
    ThreadItemEntry, ThreadItemPayload, ThreadItemsPage, ThreadTurn, ThreadTurnsPage, TodoItem,
    TokenUsage, ToolCall, ToolResult, TurnError, TurnItemsView, TurnState, UserInputAction,
    UserInputRequest, UserInputResolution,
};

mod history_pagination;
mod writer;

use writer::ThreadWriters;

pub const EVENT_SCHEMA_VERSION: u32 = 9;

fn default_in_project() -> bool {
    true
}
const MAX_TIMELINE_DETAIL_CHARS: usize = 2_000;
pub const DEFAULT_THREAD_HISTORY_PAGE_SIZE: u32 = 50;
pub const MAX_THREAD_HISTORY_PAGE_SIZE: u32 = 100;

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
            schema_version: EVENT_SCHEMA_VERSION,
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
        #[serde(default = "default_in_project")]
        in_project: bool,
    },
    ThreadWorkspaceBound {
        path: String,
    },
    ThreadForked {
        source_thread_id: String,
        last_turn_id: Option<String>,
    },
    ThreadRolledBack {
        retained_event_id: Option<String>,
        num_turns: u32,
    },
    UserMessage {
        message: ChatMessage,
    },
    TurnModeSelected {
        mode: AgentMode,
    },
    TurnStarted,
    ItemStarted {
        item_id: String,
        item_type: AgentItemType,
    },
    ItemCompleted {
        item_id: String,
        item_type: AgentItemType,
        status: AgentItemStatus,
    },
    AssistantMessage {
        message: ChatMessage,
    },
    AssistantToolCalls {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        #[serde(default)]
        text: String,
        calls: Vec<ToolCall>,
    },
    ToolStarted {
        call_id: String,
    },
    ReasoningSummary {
        item_id: String,
        summary: String,
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
    ProviderCallUsage {
        call_index: u32,
        usage: TokenUsage,
    },
    ContextCompacted {
        summary: CompactionSummary,
        automatic: bool,
    },
    ApprovalRequested {
        request: ApprovalRequest,
    },
    ApprovalResolved {
        request_id: String,
        resolution: ApprovalResolution,
    },
    UserInputRequested {
        request: UserInputRequest,
    },
    UserInputResolved {
        request_id: String,
        resolution: UserInputResolution,
    },
    TodoUpdated {
        todos: Vec<TodoItem>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<TurnError>,
    },
    TurnCancelled,
    ThreadArchived,
    ThreadRenamed {
        title: String,
    },
    ThreadDeleted,
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
    #[serde(default = "default_in_project")]
    pub in_project: bool,
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnSnapshot {
    pub turn_id: String,
    pub state: TurnState,
    pub error: Option<TurnError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetail {
    pub schema_version: u32,
    pub summary: ThreadSummary,
    pub messages: Vec<ChatMessage>,
    pub message_turn_ids: HashMap<String, String>,
    pub turn_user_message_ids: HashMap<String, String>,
    pub last_turn: Option<TurnSnapshot>,
    pub tool_activities: Vec<ToolActivitySnapshot>,
    pub turn_timeline: Vec<TurnTimelineItem>,
    pub approvals: Vec<ApprovalSnapshot>,
    pub user_inputs: Vec<UserInputSnapshot>,
    pub changes: Vec<ChangeSet>,
    pub todos: Vec<TodoItem>,
    pub last_usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolActivityState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolActivitySnapshot {
    pub turn_id: String,
    pub call: ToolCall,
    pub state: ToolActivityState,
    pub result: Option<ToolResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserInputSnapshot {
    pub request: UserInputRequest,
    pub resolution: Option<UserInputResolution>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimelineEventKind {
    ProviderContext,
    Usage,
    Compacted,
    ApprovalRequested,
    ApprovalResolved,
    ChangeApplied,
    ChangeUndone,
    UserInputRequested,
    UserInputResolved,
    TodoUpdated,
    TurnCompleted,
    TurnFailed,
    TurnCancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum TurnTimelineItem {
    Text {
        id: String,
        turn_id: String,
        text: String,
    },
    Reasoning {
        item_id: String,
        turn_id: String,
        summary: String,
    },
    Tool {
        activity: ToolActivitySnapshot,
    },
    Event {
        item_id: String,
        turn_id: String,
        kind: TimelineEventKind,
        title: String,
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
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
    writers: Arc<ThreadWriters>,
    workspace_binding_lock: Arc<Mutex<()>>,
    history_index_lock: Arc<Mutex<()>>,
    projection: ProjectionDb,
}

impl JsonlThreadRepository {
    pub fn new(data_root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let sessions_dir = data_root.as_ref().join("sessions");
        fs::create_dir_all(&sessions_dir).map_err(|error| StorageError::Io(error.to_string()))?;
        let repository = Self {
            sessions_dir,
            writers: Arc::new(ThreadWriters::new()),
            workspace_binding_lock: Arc::new(Mutex::new(())),
            history_index_lock: Arc::new(Mutex::new(())),
            projection: ProjectionDb::open(data_root.as_ref())
                .map_err(|error| StorageError::Io(error.to_string()))?,
        };
        repository.rebuild_projection()?;
        Ok(repository)
    }

    pub async fn create_thread(&self) -> Result<ThreadSummary, StorageError> {
        self.create_thread_with_project_mode(true).await
    }

    pub async fn create_standalone_thread(&self) -> Result<ThreadSummary, StorageError> {
        self.create_thread_with_project_mode(false).await
    }

    async fn create_thread_with_project_mode(
        &self,
        in_project: bool,
    ) -> Result<ThreadSummary, StorageError> {
        let thread_id = Uuid::new_v4().to_string();
        self.append(StoredEvent::new(
            &thread_id,
            None,
            StoredEventKind::ThreadCreated {
                title: "新会话".to_string(),
                in_project,
            },
        ))
        .await?;
        Ok(self.read_thread(&thread_id).await?.summary)
    }

    pub async fn create_thread_in_workspace(
        &self,
        workspace_root: &Path,
    ) -> Result<ThreadSummary, StorageError> {
        let thread = self.create_thread().await?;
        self.bind_thread_workspace(&thread.id, workspace_root).await
    }

    pub async fn bind_thread_workspace(
        &self,
        thread_id: &str,
        workspace_root: &Path,
    ) -> Result<ThreadSummary, StorageError> {
        let _binding_guard = self.workspace_binding_lock.lock().await;
        let detail = self.read_thread(thread_id).await?;
        if !detail.summary.in_project {
            return Err(StorageError::InvalidData(format!(
                "standalone thread {thread_id} cannot be bound to a workspace"
            )));
        }
        let path = workspace_root.to_string_lossy().into_owned();
        if let Some(bound) = &detail.summary.workspace_path {
            if bound == &path {
                return Ok(detail.summary);
            }
            return Err(StorageError::InvalidData(format!(
                "thread {thread_id} is already bound to workspace {bound}"
            )));
        }
        self.append(StoredEvent::new(
            thread_id,
            None,
            StoredEventKind::ThreadWorkspaceBound { path },
        ))
        .await?;
        Ok(self.read_thread(thread_id).await?.summary)
    }

    pub async fn list_threads(&self) -> Result<Vec<ThreadSummary>, StorageError> {
        self.projection
            .list_threads()
            .map_err(|error| StorageError::Io(error.to_string()))
    }

    pub async fn read_thread(&self, thread_id: &str) -> Result<ThreadDetail, StorageError> {
        let events = self.load(thread_id).await?;
        project_thread(thread_id, &events)
    }

    pub async fn read_thread_history(
        &self,
        thread_id: &str,
    ) -> Result<ThreadHistorySnapshot, StorageError> {
        self.ensure_history_index(thread_id).await?;
        let metadata = self
            .projection
            .history_metadata(thread_id)
            .map_err(projection_error)?
            .ok_or_else(|| StorageError::NotFound(thread_id.to_string()))?;
        let turns = history_pagination::paginate_indexed_turns(
            &self.projection,
            thread_id,
            None,
            Some(DEFAULT_THREAD_HISTORY_PAGE_SIZE),
            HistorySortDirection::Desc,
            TurnItemsView::Full,
        )?;
        Ok(ThreadHistorySnapshot {
            schema_version: PROTOCOL_VERSION,
            summary: metadata.summary,
            last_turn: metadata.last_turn,
            todos: metadata.todos,
            last_usage: metadata.last_usage,
            turns,
            unscoped_items: metadata.unscoped_items,
        })
    }

    pub async fn list_thread_turns(
        &self,
        thread_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
        sort_direction: HistorySortDirection,
        items_view: TurnItemsView,
    ) -> Result<ThreadTurnsPage, StorageError> {
        self.ensure_history_index(thread_id).await?;
        history_pagination::paginate_indexed_turns(
            &self.projection,
            thread_id,
            cursor,
            limit,
            sort_direction,
            items_view,
        )
    }

    pub async fn list_thread_items(
        &self,
        thread_id: &str,
        turn_id: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u32>,
        sort_direction: HistorySortDirection,
    ) -> Result<ThreadItemsPage, StorageError> {
        self.ensure_history_index(thread_id).await?;
        history_pagination::paginate_indexed_items(
            &self.projection,
            thread_id,
            turn_id,
            cursor,
            limit,
            sort_direction,
        )
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

    pub async fn rename_thread(
        &self,
        thread_id: &str,
        title: String,
    ) -> Result<ThreadSummary, StorageError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 120 {
            return Err(StorageError::InvalidData(
                "thread title must contain 1 to 120 characters".into(),
            ));
        }
        self.read_thread(thread_id).await?;
        self.append(StoredEvent::new(
            thread_id,
            None,
            StoredEventKind::ThreadRenamed {
                title: title.into(),
            },
        ))
        .await?;
        Ok(self.read_thread(thread_id).await?.summary)
    }

    pub async fn fork_thread(
        &self,
        source_thread_id: &str,
        last_turn_id: Option<&str>,
    ) -> Result<ThreadSummary, StorageError> {
        let source = self.read_thread(source_thread_id).await?;
        // Fork from the effective append-only view. Raw events may contain turns
        // hidden by an earlier rollback marker and must not be resurrected.
        let events = apply_history_rewrites(self.load(source_thread_id).await?)?;
        let through = match last_turn_id {
            Some(turn_id) => {
                events
                    .iter()
                    .position(|event| {
                        event.turn_id.as_deref() == Some(turn_id) && is_terminal_event(&event.kind)
                    })
                    .ok_or_else(|| {
                        StorageError::InvalidData(format!(
                            "last turn {turn_id} was not found or is not complete"
                        ))
                    })?
                    + 1
            }
            None => events.len(),
        };
        let destination = match (
            source.summary.in_project,
            source.summary.workspace_path.as_deref(),
        ) {
            (false, _) => self.create_standalone_thread().await?,
            (true, Some(path)) => self.create_thread_in_workspace(Path::new(path)).await?,
            (true, None) => self.create_thread().await?,
        };
        self.append(StoredEvent::new(
            &destination.id,
            None,
            StoredEventKind::ThreadForked {
                source_thread_id: source_thread_id.to_string(),
                last_turn_id: last_turn_id.map(str::to_string),
            },
        ))
        .await?;

        for source_event in events.into_iter().take(through) {
            if !is_fork_history_event(&source_event.kind) {
                continue;
            }
            let mut event = source_event;
            event.schema_version = EVENT_SCHEMA_VERSION;
            event.event_id = Uuid::new_v4().to_string();
            event.thread_id = destination.id.clone();
            match &mut event.kind {
                StoredEventKind::ApprovalRequested { request } => {
                    request.thread_id = destination.id.clone();
                }
                StoredEventKind::UserInputRequested { request } => {
                    request.thread_id = destination.id.clone();
                }
                _ => {}
            }
            self.append(event).await?;
        }
        self.rename_thread(&destination.id, format!("{} (分支)", source.summary.title))
            .await
    }

    pub async fn rollback_thread(
        &self,
        thread_id: &str,
        num_turns: u32,
    ) -> Result<ThreadHistorySnapshot, StorageError> {
        if num_turns == 0 {
            return Err(StorageError::InvalidData(
                "numTurns must be at least 1".to_string(),
            ));
        }
        // Rollback is defined over the current visible history, not over raw
        // terminal events that may already have been hidden by a prior rollback.
        let events = apply_history_rewrites(self.load(thread_id).await?)?;
        let terminals = events
            .iter()
            .enumerate()
            .filter(|(_, event)| is_terminal_event(&event.kind))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if terminals.is_empty() {
            return Err(StorageError::InvalidData(
                "thread has no completed turns to roll back".to_string(),
            ));
        }
        let keep_count = terminals.len().saturating_sub(num_turns as usize);
        let retained_event_id = if keep_count > 0 {
            Some(events[terminals[keep_count - 1]].event_id.clone())
        } else {
            events
                .iter()
                .take_while(|event| !is_history_event(&event.kind))
                .last()
                .map(|event| event.event_id.clone())
        };
        self.append(StoredEvent::new(
            thread_id,
            None,
            StoredEventKind::ThreadRolledBack {
                retained_event_id,
                num_turns,
            },
        ))
        .await?;
        self.read_thread_history(thread_id).await
    }

    pub async fn delete_thread(&self, thread_id: &str) -> Result<(), StorageError> {
        self.read_thread(thread_id).await?;
        self.append(StoredEvent::new(
            thread_id,
            None,
            StoredEventKind::ThreadDeleted,
        ))
        .await
    }

    pub async fn search_threads(&self, query: &str) -> Result<Vec<ThreadSummary>, StorageError> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return self.list_threads().await;
        }
        let mut matches = Vec::new();
        for entry in
            fs::read_dir(&self.sessions_dir).map_err(|error| StorageError::Io(error.to_string()))?
        {
            let path = entry
                .map_err(|error| StorageError::Io(error.to_string()))?
                .path();
            let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if Uuid::parse_str(id).is_err() {
                continue;
            }
            let detail = self.read_thread(id).await?;
            if !detail.summary.archived
                && (detail.summary.title.to_lowercase().contains(&query)
                    || detail
                        .messages
                        .iter()
                        .any(|message| message.text().to_lowercase().contains(&query)))
            {
                matches.push(detail.summary);
            }
        }
        matches.sort_by_key(|summary| std::cmp::Reverse(summary.updated_at_ms));
        Ok(matches)
    }

    async fn ensure_history_index(&self, thread_id: &str) -> Result<(), StorageError> {
        if self
            .projection
            .history_index_is_current(thread_id)
            .map_err(projection_error)?
        {
            return Ok(());
        }
        let _guard = self.history_index_lock.lock().await;
        if self
            .projection
            .history_index_is_current(thread_id)
            .map_err(projection_error)?
        {
            return Ok(());
        }

        let events = self.load(thread_id).await?;
        let detail = project_thread(thread_id, &events)?;
        let history = project_thread_history(thread_id, &events, &detail)?;
        let metadata = HistoryIndexMetadata {
            summary: detail.summary.clone(),
            last_turn: detail.last_turn.clone(),
            todos: detail.todos.clone(),
            last_usage: detail.last_usage,
            unscoped_items: history
                .items
                .iter()
                .rev()
                .filter(|item| item.item.turn_id.is_none())
                .take(MAX_THREAD_HISTORY_PAGE_SIZE as usize)
                .map(|item| item.item.clone())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
        };
        let turns = history
            .turns
            .iter()
            .map(|turn| HistoryIndexTurn {
                order: history_index_order(turn.order),
                turn: turn.turn.clone(),
            })
            .collect::<Vec<_>>();
        let items = history
            .items
            .iter()
            .map(|item| HistoryIndexItem {
                order: history_index_order(item.order),
                item: item.item.clone(),
            })
            .collect::<Vec<_>>();
        self.projection
            .replace_history_index(thread_id, events.len() as u64, &metadata, &turns, &items)
            .map_err(projection_error)
    }

    fn session_path(&self, thread_id: &str) -> Result<PathBuf, StorageError> {
        let id = Uuid::parse_str(thread_id)
            .map_err(|_| StorageError::InvalidData("thread ID must be a UUID".to_string()))?;
        Ok(self.sessions_dir.join(format!("{id}.jsonl")))
    }

    pub fn rebuild_projection(&self) -> Result<(), StorageError> {
        for entry in
            fs::read_dir(&self.sessions_dir).map_err(|error| StorageError::Io(error.to_string()))?
        {
            let path = entry
                .map_err(|error| StorageError::Io(error.to_string()))?
                .path();
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
            self.projection
                .replace_thread(&detail.summary, &events)
                .map_err(|error| StorageError::Io(error.to_string()))?;
        }
        Ok(())
    }

    pub fn projection(&self) -> ProjectionDb {
        self.projection.clone()
    }
}

#[async_trait]
impl ThreadRepository for JsonlThreadRepository {
    async fn append(&self, event: StoredEvent) -> Result<(), StorageError> {
        if event.schema_version != EVENT_SCHEMA_VERSION {
            return Err(StorageError::InvalidData(format!(
                "unsupported event schema version {}",
                event.schema_version
            )));
        }
        let path = self.session_path(&event.thread_id)?;
        let line = serde_json::to_vec(&event)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let _append_guard = self.writers.lock_thread(&event.thread_id).await;
        self.writers.append(&event.thread_id, path, line).await?;
        if matches!(&event.kind, StoredEventKind::ThreadRolledBack { .. }) {
            let events = self.load(&event.thread_id).await?;
            let detail = project_thread(&event.thread_id, &events)?;
            self.projection
                .replace_thread(&detail.summary, &events)
                .map_err(projection_error)?;
        } else {
            self.projection
                .append_event(&event)
                .map_err(projection_error)?;
        }
        Ok(())
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
            Ok(mut event) => {
                if event.schema_version < EVENT_SCHEMA_VERSION {
                    event.schema_version = EVENT_SCHEMA_VERSION;
                }
                if event.schema_version != EVENT_SCHEMA_VERSION {
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
    apply_history_rewrites(events)
}

fn apply_history_rewrites(events: Vec<StoredEvent>) -> Result<Vec<StoredEvent>, StorageError> {
    let mut effective = Vec::with_capacity(events.len());
    for event in events {
        let StoredEventKind::ThreadRolledBack {
            retained_event_id, ..
        } = &event.kind
        else {
            effective.push(event);
            continue;
        };

        let cutoff = match retained_event_id {
            Some(event_id) => {
                effective
                    .iter()
                    .position(|candidate| candidate.event_id == *event_id)
                    .ok_or_else(|| {
                        StorageError::InvalidData(format!(
                            "rollback retained event {event_id} is missing"
                        ))
                    })?
                    + 1
            }
            None => 0,
        };
        let metadata = effective
            .drain(cutoff..)
            .filter(|candidate| is_thread_metadata(&candidate.kind))
            .collect::<Vec<_>>();
        effective.extend(metadata);
        effective.push(event);
    }
    Ok(effective)
}

fn is_terminal_event(kind: &StoredEventKind) -> bool {
    matches!(
        kind,
        StoredEventKind::TurnCompleted { .. }
            | StoredEventKind::TurnFailed { .. }
            | StoredEventKind::TurnCancelled
    )
}

fn is_thread_metadata(kind: &StoredEventKind) -> bool {
    matches!(
        kind,
        StoredEventKind::ThreadCreated { .. }
            | StoredEventKind::ThreadWorkspaceBound { .. }
            | StoredEventKind::ThreadForked { .. }
            | StoredEventKind::ThreadRolledBack { .. }
            | StoredEventKind::ThreadArchived
            | StoredEventKind::ThreadRenamed { .. }
            | StoredEventKind::ThreadDeleted
    )
}

fn is_history_event(kind: &StoredEventKind) -> bool {
    !is_thread_metadata(kind)
}

fn is_fork_history_event(kind: &StoredEventKind) -> bool {
    !is_thread_metadata(kind)
        && !matches!(
            kind,
            StoredEventKind::ChangeApplied { .. }
                | StoredEventKind::ChangeUndone { .. }
                | StoredEventKind::ItemStarted {
                    item_type: AgentItemType::Change,
                    ..
                }
                | StoredEventKind::ItemCompleted {
                    item_type: AgentItemType::Change,
                    ..
                }
        )
}

fn project_thread(thread_id: &str, events: &[StoredEvent]) -> Result<ThreadDetail, StorageError> {
    let created = events
        .iter()
        .find_map(|event| match &event.kind {
            StoredEventKind::ThreadCreated { title, in_project } => {
                Some((title.clone(), event.created_at_ms, *in_project))
            }
            _ => None,
        })
        .ok_or_else(|| StorageError::InvalidData("thread_created event is missing".to_string()))?;

    let mut title = created.0;
    let mut messages = Vec::new();
    let mut message_turn_ids = HashMap::new();
    let mut turn_user_message_ids = HashMap::new();
    let mut latest_user_message_id: Option<String> = None;
    let mut archived = false;
    let in_project = created.2;
    let mut workspace_path = None;
    let mut last_turn = None;
    let mut tool_activities: Vec<ToolActivitySnapshot> = Vec::new();
    let mut turn_timeline: Vec<TurnTimelineItem> = Vec::new();
    let mut approvals: Vec<ApprovalSnapshot> = Vec::new();
    let mut user_inputs: Vec<UserInputSnapshot> = Vec::new();
    let mut changes: Vec<ChangeSet> = Vec::new();
    let mut todos: Vec<TodoItem> = Vec::new();
    let mut last_usage: Option<TokenUsage> = None;
    let mut turn_started_at_ms = HashMap::<String, u64>::new();
    let mut updated_at_ms = created.1;

    for event in events {
        if event.thread_id != thread_id {
            return Err(StorageError::InvalidData(
                "event thread ID does not match its session file".to_string(),
            ));
        }
        updated_at_ms = updated_at_ms.max(event.created_at_ms);
        match &event.kind {
            StoredEventKind::ItemStarted { .. } => {}
            StoredEventKind::ItemCompleted {
                item_id,
                item_type: AgentItemType::Tool,
                status,
            } => {
                finish_tool_activity(
                    &mut tool_activities,
                    &mut turn_timeline,
                    item_id,
                    event,
                    match status {
                        AgentItemStatus::Completed => ToolActivityState::Completed,
                        AgentItemStatus::Failed => ToolActivityState::Failed,
                        AgentItemStatus::Cancelled => ToolActivityState::Cancelled,
                    },
                );
            }
            StoredEventKind::ItemCompleted { .. } => {}
            StoredEventKind::ThreadWorkspaceBound { path } => {
                if !in_project {
                    return Err(StorageError::InvalidData(
                        "standalone thread contains a workspace binding".into(),
                    ));
                }
                if workspace_path
                    .as_ref()
                    .is_some_and(|bound: &String| bound != path)
                {
                    return Err(StorageError::InvalidData(
                        "thread contains conflicting workspace bindings".into(),
                    ));
                }
                workspace_path = Some(path.clone());
            }
            StoredEventKind::ThreadForked { .. } | StoredEventKind::ThreadRolledBack { .. } => {}
            StoredEventKind::UserMessage { message } => {
                if title == "新会话" && message.role == MessageRole::User {
                    title = title_from_message(&message.visible_text());
                }
                if let Some(turn_id) = &event.turn_id {
                    message_turn_ids.insert(message.id.clone(), turn_id.clone());
                }
                if message.role == MessageRole::User {
                    latest_user_message_id = Some(message.id.clone());
                }
                messages.push(message.clone());
            }
            StoredEventKind::AssistantMessage { message } => {
                if let Some(turn_id) = &event.turn_id {
                    message_turn_ids.insert(message.id.clone(), turn_id.clone());
                    if !message.text().is_empty() {
                        turn_timeline.push(TurnTimelineItem::Text {
                            id: message.id.clone(),
                            turn_id: turn_id.clone(),
                            text: message.text(),
                        });
                    }
                }
                if message.role == MessageRole::User {
                    latest_user_message_id = Some(message.id.clone());
                }
                messages.push(message.clone());
            }
            StoredEventKind::AssistantToolCalls {
                item_id,
                text,
                calls,
            } => {
                if let Some(turn_id) = &event.turn_id {
                    if !text.is_empty() {
                        turn_timeline.push(TurnTimelineItem::Text {
                            id: item_id.clone().unwrap_or_else(|| event.event_id.clone()),
                            turn_id: turn_id.clone(),
                            text: text.clone(),
                        });
                    }
                    for call in calls.iter().cloned() {
                        let activity = ToolActivitySnapshot {
                            turn_id: turn_id.clone(),
                            call,
                            state: ToolActivityState::Pending,
                            result: None,
                            started_at_ms: None,
                            completed_at_ms: None,
                            duration_ms: None,
                        };
                        tool_activities.push(activity.clone());
                        turn_timeline.push(TurnTimelineItem::Tool { activity });
                    }
                }
            }
            StoredEventKind::ToolStarted { call_id } => {
                if let Some(activity) = tool_activities
                    .iter_mut()
                    .rev()
                    .find(|activity| activity.call.id == *call_id)
                {
                    activity.state = ToolActivityState::Running;
                    activity.started_at_ms = Some(event.created_at_ms);
                }
                if let Some(TurnTimelineItem::Tool { activity }) = turn_timeline
                    .iter_mut()
                    .rev()
                    .find(|item| matches!(item, TurnTimelineItem::Tool { activity } if activity.call.id == *call_id))
                {
                    activity.state = ToolActivityState::Running;
                    activity.started_at_ms = Some(event.created_at_ms);
                }
            }
            StoredEventKind::ReasoningSummary { item_id, summary } => {
                if let Some(turn_id) = &event.turn_id {
                    turn_timeline.push(TurnTimelineItem::Reasoning {
                        item_id: item_id.clone(),
                        turn_id: turn_id.clone(),
                        summary: summary.clone(),
                    });
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
                    activity.completed_at_ms = Some(event.created_at_ms);
                    activity.duration_ms = activity
                        .started_at_ms
                        .map(|started| event.created_at_ms.saturating_sub(started));
                }
                if let Some(TurnTimelineItem::Tool { activity }) = turn_timeline
                    .iter_mut()
                    .rev()
                    .find(|item| matches!(item, TurnTimelineItem::Tool { activity } if activity.call.id == *call_id))
                {
                    activity.state = if result.success {
                        ToolActivityState::Completed
                    } else {
                        ToolActivityState::Failed
                    };
                    activity.result = Some(result.clone());
                    activity.completed_at_ms = Some(event.created_at_ms);
                    activity.duration_ms = activity
                        .started_at_ms
                        .map(|started| event.created_at_ms.saturating_sub(started));
                }
            }
            StoredEventKind::ProviderContext { provider, item } => {
                let context_type = item
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("provider_item");
                let provider_item_id = item
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(|id| format!("，项目 {id}"))
                    .unwrap_or_default();
                push_timeline_event(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::ProviderContext,
                    "已保留模型上下文",
                    Some(format!("{provider} · {context_type}{provider_item_id}")),
                );
            }
            StoredEventKind::ProviderCallUsage { call_index, usage } => {
                last_usage = Some(add_token_usage(last_usage.unwrap_or_default(), *usage));
                push_timeline_event(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::Usage,
                    format!("模型调用 {} 用量", call_index + 1),
                    Some(format!(
                        "输入 {} · 输出 {} · 总计 {} tokens",
                        usage.input_tokens, usage.output_tokens, usage.total_tokens
                    )),
                );
            }
            StoredEventKind::ContextCompacted { summary, automatic } => {
                push_timeline_event(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::Compacted,
                    if *automatic {
                        "已自动压缩上下文"
                    } else {
                        "已手动压缩上下文"
                    },
                    Some(format!(
                        "压缩了 {} 条历史消息，保留 {} 项用户约束、{} 项近期用户请求和 {} 项近期工具结果",
                        summary.compacted_message_count,
                        summary.user_constraints.len(),
                        summary.recent_user_messages.len(),
                        summary.recent_tool_results.len()
                    )),
                );
            }
            StoredEventKind::ApprovalRequested { request } => {
                approvals.push(ApprovalSnapshot {
                    request: request.clone(),
                    resolution: None,
                });
                push_timeline_event(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::ApprovalRequested,
                    if request.auto_approved {
                        "已自动批准操作"
                    } else {
                        "已请求操作确认"
                    },
                    Some(format!("{} · {}", request.tool_name, request.reason)),
                );
                if !request.auto_approved {
                    update_turn(&mut last_turn, event, TurnState::AwaitingApproval, None);
                }
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
                push_timeline_event(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::ApprovalResolved,
                    "操作确认已处理",
                    Some(format!(
                        "请求 {request_id} · {}",
                        approval_action_label(resolution.action)
                    )),
                );
                update_turn(&mut last_turn, event, TurnState::Streaming, None);
            }
            StoredEventKind::UserInputRequested { request } => {
                user_inputs.push(UserInputSnapshot {
                    request: request.clone(),
                    resolution: None,
                });
                let questions = request
                    .questions
                    .iter()
                    .map(|question| question.question.as_str())
                    .collect::<Vec<_>>()
                    .join("；");
                push_timeline_event(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::UserInputRequested,
                    "已请求用户输入",
                    Some(questions),
                );
                update_turn(&mut last_turn, event, TurnState::AwaitingApproval, None);
            }
            StoredEventKind::UserInputResolved {
                request_id,
                resolution,
            } => {
                if let Some(input) = user_inputs
                    .iter_mut()
                    .rev()
                    .find(|input| input.request.id == *request_id)
                {
                    input.resolution = Some(resolution.clone());
                }
                push_timeline_event(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::UserInputResolved,
                    "用户输入已处理",
                    Some(format!(
                        "请求 {request_id} · {}",
                        user_input_action_label(resolution.action)
                    )),
                );
                update_turn(&mut last_turn, event, TurnState::Streaming, None);
            }
            StoredEventKind::TodoUpdated { todos: updated } => {
                todos = updated.clone();
                let completed = updated
                    .iter()
                    .filter(|todo| matches!(todo.status, crate::protocol::TodoStatus::Completed))
                    .count();
                push_timeline_event(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::TodoUpdated,
                    "任务清单已更新",
                    Some(format!("已完成 {completed}/{} 项", updated.len())),
                );
            }
            StoredEventKind::ChangeApplied { change_set } => {
                changes.push(change_set.clone());
                let paths = change_set
                    .files
                    .iter()
                    .map(|file| file.path.as_str())
                    .collect::<Vec<_>>()
                    .join("、");
                push_timeline_event(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::ChangeApplied,
                    "编辑了文件",
                    Some(paths),
                );
            }
            StoredEventKind::ChangeUndone { change_id } => {
                if let Some(change) = changes
                    .iter_mut()
                    .rev()
                    .find(|change| change.id == *change_id)
                {
                    change.undone = true;
                }
                push_timeline_event(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::ChangeUndone,
                    "已撤销文件变更",
                    Some(format!("变更 {change_id}")),
                );
            }
            StoredEventKind::TurnModeSelected { .. } => {}
            StoredEventKind::TurnStarted => {
                if let Some(turn_id) = &event.turn_id {
                    turn_started_at_ms.insert(turn_id.clone(), event.created_at_ms);
                    last_usage = None;
                    if let Some(message_id) = &latest_user_message_id {
                        turn_user_message_ids.insert(turn_id.clone(), message_id.clone());
                    }
                    last_turn = Some(TurnSnapshot {
                        turn_id: turn_id.clone(),
                        state: TurnState::Streaming,
                        error: None,
                    });
                }
            }
            StoredEventKind::TurnCompleted { usage } => {
                if usage.is_some() {
                    last_usage = *usage;
                }
                finish_running_tool_activities(
                    &mut tool_activities,
                    &mut turn_timeline,
                    event,
                    ToolActivityState::Failed,
                );
                push_timeline_event_with_duration(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::TurnCompleted,
                    "Turn 已完成",
                    None,
                    turn_duration_ms(&turn_started_at_ms, event),
                );
                update_turn(&mut last_turn, event, TurnState::Completed, None)
            }
            StoredEventKind::TurnFailed { message, error } => {
                finish_running_tool_activities(
                    &mut tool_activities,
                    &mut turn_timeline,
                    event,
                    ToolActivityState::Failed,
                );
                push_timeline_event_with_duration(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::TurnFailed,
                    "Turn 执行失败",
                    Some(message.clone()),
                    turn_duration_ms(&turn_started_at_ms, event),
                );
                update_turn(
                    &mut last_turn,
                    event,
                    TurnState::Failed,
                    Some(
                        error
                            .clone()
                            .unwrap_or_else(|| TurnError::legacy(message.clone())),
                    ),
                )
            }
            StoredEventKind::TurnCancelled => {
                finish_running_tool_activities(
                    &mut tool_activities,
                    &mut turn_timeline,
                    event,
                    ToolActivityState::Cancelled,
                );
                push_timeline_event_with_duration(
                    &mut turn_timeline,
                    event,
                    TimelineEventKind::TurnCancelled,
                    "Turn 已取消",
                    None,
                    turn_duration_ms(&turn_started_at_ms, event),
                );
                update_turn(&mut last_turn, event, TurnState::Cancelled, None)
            }
            StoredEventKind::ThreadArchived => archived = true,
            StoredEventKind::ThreadRenamed { title: renamed } => title = renamed.clone(),
            StoredEventKind::ThreadDeleted => archived = true,
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
            in_project,
            workspace_path,
        },
        messages,
        message_turn_ids,
        turn_user_message_ids,
        last_turn,
        tool_activities,
        turn_timeline,
        approvals,
        user_inputs,
        changes,
        todos,
        last_usage,
    })
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
struct HistoryOrder {
    event_index: u64,
    item_index: u32,
}

#[derive(Debug, Clone)]
struct ProjectedHistory {
    turns: Vec<ProjectedTurn>,
    items: Vec<ProjectedItem>,
}

#[derive(Debug, Clone)]
struct ProjectedTurn {
    order: HistoryOrder,
    turn: ThreadTurn,
}

#[derive(Debug, Clone)]
struct ProjectedItem {
    order: HistoryOrder,
    item: ThreadItem,
}

#[derive(Debug, Clone)]
struct ItemLifecycleSnapshot {
    turn_id: Option<String>,
    item_id: String,
    item_type: AgentItemType,
    started_at_ms: Option<u64>,
    completed_at_ms: Option<u64>,
    status: Option<AgentItemStatus>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum HistoryCursorResource {
    Turns,
    Items,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct HistoryCursor {
    version: u32,
    thread_id: String,
    resource: HistoryCursorResource,
    anchor_id: String,
    order: HistoryOrder,
    inclusive: bool,
    filter_turn_id: Option<String>,
}

fn project_thread_history(
    thread_id: &str,
    events: &[StoredEvent],
    detail: &ThreadDetail,
) -> Result<ProjectedHistory, StorageError> {
    let lifecycles = project_item_lifecycles(events);
    let mut turns = project_turn_metadata(events, detail);
    let mut items = Vec::<ProjectedItem>::new();
    let mut inserted_user_messages = HashSet::<String>::new();
    let mut grouped_approvals = HashSet::<String>::new();
    let mut grouped_user_inputs = HashSet::<String>::new();

    for (event_index, event) in events.iter().enumerate() {
        if event.thread_id != thread_id {
            return Err(StorageError::InvalidData(
                "event thread ID does not match its session file".to_string(),
            ));
        }
        let mut item_index = 0u32;
        let mut next_order = || {
            let order = HistoryOrder {
                event_index: event_index as u64,
                item_index,
            };
            item_index = item_index.saturating_add(1);
            order
        };

        match &event.kind {
            StoredEventKind::UserMessage { message } => {
                if !inserted_user_messages.insert(message.id.clone()) {
                    continue;
                }
                let turn_id = event.turn_id.clone().or_else(|| {
                    turns
                        .iter()
                        .find(|turn| turn.turn.user_message_id.as_deref() == Some(&message.id))
                        .map(|turn| turn.turn.id.clone())
                });
                let item = ThreadItem {
                    schema_version: PROTOCOL_VERSION,
                    id: message.id.clone(),
                    turn_id,
                    status: Some(AgentItemStatus::Completed),
                    started_at_ms: Some(message.created_at_ms),
                    completed_at_ms: Some(message.created_at_ms),
                    timeline_items: Vec::new(),
                    payload: ThreadItemPayload::UserMessage {
                        message: message.clone(),
                    },
                };
                push_history_item(&mut turns, &mut items, next_order(), item);
            }
            StoredEventKind::AssistantMessage { message } => {
                let Some(turn_id) = event.turn_id.as_deref() else {
                    continue;
                };
                let timeline_items = find_timeline_text(detail, turn_id, &message.id)
                    .into_iter()
                    .collect();
                let item = history_item(
                    &lifecycles,
                    event,
                    message.id.clone(),
                    Some(AgentItemType::AgentMessage),
                    Some(AgentItemStatus::Completed),
                    ThreadItemPayload::AgentMessage {
                        message: message.clone(),
                        phase: AgentMessagePhase::FinalAnswer,
                    },
                    timeline_items,
                );
                push_history_item(&mut turns, &mut items, next_order(), item);
            }
            StoredEventKind::AssistantToolCalls {
                item_id,
                text,
                calls,
            } => {
                let Some(turn_id) = event.turn_id.as_deref() else {
                    continue;
                };
                if !text.is_empty() {
                    let item_id = item_id.clone().unwrap_or_else(|| event.event_id.clone());
                    let message = ChatMessage {
                        schema_version: PROTOCOL_VERSION,
                        id: item_id.clone(),
                        role: MessageRole::Assistant,
                        content: vec![ContentBlock::Text { text: text.clone() }],
                        created_at_ms: event.created_at_ms,
                    };
                    let timeline_items = find_timeline_text(detail, turn_id, &item_id)
                        .into_iter()
                        .collect();
                    let item = history_item(
                        &lifecycles,
                        event,
                        item_id,
                        Some(AgentItemType::AgentMessage),
                        Some(AgentItemStatus::Completed),
                        ThreadItemPayload::AgentMessage {
                            message,
                            phase: AgentMessagePhase::Commentary,
                        },
                        timeline_items,
                    );
                    push_history_item(&mut turns, &mut items, next_order(), item);
                }
                for call in calls {
                    let Some(activity) = detail
                        .tool_activities
                        .iter()
                        .find(|activity| activity.turn_id == turn_id && activity.call.id == call.id)
                        .cloned()
                    else {
                        continue;
                    };
                    let fallback_status = tool_item_status(&activity.state);
                    let timeline_items = find_timeline_tool(detail, turn_id, &call.id)
                        .into_iter()
                        .collect();
                    let mut item = history_item(
                        &lifecycles,
                        event,
                        call.id.clone(),
                        Some(AgentItemType::Tool),
                        fallback_status,
                        ThreadItemPayload::Tool {
                            activity: activity.clone(),
                        },
                        timeline_items,
                    );
                    if !has_completed_lifecycle(
                        &lifecycles,
                        event.turn_id.as_deref(),
                        &call.id,
                        AgentItemType::Tool,
                    ) {
                        item.completed_at_ms = activity.completed_at_ms;
                    }
                    push_history_item(&mut turns, &mut items, next_order(), item);
                }
            }
            StoredEventKind::ReasoningSummary { item_id, summary } => {
                let Some(turn_id) = event.turn_id.as_deref() else {
                    continue;
                };
                let timeline_items = find_timeline_reasoning(detail, turn_id, item_id)
                    .into_iter()
                    .collect();
                let item = history_item(
                    &lifecycles,
                    event,
                    item_id.clone(),
                    Some(AgentItemType::Reasoning),
                    Some(AgentItemStatus::Completed),
                    ThreadItemPayload::Reasoning {
                        summary: summary.clone(),
                    },
                    timeline_items,
                );
                push_history_item(&mut turns, &mut items, next_order(), item);
            }
            StoredEventKind::ApprovalRequested { request } => {
                grouped_approvals.insert(request.id.clone());
                let Some(approval) = detail
                    .approvals
                    .iter()
                    .find(|approval| approval.request.id == request.id)
                    .cloned()
                else {
                    continue;
                };
                let timeline_items = approval_timeline_items(detail, &request.id);
                let fallback_status =
                    approval
                        .resolution
                        .as_ref()
                        .map(|resolution| match resolution.action {
                            ApprovalAction::Approved => AgentItemStatus::Completed,
                            ApprovalAction::Rejected | ApprovalAction::TimedOut => {
                                AgentItemStatus::Failed
                            }
                            ApprovalAction::Cancelled => AgentItemStatus::Cancelled,
                        });
                let mut item = history_item(
                    &lifecycles,
                    event,
                    request.id.clone(),
                    Some(AgentItemType::Approval),
                    fallback_status,
                    ThreadItemPayload::Approval { approval },
                    timeline_items,
                );
                if !has_completed_lifecycle(
                    &lifecycles,
                    event.turn_id.as_deref(),
                    &request.id,
                    AgentItemType::Approval,
                ) {
                    item.completed_at_ms =
                        events.iter().find_map(|candidate| match &candidate.kind {
                            StoredEventKind::ApprovalResolved { request_id, .. }
                                if request_id == &request.id =>
                            {
                                Some(candidate.created_at_ms)
                            }
                            _ => None,
                        });
                }
                push_history_item(&mut turns, &mut items, next_order(), item);
            }
            StoredEventKind::ApprovalResolved { request_id, .. } => {
                if !grouped_approvals.contains(request_id) {
                    push_generic_event_item(&mut turns, &mut items, detail, event, next_order());
                }
            }
            StoredEventKind::UserInputRequested { request } => {
                grouped_user_inputs.insert(request.id.clone());
                let Some(user_input) = detail
                    .user_inputs
                    .iter()
                    .find(|input| input.request.id == request.id)
                    .cloned()
                else {
                    continue;
                };
                let timeline_items = user_input_timeline_items(detail, &request.id);
                let fallback_status =
                    user_input
                        .resolution
                        .as_ref()
                        .map(|resolution| match resolution.action {
                            UserInputAction::Answered => AgentItemStatus::Completed,
                            UserInputAction::Skipped => AgentItemStatus::Failed,
                            UserInputAction::Cancelled => AgentItemStatus::Cancelled,
                        });
                let mut item = history_item(
                    &lifecycles,
                    event,
                    request.id.clone(),
                    Some(AgentItemType::UserInput),
                    fallback_status,
                    ThreadItemPayload::UserInput { user_input },
                    timeline_items,
                );
                if !has_completed_lifecycle(
                    &lifecycles,
                    event.turn_id.as_deref(),
                    &request.id,
                    AgentItemType::UserInput,
                ) {
                    item.completed_at_ms =
                        events.iter().find_map(|candidate| match &candidate.kind {
                            StoredEventKind::UserInputResolved { request_id, .. }
                                if request_id == &request.id =>
                            {
                                Some(candidate.created_at_ms)
                            }
                            _ => None,
                        });
                }
                push_history_item(&mut turns, &mut items, next_order(), item);
            }
            StoredEventKind::UserInputResolved { request_id, .. } => {
                if !grouped_user_inputs.contains(request_id) {
                    push_generic_event_item(&mut turns, &mut items, detail, event, next_order());
                }
            }
            StoredEventKind::ChangeApplied { change_set } => {
                let change_set = detail
                    .changes
                    .iter()
                    .find(|change| change.id == change_set.id)
                    .cloned()
                    .unwrap_or_else(|| change_set.clone());
                let timeline_items = find_timeline_event(detail, event).into_iter().collect();
                let item = history_item(
                    &lifecycles,
                    event,
                    change_set.id.clone(),
                    Some(AgentItemType::Change),
                    Some(AgentItemStatus::Completed),
                    ThreadItemPayload::Change { change_set },
                    timeline_items,
                );
                push_history_item(&mut turns, &mut items, next_order(), item);
            }
            StoredEventKind::ContextCompacted { summary, automatic } => {
                let timeline_items = find_timeline_event(detail, event).into_iter().collect();
                let item = history_item(
                    &lifecycles,
                    event,
                    event.event_id.clone(),
                    Some(AgentItemType::ContextCompaction),
                    Some(AgentItemStatus::Completed),
                    ThreadItemPayload::ContextCompaction {
                        automatic: *automatic,
                        compacted_message_count: summary.compacted_message_count,
                        user_constraint_count: summary.user_constraints.len(),
                        recent_tool_result_count: summary.recent_tool_results.len(),
                        recent_user_message_count: summary.recent_user_messages.len(),
                    },
                    timeline_items,
                );
                push_history_item(&mut turns, &mut items, next_order(), item);
            }
            StoredEventKind::ProviderContext { .. }
            | StoredEventKind::ProviderCallUsage { .. }
            | StoredEventKind::TodoUpdated { .. }
            | StoredEventKind::ChangeUndone { .. }
            | StoredEventKind::TurnCompleted { .. }
            | StoredEventKind::TurnFailed { .. }
            | StoredEventKind::TurnCancelled => {
                push_generic_event_item(&mut turns, &mut items, detail, event, next_order());
            }
            StoredEventKind::ThreadCreated { .. }
            | StoredEventKind::ThreadWorkspaceBound { .. }
            | StoredEventKind::ThreadForked { .. }
            | StoredEventKind::ThreadRolledBack { .. }
            | StoredEventKind::TurnStarted
            | StoredEventKind::TurnModeSelected { .. }
            | StoredEventKind::ItemStarted { .. }
            | StoredEventKind::ItemCompleted { .. }
            | StoredEventKind::ToolStarted { .. }
            | StoredEventKind::ToolResult { .. }
            | StoredEventKind::ThreadArchived
            | StoredEventKind::ThreadRenamed { .. }
            | StoredEventKind::ThreadDeleted => {}
        }
    }

    items.sort_by_key(|item| item.order);
    for turn in &mut turns {
        turn.turn
            .items
            .sort_by_key(|item| turn_item_display_order(detail, &items, item));
    }

    Ok(ProjectedHistory { turns, items })
}

fn project_item_lifecycles(events: &[StoredEvent]) -> Vec<ItemLifecycleSnapshot> {
    let mut lifecycles = Vec::<ItemLifecycleSnapshot>::new();
    for event in events {
        match &event.kind {
            StoredEventKind::ItemStarted { item_id, item_type } => {
                if let Some(lifecycle) = lifecycles.iter_mut().rev().find(|lifecycle| {
                    lifecycle.turn_id == event.turn_id
                        && lifecycle.item_id == *item_id
                        && lifecycle.item_type == *item_type
                }) {
                    lifecycle.started_at_ms.get_or_insert(event.created_at_ms);
                } else {
                    lifecycles.push(ItemLifecycleSnapshot {
                        turn_id: event.turn_id.clone(),
                        item_id: item_id.clone(),
                        item_type: *item_type,
                        started_at_ms: Some(event.created_at_ms),
                        completed_at_ms: None,
                        status: None,
                    });
                }
            }
            StoredEventKind::ItemCompleted {
                item_id,
                item_type,
                status,
            } => {
                if let Some(lifecycle) = lifecycles.iter_mut().rev().find(|lifecycle| {
                    lifecycle.turn_id == event.turn_id
                        && lifecycle.item_id == *item_id
                        && lifecycle.item_type == *item_type
                }) {
                    lifecycle.completed_at_ms = Some(event.created_at_ms);
                    lifecycle.status = Some(*status);
                } else {
                    lifecycles.push(ItemLifecycleSnapshot {
                        turn_id: event.turn_id.clone(),
                        item_id: item_id.clone(),
                        item_type: *item_type,
                        started_at_ms: None,
                        completed_at_ms: Some(event.created_at_ms),
                        status: Some(*status),
                    });
                }
            }
            _ => {}
        }
    }
    lifecycles
}

fn has_completed_lifecycle(
    lifecycles: &[ItemLifecycleSnapshot],
    turn_id: Option<&str>,
    item_id: &str,
    item_type: AgentItemType,
) -> bool {
    lifecycles.iter().any(|lifecycle| {
        lifecycle.turn_id.as_deref() == turn_id
            && lifecycle.item_id == item_id
            && lifecycle.item_type == item_type
            && lifecycle.completed_at_ms.is_some()
    })
}

fn turn_item_display_order(
    detail: &ThreadDetail,
    items: &[ProjectedItem],
    item: &ThreadItem,
) -> (u8, u64, u32) {
    if matches!(&item.payload, ThreadItemPayload::UserMessage { .. }) {
        let order = projected_item_order(items, item);
        return (0, order.event_index, order.item_index);
    }
    if let ThreadItemPayload::Approval { approval } = &item.payload
        && let Some(index) = detail.turn_timeline.iter().position(|candidate| {
            matches!(candidate, TurnTimelineItem::Tool { activity }
                if activity.turn_id == approval.request.turn_id
                    && activity.call.id == approval.request.tool_call_id)
        })
    {
        return (1, (index as u64).saturating_mul(2).saturating_sub(1), 0);
    }
    if let Some(index) = item
        .timeline_items
        .iter()
        .filter_map(|timeline_item| {
            detail.turn_timeline.iter().position(|candidate| {
                timeline_item_id(candidate) == timeline_item_id(timeline_item)
                    && timeline_item_turn_id(candidate) == timeline_item_turn_id(timeline_item)
            })
        })
        .min()
    {
        return (1, (index as u64).saturating_mul(2), 0);
    }
    let order = projected_item_order(items, item);
    (2, order.event_index, order.item_index)
}

fn projected_item_order(items: &[ProjectedItem], item: &ThreadItem) -> HistoryOrder {
    items
        .iter()
        .find(|projected| projected.item.id == item.id && projected.item.turn_id == item.turn_id)
        .map(|projected| projected.order)
        .unwrap_or(HistoryOrder {
            event_index: u64::MAX,
            item_index: u32::MAX,
        })
}

fn timeline_item_turn_id(item: &TurnTimelineItem) -> &str {
    match item {
        TurnTimelineItem::Text { turn_id, .. }
        | TurnTimelineItem::Reasoning { turn_id, .. }
        | TurnTimelineItem::Event { turn_id, .. } => turn_id,
        TurnTimelineItem::Tool { activity } => &activity.turn_id,
    }
}

fn project_turn_metadata(events: &[StoredEvent], detail: &ThreadDetail) -> Vec<ProjectedTurn> {
    let mut turns = Vec::<ProjectedTurn>::new();
    for (event_index, event) in events.iter().enumerate() {
        let Some(turn_id) = event.turn_id.as_deref() else {
            continue;
        };
        if !turns.iter().any(|turn| turn.turn.id == turn_id) {
            turns.push(ProjectedTurn {
                order: HistoryOrder {
                    event_index: event_index as u64,
                    item_index: 0,
                },
                turn: ThreadTurn {
                    schema_version: PROTOCOL_VERSION,
                    id: turn_id.to_string(),
                    user_message_id: detail.turn_user_message_ids.get(turn_id).cloned(),
                    state: TurnState::Streaming,
                    error: None,
                    started_at_ms: Some(event.created_at_ms),
                    completed_at_ms: None,
                    duration_ms: None,
                    items_view: TurnItemsView::Full,
                    items: Vec::new(),
                },
            });
        }
        let Some(turn) = turns.iter_mut().find(|turn| turn.turn.id == turn_id) else {
            continue;
        };
        match &event.kind {
            StoredEventKind::TurnStarted => {
                turn.turn.state = TurnState::Streaming;
                turn.turn.started_at_ms = Some(event.created_at_ms);
            }
            StoredEventKind::ToolStarted { .. } => turn.turn.state = TurnState::RunningTool,
            StoredEventKind::ToolResult { .. }
            | StoredEventKind::ApprovalResolved { .. }
            | StoredEventKind::UserInputResolved { .. } => {
                turn.turn.state = TurnState::Streaming;
            }
            StoredEventKind::ApprovalRequested { request } if !request.auto_approved => {
                turn.turn.state = TurnState::AwaitingApproval;
            }
            StoredEventKind::UserInputRequested { .. } => {
                turn.turn.state = TurnState::AwaitingApproval;
            }
            StoredEventKind::TurnCompleted { .. } => {
                finish_thread_turn(&mut turn.turn, event, TurnState::Completed, None);
            }
            StoredEventKind::TurnFailed { message, error } => {
                finish_thread_turn(
                    &mut turn.turn,
                    event,
                    TurnState::Failed,
                    Some(
                        error
                            .clone()
                            .unwrap_or_else(|| TurnError::legacy(message.clone())),
                    ),
                );
            }
            StoredEventKind::TurnCancelled => {
                finish_thread_turn(&mut turn.turn, event, TurnState::Cancelled, None);
            }
            _ => {}
        }
    }
    turns
}

fn finish_thread_turn(
    turn: &mut ThreadTurn,
    event: &StoredEvent,
    state: TurnState,
    error: Option<TurnError>,
) {
    turn.state = state;
    turn.error = error;
    turn.completed_at_ms = Some(event.created_at_ms);
    turn.duration_ms = turn
        .started_at_ms
        .map(|started| event.created_at_ms.saturating_sub(started));
}

fn history_item(
    lifecycles: &[ItemLifecycleSnapshot],
    event: &StoredEvent,
    item_id: String,
    item_type: Option<AgentItemType>,
    fallback_status: Option<AgentItemStatus>,
    payload: ThreadItemPayload,
    timeline_items: Vec<TurnTimelineItem>,
) -> ThreadItem {
    let lifecycle = item_type.and_then(|item_type| {
        lifecycles.iter().rev().find(|lifecycle| {
            lifecycle.turn_id == event.turn_id
                && lifecycle.item_id == item_id
                && lifecycle.item_type == item_type
        })
    });
    let status = lifecycle
        .and_then(|lifecycle| lifecycle.status)
        .or(fallback_status);
    ThreadItem {
        schema_version: PROTOCOL_VERSION,
        id: item_id,
        turn_id: event.turn_id.clone(),
        status,
        started_at_ms: lifecycle
            .and_then(|lifecycle| lifecycle.started_at_ms)
            .or(Some(event.created_at_ms)),
        completed_at_ms: lifecycle
            .and_then(|lifecycle| lifecycle.completed_at_ms)
            .or_else(|| status.map(|_| event.created_at_ms)),
        timeline_items,
        payload,
    }
}

fn push_history_item(
    turns: &mut [ProjectedTurn],
    items: &mut Vec<ProjectedItem>,
    order: HistoryOrder,
    item: ThreadItem,
) {
    if let Some(turn_id) = item.turn_id.as_deref()
        && let Some(turn) = turns.iter_mut().find(|turn| turn.turn.id == turn_id)
    {
        turn.turn.items.push(item.clone());
    }
    items.push(ProjectedItem { order, item });
}

fn push_generic_event_item(
    turns: &mut [ProjectedTurn],
    items: &mut Vec<ProjectedItem>,
    detail: &ThreadDetail,
    event: &StoredEvent,
    order: HistoryOrder,
) {
    let Some(timeline_item) = find_timeline_event(detail, event) else {
        return;
    };
    let item_id = timeline_item_id(&timeline_item).to_string();
    let item = ThreadItem {
        schema_version: PROTOCOL_VERSION,
        id: item_id,
        turn_id: event.turn_id.clone(),
        status: None,
        started_at_ms: Some(event.created_at_ms),
        completed_at_ms: Some(event.created_at_ms),
        timeline_items: vec![timeline_item],
        payload: ThreadItemPayload::Event,
    };
    push_history_item(turns, items, order, item);
}

fn find_timeline_text(
    detail: &ThreadDetail,
    turn_id: &str,
    item_id: &str,
) -> Option<TurnTimelineItem> {
    detail.turn_timeline.iter().find_map(|item| match item {
        TurnTimelineItem::Text {
            id,
            turn_id: item_turn_id,
            ..
        } if id == item_id && item_turn_id == turn_id => Some(item.clone()),
        _ => None,
    })
}

fn find_timeline_reasoning(
    detail: &ThreadDetail,
    turn_id: &str,
    item_id: &str,
) -> Option<TurnTimelineItem> {
    detail.turn_timeline.iter().find_map(|item| match item {
        TurnTimelineItem::Reasoning {
            item_id: reasoning_id,
            turn_id: item_turn_id,
            ..
        } if reasoning_id == item_id && item_turn_id == turn_id => Some(item.clone()),
        _ => None,
    })
}

fn find_timeline_tool(
    detail: &ThreadDetail,
    turn_id: &str,
    call_id: &str,
) -> Option<TurnTimelineItem> {
    detail.turn_timeline.iter().find_map(|item| match item {
        TurnTimelineItem::Tool { activity }
            if activity.turn_id == turn_id && activity.call.id == call_id =>
        {
            Some(item.clone())
        }
        _ => None,
    })
}

fn approval_timeline_items(detail: &ThreadDetail, request_id: &str) -> Vec<TurnTimelineItem> {
    let requested_id = format!("approval-requested-{request_id}");
    let resolved_id = format!("approval-resolved-{request_id}");
    detail
        .turn_timeline
        .iter()
        .filter(|item| {
            matches!(
                item,
                TurnTimelineItem::Event { item_id, .. }
                    if item_id == &requested_id || item_id == &resolved_id
            )
        })
        .cloned()
        .collect()
}

fn user_input_timeline_items(detail: &ThreadDetail, request_id: &str) -> Vec<TurnTimelineItem> {
    let requested_id = format!("user-input-requested-{request_id}");
    let resolved_id = format!("user-input-resolved-{request_id}");
    detail
        .turn_timeline
        .iter()
        .filter(|item| {
            matches!(
                item,
                TurnTimelineItem::Event { item_id, .. }
                    if item_id == &requested_id || item_id == &resolved_id
            )
        })
        .cloned()
        .collect()
}

fn find_timeline_event(detail: &ThreadDetail, event: &StoredEvent) -> Option<TurnTimelineItem> {
    let item_id = timeline_event_id(event)?;
    detail.turn_timeline.iter().find_map(|item| match item {
        TurnTimelineItem::Event {
            item_id: timeline_id,
            ..
        } if timeline_id == &item_id => Some(item.clone()),
        _ => None,
    })
}

fn timeline_event_id(event: &StoredEvent) -> Option<String> {
    let turn_id = event.turn_id.as_deref()?;
    Some(match &event.kind {
        StoredEventKind::ApprovalRequested { request } => {
            format!("approval-requested-{}", request.id)
        }
        StoredEventKind::ApprovalResolved { request_id, .. } => {
            format!("approval-resolved-{request_id}")
        }
        StoredEventKind::UserInputRequested { request } => {
            format!("user-input-requested-{}", request.id)
        }
        StoredEventKind::UserInputResolved { request_id, .. } => {
            format!("user-input-resolved-{request_id}")
        }
        StoredEventKind::ChangeApplied { change_set } => {
            format!("change-applied-{}", change_set.id)
        }
        StoredEventKind::ChangeUndone { change_id } => format!("change-undone-{change_id}"),
        StoredEventKind::TurnCompleted { .. } => format!("turn-completed-{turn_id}"),
        StoredEventKind::TurnFailed { .. } => format!("turn-failed-{turn_id}"),
        StoredEventKind::TurnCancelled => format!("turn-cancelled-{turn_id}"),
        StoredEventKind::ProviderContext { .. }
        | StoredEventKind::ProviderCallUsage { .. }
        | StoredEventKind::ContextCompacted { .. }
        | StoredEventKind::TodoUpdated { .. } => event.event_id.clone(),
        _ => return None,
    })
}

fn timeline_item_id(item: &TurnTimelineItem) -> &str {
    match item {
        TurnTimelineItem::Text { id, .. } => id,
        TurnTimelineItem::Reasoning { item_id, .. } | TurnTimelineItem::Event { item_id, .. } => {
            item_id
        }
        TurnTimelineItem::Tool { activity } => &activity.call.id,
    }
}

fn tool_item_status(state: &ToolActivityState) -> Option<AgentItemStatus> {
    match state {
        ToolActivityState::Completed => Some(AgentItemStatus::Completed),
        ToolActivityState::Failed => Some(AgentItemStatus::Failed),
        ToolActivityState::Cancelled => Some(AgentItemStatus::Cancelled),
        ToolActivityState::Pending | ToolActivityState::Running => None,
    }
}

fn projection_error(error: crate::persistence::ProjectionError) -> StorageError {
    StorageError::Io(error.to_string())
}

fn history_index_order(order: HistoryOrder) -> HistoryIndexOrder {
    HistoryIndexOrder {
        event_index: order.event_index,
        item_index: order.item_index,
    }
}

fn projected_history_order(order: HistoryIndexOrder) -> HistoryOrder {
    HistoryOrder {
        event_index: order.event_index,
        item_index: order.item_index,
    }
}

fn push_timeline_event(
    timeline: &mut Vec<TurnTimelineItem>,
    event: &StoredEvent,
    kind: TimelineEventKind,
    title: impl Into<String>,
    detail: Option<String>,
) {
    push_timeline_event_with_duration(timeline, event, kind, title, detail, None);
}

fn push_timeline_event_with_duration(
    timeline: &mut Vec<TurnTimelineItem>,
    event: &StoredEvent,
    kind: TimelineEventKind,
    title: impl Into<String>,
    detail: Option<String>,
    duration_ms: Option<u64>,
) {
    let Some(turn_id) = &event.turn_id else {
        return;
    };
    let item_id = match &event.kind {
        StoredEventKind::ApprovalRequested { request } => {
            format!("approval-requested-{}", request.id)
        }
        StoredEventKind::ApprovalResolved { request_id, .. } => {
            format!("approval-resolved-{request_id}")
        }
        StoredEventKind::UserInputRequested { request } => {
            format!("user-input-requested-{}", request.id)
        }
        StoredEventKind::UserInputResolved { request_id, .. } => {
            format!("user-input-resolved-{request_id}")
        }
        StoredEventKind::ChangeApplied { change_set } => {
            format!("change-applied-{}", change_set.id)
        }
        StoredEventKind::ChangeUndone { change_id } => format!("change-undone-{change_id}"),
        StoredEventKind::TurnCompleted { .. } => format!("turn-completed-{turn_id}"),
        StoredEventKind::TurnFailed { .. } => format!("turn-failed-{turn_id}"),
        StoredEventKind::TurnCancelled => format!("turn-cancelled-{turn_id}"),
        _ => event.event_id.clone(),
    };
    timeline.push(TurnTimelineItem::Event {
        item_id,
        turn_id: turn_id.clone(),
        kind,
        title: bound_timeline_text(title.into()),
        detail: detail.map(bound_timeline_text),
        duration_ms,
    });
}

fn turn_duration_ms(started_at: &HashMap<String, u64>, event: &StoredEvent) -> Option<u64> {
    event
        .turn_id
        .as_ref()
        .and_then(|turn_id| started_at.get(turn_id))
        .map(|started_at_ms| event.created_at_ms.saturating_sub(*started_at_ms))
}

fn finish_running_tool_activities(
    tool_activities: &mut [ToolActivitySnapshot],
    turn_timeline: &mut [TurnTimelineItem],
    event: &StoredEvent,
    terminal_state: ToolActivityState,
) {
    let Some(turn_id) = event.turn_id.as_deref() else {
        return;
    };
    let finish = |activity: &mut ToolActivitySnapshot| {
        if activity.turn_id == turn_id
            && matches!(
                activity.state,
                ToolActivityState::Pending | ToolActivityState::Running
            )
        {
            activity.state = terminal_state.clone();
            activity.completed_at_ms = Some(event.created_at_ms);
            activity.duration_ms = activity
                .started_at_ms
                .map(|started| event.created_at_ms.saturating_sub(started));
        }
    };
    tool_activities.iter_mut().for_each(finish);
    turn_timeline.iter_mut().for_each(|item| {
        if let TurnTimelineItem::Tool { activity } = item {
            finish(activity);
        }
    });
}

fn bound_timeline_text(value: String) -> String {
    if value.chars().count() <= MAX_TIMELINE_DETAIL_CHARS {
        return value;
    }
    value
        .chars()
        .take(MAX_TIMELINE_DETAIL_CHARS.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn add_token_usage(left: TokenUsage, right: TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: left.input_tokens.saturating_add(right.input_tokens),
        output_tokens: left.output_tokens.saturating_add(right.output_tokens),
        total_tokens: left.total_tokens.saturating_add(right.total_tokens),
    }
}

fn finish_tool_activity(
    tool_activities: &mut [ToolActivitySnapshot],
    turn_timeline: &mut [TurnTimelineItem],
    call_id: &str,
    event: &StoredEvent,
    terminal_state: ToolActivityState,
) {
    let belongs_to_turn = |activity: &ToolActivitySnapshot| {
        activity.call.id == call_id
            && event
                .turn_id
                .as_deref()
                .is_some_and(|turn_id| activity.turn_id == turn_id)
    };
    let finish = |activity: &mut ToolActivitySnapshot| {
        activity.state = terminal_state.clone();
        activity.completed_at_ms = Some(event.created_at_ms);
        activity.duration_ms = activity
            .started_at_ms
            .map(|started| event.created_at_ms.saturating_sub(started));
    };

    if let Some(activity) = tool_activities
        .iter_mut()
        .rev()
        .find(|activity| belongs_to_turn(activity))
    {
        finish(activity);
    }
    if let Some(TurnTimelineItem::Tool { activity }) = turn_timeline.iter_mut().rev().find(
        |item| matches!(item, TurnTimelineItem::Tool { activity } if belongs_to_turn(activity)),
    ) {
        finish(activity);
    }
}

fn approval_action_label(action: ApprovalAction) -> &'static str {
    match action {
        ApprovalAction::Approved => "已批准",
        ApprovalAction::Rejected => "已拒绝",
        ApprovalAction::TimedOut => "已超时",
        ApprovalAction::Cancelled => "已取消",
    }
}

fn user_input_action_label(action: UserInputAction) -> &'static str {
    match action {
        UserInputAction::Answered => "已回答",
        UserInputAction::Skipped => "已跳过",
        UserInputAction::Cancelled => "已取消",
    }
}

fn update_turn(
    last_turn: &mut Option<TurnSnapshot>,
    event: &StoredEvent,
    state: TurnState,
    error: Option<TurnError>,
) {
    if let Some(turn_id) = &event.turn_id {
        *last_turn = Some(TurnSnapshot {
            turn_id: turn_id.clone(),
            state,
            error,
        });
    }
}

pub(crate) fn title_from_message(message: &str) -> String {
    let message = message
        .split("\n\n[图片文字识别:")
        .next()
        .unwrap_or(message);
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
        MessageRole, PatchFilePreview, PatchPreview, ToolRisk, UserInputQuestion,
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

    async fn append_at(
        repository: &JsonlThreadRepository,
        thread_id: &str,
        turn_id: Option<&str>,
        created_at_ms: u64,
        kind: StoredEventKind,
    ) {
        let mut event = StoredEvent::new(thread_id, turn_id.map(str::to_string), kind);
        event.created_at_ms = created_at_ms;
        repository.append(event).await.unwrap();
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
        let mut user_message = message(MessageRole::User, "Explain this repository");
        user_message.content.push(ContentBlock::Context {
            text: "\n\n[图片文字识别: image.png]\nhidden text".into(),
        });
        repository
            .append(StoredEvent::new(
                &thread.id,
                None,
                StoredEventKind::UserMessage {
                    message: user_message,
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
    async fn concurrent_appends_keep_jsonl_and_incremental_projection_in_the_same_order() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let mut appends = Vec::new();
        for index in 0..24 {
            let repository = repository.clone();
            let thread_id = thread.id.clone();
            appends.push(tokio::spawn(async move {
                repository
                    .append(StoredEvent::new(
                        thread_id,
                        None,
                        StoredEventKind::ThreadRenamed {
                            title: format!("title-{index}"),
                        },
                    ))
                    .await
                    .unwrap();
            }));
        }
        for append in appends {
            append.await.unwrap();
        }

        let events = repository.load(&thread.id).await.unwrap();
        let jsonl_ids = events
            .iter()
            .map(|event| event.event_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            repository
                .projection()
                .indexed_event_ids(&thread.id)
                .unwrap(),
            jsonl_ids
        );
        assert_eq!(
            repository.list_threads().await.unwrap()[0].title,
            repository
                .read_thread(&thread.id)
                .await
                .unwrap()
                .summary
                .title
        );
    }

    #[tokio::test]
    async fn history_index_is_invalidated_by_append_and_rebuilt_on_demand() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        repository.read_thread_history(&thread.id).await.unwrap();
        assert!(
            repository
                .projection()
                .history_index_is_current(&thread.id)
                .unwrap()
        );

        repository
            .append(StoredEvent::new(
                &thread.id,
                None,
                StoredEventKind::UserMessage {
                    message: message(MessageRole::User, "indexed lazily"),
                },
            ))
            .await
            .unwrap();
        assert!(
            !repository
                .projection()
                .history_index_is_current(&thread.id)
                .unwrap()
        );

        let page = repository
            .list_thread_items(&thread.id, None, None, Some(10), HistorySortDirection::Asc)
            .await
            .unwrap();
        assert_eq!(page.data.len(), 1);
        assert!(
            repository
                .projection()
                .history_index_is_current(&thread.id)
                .unwrap()
        );
    }

    #[tokio::test]
    async fn persists_one_immutable_workspace_binding_per_thread() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let other_workspace = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(data.path()).unwrap();

        let thread = repository
            .create_thread_in_workspace(workspace.path())
            .await
            .unwrap();
        assert_eq!(
            thread.workspace_path.as_deref(),
            Some(workspace.path().to_string_lossy().as_ref())
        );
        assert_eq!(
            repository.list_threads().await.unwrap()[0]
                .workspace_path
                .as_deref(),
            thread.workspace_path.as_deref()
        );

        let error = repository
            .bind_thread_workspace(&thread.id, other_workspace.path())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("already bound"));
        assert_eq!(
            repository
                .read_thread(&thread.id)
                .await
                .unwrap()
                .summary
                .workspace_path,
            thread.workspace_path
        );
    }

    #[tokio::test]
    async fn persists_standalone_threads_and_rejects_workspace_binding() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(data.path()).unwrap();

        let thread = repository.create_standalone_thread().await.unwrap();
        assert!(!thread.in_project);
        assert!(thread.workspace_path.is_none());
        assert!(
            repository
                .bind_thread_workspace(&thread.id, workspace.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("standalone thread")
        );

        let listed = repository.list_threads().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].in_project);
        assert!(listed[0].workspace_path.is_none());

        let fork = repository.fork_thread(&thread.id, None).await.unwrap();
        assert!(!fork.in_project);
        assert!(fork.workspace_path.is_none());
    }

    #[tokio::test]
    async fn serializes_concurrent_workspace_binding_attempts() {
        let data = tempfile::tempdir().unwrap();
        let first_workspace = tempfile::tempdir().unwrap();
        let second_workspace = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(data.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();

        let first_repository = repository.clone();
        let first_thread_id = thread.id.clone();
        let first_path = first_workspace.path().to_path_buf();
        let second_repository = repository.clone();
        let second_thread_id = thread.id.clone();
        let second_path = second_workspace.path().to_path_buf();
        let (first, second) = tokio::join!(
            async move {
                first_repository
                    .bind_thread_workspace(&first_thread_id, &first_path)
                    .await
            },
            async move {
                second_repository
                    .bind_thread_workspace(&second_thread_id, &second_path)
                    .await
            }
        );

        assert_ne!(first.is_ok(), second.is_ok());
        let events = repository.load(&thread.id).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind, StoredEventKind::ThreadWorkspaceBound { .. }))
                .count(),
            1
        );
        assert!(repository.read_thread(&thread.id).await.is_ok());
    }

    #[tokio::test]
    async fn projects_assistant_message_turn_ids_for_inline_activity() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let assistant = message(MessageRole::Assistant, "done");

        repository
            .append(StoredEvent::new(
                &thread.id,
                Some("turn-inline".to_string()),
                StoredEventKind::AssistantMessage {
                    message: assistant.clone(),
                },
            ))
            .await
            .unwrap();

        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert_eq!(
            detail
                .message_turn_ids
                .get(&assistant.id)
                .map(String::as_str),
            Some("turn-inline")
        );
    }

    #[tokio::test]
    async fn projects_retry_attempts_to_the_same_user_message() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let user = message(MessageRole::User, "fix the issue");
        let user_id = user.id.clone();

        repository
            .append(StoredEvent::new(
                &thread.id,
                None,
                StoredEventKind::UserMessage { message: user },
            ))
            .await
            .unwrap();
        for turn_id in ["turn-first", "turn-retry"] {
            repository
                .append(StoredEvent::new(
                    &thread.id,
                    Some(turn_id.to_string()),
                    StoredEventKind::TurnStarted,
                ))
                .await
                .unwrap();
        }

        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert_eq!(
            detail.turn_user_message_ids.get("turn-first"),
            Some(&user_id)
        );
        assert_eq!(
            detail.turn_user_message_ids.get("turn-retry"),
            Some(&user_id)
        );
    }

    #[tokio::test]
    async fn projects_interleaved_turn_timeline_in_event_order() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let call = ToolCall {
            id: "call-read".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "path": "README.md" }),
            metadata: serde_json::json!({}),
        };

        repository
            .append(StoredEvent::new(
                &thread.id,
                Some("turn-timeline".into()),
                StoredEventKind::AssistantToolCalls {
                    item_id: Some("agent-message-progress".into()),
                    text: "我先读取说明文件。".into(),
                    calls: vec![call.clone()],
                },
            ))
            .await
            .unwrap();
        repository
            .append(StoredEvent::new(
                &thread.id,
                Some("turn-timeline".into()),
                StoredEventKind::ToolResult {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    result: ToolResult {
                        success: true,
                        output: "docs".into(),
                        metadata: serde_json::json!({}),
                    },
                },
            ))
            .await
            .unwrap();
        repository
            .append(StoredEvent::new(
                &thread.id,
                Some("turn-timeline".into()),
                StoredEventKind::AssistantMessage {
                    message: message(MessageRole::Assistant, "读取完成。"),
                },
            ))
            .await
            .unwrap();

        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.turn_timeline.len(), 3);
        assert!(matches!(
            &detail.turn_timeline[0],
            TurnTimelineItem::Text { id, text, .. }
                if id == "agent-message-progress" && text == "我先读取说明文件。"
        ));
        assert!(matches!(
            &detail.turn_timeline[1],
            TurnTimelineItem::Tool { activity }
                if activity.call.id == "call-read"
                    && activity.state == ToolActivityState::Completed
        ));
        assert!(matches!(
            &detail.turn_timeline[2],
            TurnTimelineItem::Text { text, .. } if text == "读取完成。"
        ));
    }

    #[tokio::test]
    async fn projects_completed_reasoning_summaries_without_provider_context() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        repository
            .append(StoredEvent::new(
                &thread.id,
                Some("turn-reasoning".into()),
                StoredEventKind::ReasoningSummary {
                    item_id: "rs_1".into(),
                    summary: "Checked the public API contract.".into(),
                },
            ))
            .await
            .unwrap();

        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert!(matches!(
            &detail.turn_timeline[0],
            TurnTimelineItem::Reasoning { item_id, summary, .. }
                if item_id == "rs_1" && summary == "Checked the public API contract."
        ));
        assert!(detail.messages.is_empty());
    }

    #[test]
    fn legacy_tool_call_events_default_to_empty_progress_text() {
        let event: StoredEventKind = serde_json::from_value(serde_json::json!({
            "type": "assistant_tool_calls",
            "data": { "calls": [] }
        }))
        .unwrap();
        assert!(matches!(
            event,
            StoredEventKind::AssistantToolCalls {
                item_id: None,
                text,
                calls,
            } if text.is_empty() && calls.is_empty()
        ));
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
            auto_approved: false,
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
        assert!(matches!(
            &detail.turn_timeline[..],
            [
                TurnTimelineItem::Event {
                    kind: TimelineEventKind::ApprovalRequested,
                    ..
                },
                TurnTimelineItem::Event {
                    kind: TimelineEventKind::ApprovalResolved,
                    ..
                },
                TurnTimelineItem::Event {
                    kind: TimelineEventKind::ChangeApplied,
                    title: change_title,
                    ..
                },
                TurnTimelineItem::Event {
                    kind: TimelineEventKind::ChangeUndone,
                    ..
                },
            ] if change_title == "编辑了文件"
        ));
    }

    #[tokio::test]
    async fn rebuilds_pending_user_input_and_terminal_timeline_events() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let turn_id = "turn-input".to_string();
        let request = UserInputRequest {
            id: "input-1".into(),
            thread_id: thread.id.clone(),
            turn_id: turn_id.clone(),
            tool_call_id: "call-input".into(),
            kind: crate::protocol::UserInputRequestKind::ModelQuestion,
            questions: vec![UserInputQuestion {
                question: "选择实现方式".into(),
                options: vec!["稳妥".into(), "快速".into()],
            }],
            created_at_ms: now_ms(),
            expires_at_ms: now_ms() + 60_000,
        };
        for (index, kind) in [
            StoredEventKind::UserMessage {
                message: message(MessageRole::User, "先规划"),
            },
            StoredEventKind::TurnStarted,
            StoredEventKind::UserInputRequested {
                request: request.clone(),
            },
            StoredEventKind::TurnCancelled,
        ]
        .into_iter()
        .enumerate()
        {
            let mut event = StoredEvent::new(&thread.id, Some(turn_id.clone()), kind);
            event.created_at_ms = 10_000 + index as u64 * 25;
            repository.append(event).await.unwrap();
        }

        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.user_inputs.len(), 1);
        assert_eq!(detail.user_inputs[0].request, request);
        assert!(detail.user_inputs[0].resolution.is_none());
        assert_eq!(
            detail.turn_user_message_ids.get(&turn_id),
            detail.messages.first().map(|message| &message.id)
        );
        assert!(matches!(
            &detail.turn_timeline[..],
            [
                TurnTimelineItem::Event {
                    kind: TimelineEventKind::UserInputRequested,
                    ..
                },
                TurnTimelineItem::Event {
                    kind: TimelineEventKind::TurnCancelled,
                    duration_ms: Some(50),
                    ..
                },
            ]
        ));
    }

    #[tokio::test]
    async fn cancelled_turn_finishes_running_tools_during_timeline_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let turn_id = "turn-cancelled-tools".to_string();
        let calls = ["build", "version"]
            .into_iter()
            .map(|id| ToolCall {
                id: id.to_string(),
                name: "run_command".to_string(),
                arguments: serde_json::json!({ "program": id }),
                metadata: serde_json::json!({}),
            })
            .collect::<Vec<_>>();
        for (index, kind) in [
            StoredEventKind::TurnStarted,
            StoredEventKind::AssistantToolCalls {
                item_id: None,
                text: String::new(),
                calls,
            },
            StoredEventKind::ToolStarted {
                call_id: "build".to_string(),
            },
            StoredEventKind::TurnCancelled,
        ]
        .into_iter()
        .enumerate()
        {
            let mut event = StoredEvent::new(&thread.id, Some(turn_id.clone()), kind);
            event.created_at_ms = 20_000 + index as u64 * 25;
            repository.append(event).await.unwrap();
        }

        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.tool_activities.len(), 2);
        assert_eq!(
            detail.tool_activities[0].state,
            ToolActivityState::Cancelled
        );
        assert_eq!(detail.tool_activities[0].completed_at_ms, Some(20_075));
        assert_eq!(detail.tool_activities[0].duration_ms, Some(25));
        assert_eq!(
            detail.tool_activities[1].state,
            ToolActivityState::Cancelled
        );
        assert_eq!(detail.tool_activities[1].completed_at_ms, Some(20_075));
        assert_eq!(detail.tool_activities[1].duration_ms, None);
        assert!(matches!(
            detail.turn_timeline.last(),
            Some(TurnTimelineItem::Event {
                kind: TimelineEventKind::TurnCancelled,
                duration_ms: Some(75),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn serial_tool_recovery_distinguishes_running_and_pending_calls() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let turn_id = "turn-serial-tools".to_string();
        let calls = ["first", "second"]
            .into_iter()
            .map(|id| ToolCall {
                id: id.to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({ "path": id }),
                metadata: serde_json::json!({}),
            })
            .collect::<Vec<_>>();
        for (index, kind) in [
            StoredEventKind::TurnStarted,
            StoredEventKind::AssistantToolCalls {
                item_id: None,
                text: String::new(),
                calls,
            },
            StoredEventKind::ToolStarted {
                call_id: "first".to_string(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let mut event = StoredEvent::new(&thread.id, Some(turn_id.clone()), kind);
            event.created_at_ms = 30_000 + index as u64 * 25;
            repository.append(event).await.unwrap();
        }

        let detail = repository.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.tool_activities[0].state, ToolActivityState::Running);
        assert_eq!(detail.tool_activities[0].started_at_ms, Some(30_050));
        assert_eq!(detail.tool_activities[1].state, ToolActivityState::Pending);
        assert_eq!(detail.tool_activities[1].started_at_ms, None);
    }

    #[tokio::test]
    async fn projects_one_thread_item_history_without_rewriting_legacy_events() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let turn_id = "turn-legacy-history";
        let base = now_ms() + 100;
        let user = message(MessageRole::User, "inspect history");
        let assistant = message(MessageRole::Assistant, "history is consistent");
        let call = ToolCall {
            id: "call-history".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "path": "README.md" }),
            metadata: serde_json::json!({}),
        };
        let approval = ApprovalRequest {
            id: "approval-history".into(),
            thread_id: thread.id.clone(),
            turn_id: turn_id.into(),
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            reason: "legacy approval".into(),
            auto_approved: false,
            risk: ToolRisk::Read,
            arguments: call.arguments.clone(),
            preview: None,
            created_at_ms: base + 30,
            expires_at_ms: base + 60_000,
        };
        let resolution = ApprovalResolution {
            action: ApprovalAction::Approved,
            patch: None,
            selected_paths: Vec::new(),
            expected_hashes: Vec::new(),
        };

        for (offset, kind) in [
            StoredEventKind::UserMessage { message: user },
            StoredEventKind::AssistantToolCalls {
                item_id: Some("commentary-history".into()),
                text: "checking the file".into(),
                calls: vec![call.clone()],
            },
            StoredEventKind::ApprovalRequested {
                request: approval.clone(),
            },
            StoredEventKind::ApprovalResolved {
                request_id: approval.id.clone(),
                resolution,
            },
            StoredEventKind::ToolStarted {
                call_id: call.id.clone(),
            },
            StoredEventKind::ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                result: ToolResult {
                    success: true,
                    output: "read".into(),
                    metadata: serde_json::json!({}),
                },
            },
            StoredEventKind::AssistantMessage { message: assistant },
            StoredEventKind::TurnCompleted { usage: None },
        ]
        .into_iter()
        .enumerate()
        {
            append_at(
                &repository,
                &thread.id,
                Some(turn_id),
                base + offset as u64 * 10,
                kind,
            )
            .await;
        }

        let event_count = repository.load(&thread.id).await.unwrap().len();
        let history = repository.read_thread_history(&thread.id).await.unwrap();
        assert_eq!(
            repository.load(&thread.id).await.unwrap().len(),
            event_count
        );
        assert_eq!(history.turns.data.len(), 1);
        let turn = &history.turns.data[0];
        assert_eq!(turn.id, turn_id);
        assert_eq!(turn.state, TurnState::Completed);
        assert_eq!(turn.items_view, TurnItemsView::Full);
        assert!(
            matches!(
                &turn.items[..],
                [
                    ThreadItem { payload: ThreadItemPayload::UserMessage { .. }, .. },
                    ThreadItem { payload: ThreadItemPayload::AgentMessage { phase: AgentMessagePhase::Commentary, .. }, .. },
                    ThreadItem { payload: ThreadItemPayload::Approval { .. }, completed_at_ms: Some(completed), .. },
                    ThreadItem { payload: ThreadItemPayload::Tool { .. }, completed_at_ms: Some(tool_completed), .. },
                    ThreadItem { payload: ThreadItemPayload::AgentMessage { phase: AgentMessagePhase::FinalAnswer, .. }, .. },
                    ThreadItem { payload: ThreadItemPayload::Event, .. },
                ] if *completed == base + 30 && *tool_completed == base + 50
            ),
            "{:#?}",
            turn.items
        );
        let timeline = turn
            .items
            .iter()
            .flat_map(|item| item.timeline_items.iter())
            .map(timeline_item_id)
            .collect::<Vec<_>>();
        assert_eq!(
            timeline,
            vec![
                "commentary-history",
                "approval-requested-approval-history",
                "approval-resolved-approval-history",
                "call-history",
                turn.items[4].id.as_str(),
                "turn-completed-turn-legacy-history",
            ]
        );
        let json = serde_json::to_value(history).unwrap();
        assert_eq!(json["turns"]["data"][0]["itemsView"], "full");
        assert_eq!(json["turns"]["data"][0]["items"][2]["type"], "approval");
    }

    #[tokio::test]
    async fn paginates_thread_turns_and_items_with_bound_query_cursors() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let base = now_ms() + 100;

        for index in 1..=4u64 {
            let turn_id = format!("turn-{index}");
            append_at(
                &repository,
                &thread.id,
                None,
                base + index * 100,
                StoredEventKind::UserMessage {
                    message: message(MessageRole::User, &format!("question {index}")),
                },
            )
            .await;
            append_at(
                &repository,
                &thread.id,
                Some(&turn_id),
                base + index * 100 + 10,
                StoredEventKind::TurnStarted,
            )
            .await;
            append_at(
                &repository,
                &thread.id,
                Some(&turn_id),
                base + index * 100 + 20,
                StoredEventKind::AssistantMessage {
                    message: message(MessageRole::Assistant, &format!("answer {index}")),
                },
            )
            .await;
            append_at(
                &repository,
                &thread.id,
                Some(&turn_id),
                base + index * 100 + 30,
                StoredEventKind::TurnCompleted { usage: None },
            )
            .await;
        }

        let first = repository
            .list_thread_turns(
                &thread.id,
                None,
                Some(2),
                HistorySortDirection::Desc,
                TurnItemsView::Summary,
            )
            .await
            .unwrap();
        assert_eq!(
            first
                .data
                .iter()
                .map(|turn| turn.id.as_str())
                .collect::<Vec<_>>(),
            vec!["turn-4", "turn-3"]
        );
        assert!(first.data.iter().all(|turn| {
            turn.items_view == TurnItemsView::Summary
                && turn.items.len() == 2
                && turn.items.iter().all(|item| {
                    matches!(
                        &item.payload,
                        ThreadItemPayload::UserMessage { .. }
                            | ThreadItemPayload::AgentMessage {
                                phase: AgentMessagePhase::FinalAnswer,
                                ..
                            }
                    )
                })
        }));
        let second = repository
            .list_thread_turns(
                &thread.id,
                first.next_cursor.as_deref(),
                Some(2),
                HistorySortDirection::Desc,
                TurnItemsView::NotLoaded,
            )
            .await
            .unwrap();
        assert_eq!(
            second
                .data
                .iter()
                .map(|turn| turn.id.as_str())
                .collect::<Vec<_>>(),
            vec!["turn-2", "turn-1"]
        );
        assert!(second.data.iter().all(|turn| turn.items.is_empty()));

        append_at(
            &repository,
            &thread.id,
            Some("turn-5"),
            base + 510,
            StoredEventKind::TurnStarted,
        )
        .await;
        let newer = repository
            .list_thread_turns(
                &thread.id,
                first.backwards_cursor.as_deref(),
                Some(10),
                HistorySortDirection::Asc,
                TurnItemsView::NotLoaded,
            )
            .await
            .unwrap();
        assert_eq!(
            newer
                .data
                .iter()
                .map(|turn| turn.id.as_str())
                .collect::<Vec<_>>(),
            vec!["turn-4", "turn-5"]
        );

        let turn_items = repository
            .list_thread_items(
                &thread.id,
                Some("turn-1"),
                None,
                Some(1),
                HistorySortDirection::Asc,
            )
            .await
            .unwrap();
        assert!(matches!(
            turn_items.data[0].item.payload,
            ThreadItemPayload::UserMessage { .. }
        ));
        assert!(turn_items.next_cursor.is_some());
        assert!(matches!(
            repository
                .list_thread_items(
                    &thread.id,
                    Some("turn-2"),
                    turn_items.next_cursor.as_deref(),
                    Some(1),
                    HistorySortDirection::Asc,
                )
                .await,
            Err(StorageError::InvalidData(_))
        ));
        assert!(matches!(
            repository
                .list_thread_items(
                    &thread.id,
                    None,
                    first.next_cursor.as_deref(),
                    Some(1),
                    HistorySortDirection::Asc,
                )
                .await,
            Err(StorageError::InvalidData(_))
        ));
        assert!(matches!(
            repository
                .list_thread_turns(
                    &thread.id,
                    None,
                    Some(101),
                    HistorySortDirection::Desc,
                    TurnItemsView::Full,
                )
                .await,
            Err(StorageError::InvalidData(_))
        ));
    }

    #[tokio::test]
    async fn migrates_v1_events_and_rebuilds_sqlite_from_jsonl() {
        let directory = tempfile::tempdir().unwrap();
        let thread_id = Uuid::new_v4().to_string();
        let sessions = directory.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let event = StoredEvent::new(
            &thread_id,
            None,
            StoredEventKind::ThreadCreated {
                title: "legacy".into(),
                in_project: true,
            },
        );
        let mut value = serde_json::to_value(event).unwrap();
        value["schemaVersion"] = serde_json::json!(1);
        value["data"].as_object_mut().unwrap().remove("in_project");
        fs::write(
            sessions.join(format!("{thread_id}.jsonl")),
            format!("{}\n", value),
        )
        .unwrap();

        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        assert_eq!(
            repository.load(&thread_id).await.unwrap()[0].schema_version,
            EVENT_SCHEMA_VERSION
        );
        let migrated = &repository.list_threads().await.unwrap()[0];
        assert_eq!(migrated.title, "legacy");
        assert!(migrated.in_project);
        drop(repository);
        fs::remove_file(directory.path().join("k-coder.db")).unwrap();
        let rebuilt = JsonlThreadRepository::new(directory.path()).unwrap();
        assert_eq!(rebuilt.list_threads().await.unwrap()[0].id, thread_id);
    }

    #[tokio::test]
    async fn rollback_truncates_effective_history_and_keeps_an_audit_marker() {
        let directory = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository.create_thread().await.unwrap();
        let base = now_ms() + 100;

        for index in 1..=2u64 {
            let turn_id = format!("turn-{index}");
            append_at(
                &repository,
                &thread.id,
                None,
                base + index * 100,
                StoredEventKind::UserMessage {
                    message: message(MessageRole::User, &format!("question {index}")),
                },
            )
            .await;
            append_at(
                &repository,
                &thread.id,
                Some(&turn_id),
                base + index * 100 + 10,
                StoredEventKind::TurnStarted,
            )
            .await;
            append_at(
                &repository,
                &thread.id,
                Some(&turn_id),
                base + index * 100 + 20,
                StoredEventKind::AssistantMessage {
                    message: message(MessageRole::Assistant, &format!("answer {index}")),
                },
            )
            .await;
            append_at(
                &repository,
                &thread.id,
                Some(&turn_id),
                base + index * 100 + 30,
                StoredEventKind::TurnCompleted { usage: None },
            )
            .await;
        }

        let rolled_back = repository.rollback_thread(&thread.id, 1).await.unwrap();
        assert_eq!(rolled_back.turns.data.len(), 1);
        assert_eq!(rolled_back.turns.data[0].id, "turn-1");
        assert_eq!(rolled_back.summary.title, "question 1");
        assert!(
            repository
                .load(&thread.id)
                .await
                .unwrap()
                .iter()
                .any(|event| {
                    matches!(
                        event.kind,
                        StoredEventKind::ThreadRolledBack { num_turns: 1, .. }
                    )
                })
        );

        let rolled_back_again = repository.rollback_thread(&thread.id, 1).await.unwrap();
        assert!(rolled_back_again.turns.data.is_empty());

        drop(repository);
        let restored = JsonlThreadRepository::new(directory.path()).unwrap();
        assert!(
            restored
                .read_thread_history(&thread.id)
                .await
                .unwrap()
                .turns
                .data
                .is_empty()
        );
    }

    #[tokio::test]
    async fn fork_copies_only_the_selected_completed_history() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let repository = JsonlThreadRepository::new(directory.path()).unwrap();
        let thread = repository
            .create_thread_in_workspace(workspace.path())
            .await
            .unwrap();
        for index in 1..=2u64 {
            let turn_id = format!("turn-{index}");
            for kind in [
                StoredEventKind::UserMessage {
                    message: message(MessageRole::User, &format!("question {index}")),
                },
                StoredEventKind::TurnStarted,
                StoredEventKind::AssistantMessage {
                    message: message(MessageRole::Assistant, &format!("answer {index}")),
                },
                StoredEventKind::TurnCompleted { usage: None },
            ] {
                repository
                    .append(StoredEvent::new(
                        &thread.id,
                        (!matches!(kind, StoredEventKind::UserMessage { .. }))
                            .then_some(turn_id.clone()),
                        kind,
                    ))
                    .await
                    .unwrap();
            }
        }

        let fork = repository
            .fork_thread(&thread.id, Some("turn-1"))
            .await
            .unwrap();
        assert_ne!(fork.id, thread.id);
        assert_eq!(fork.workspace_path, thread.workspace_path);
        assert!(fork.title.ends_with("(分支)"));
        let history = repository.read_thread_history(&fork.id).await.unwrap();
        assert_eq!(history.turns.data.len(), 1);
        assert_eq!(history.turns.data[0].id, "turn-1");
        assert!(
            repository
                .load(&fork.id)
                .await
                .unwrap()
                .iter()
                .any(|event| {
                    matches!(
                        &event.kind,
                        StoredEventKind::ThreadForked { source_thread_id, .. }
                            if source_thread_id == &thread.id
                    )
                })
        );

        repository.rollback_thread(&thread.id, 1).await.unwrap();
        let fork_after_rollback = repository.fork_thread(&thread.id, None).await.unwrap();
        let forked_history = repository
            .read_thread_history(&fork_after_rollback.id)
            .await
            .unwrap();
        assert_eq!(forked_history.turns.data.len(), 1);
        assert_eq!(forked_history.turns.data[0].id, "turn-1");
    }
}
