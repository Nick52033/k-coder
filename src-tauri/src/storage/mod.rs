use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::context::CompactionSummary;
use crate::persistence::ProjectionDb;
use crate::protocol::{
    AgentMode, ApprovalAction, ApprovalRequest, ApprovalResolution, ApprovalSnapshot, ChangeSet,
    ChatMessage, MessageRole, PROTOCOL_VERSION, TodoItem, TokenUsage, ToolCall, ToolResult,
    TurnState, UserInputAction, UserInputRequest, UserInputResolution,
};

pub const EVENT_SCHEMA_VERSION: u32 = 4;
const MAX_TIMELINE_DETAIL_CHARS: usize = 2_000;

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
    },
    UserMessage {
        message: ChatMessage,
    },
    TurnModeSelected {
        mode: AgentMode,
    },
    TurnStarted,
    AssistantMessage {
        message: ChatMessage,
    },
    AssistantToolCalls {
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
    append_lock: Arc<Mutex<()>>,
    projection: ProjectionDb,
}

impl JsonlThreadRepository {
    pub fn new(data_root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let sessions_dir = data_root.as_ref().join("sessions");
        fs::create_dir_all(&sessions_dir).map_err(|error| StorageError::Io(error.to_string()))?;
        let repository = Self {
            sessions_dir,
            append_lock: Arc::new(Mutex::new(())),
            projection: ProjectionDb::open(data_root.as_ref())
                .map_err(|error| StorageError::Io(error.to_string()))?,
        };
        repository.rebuild_projection()?;
        Ok(repository)
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
        self.projection
            .list_threads()
            .map_err(|error| StorageError::Io(error.to_string()))
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
        .map_err(|error| StorageError::Io(error.to_string()))??;
        let events = self.load(&event.thread_id).await?;
        let detail = project_thread(&event.thread_id, &events)?;
        self.projection
            .replace_thread(&detail.summary, &events)
            .map_err(|error| StorageError::Io(error.to_string()))?;
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
    let mut message_turn_ids = HashMap::new();
    let mut turn_user_message_ids = HashMap::new();
    let mut latest_user_message_id: Option<String> = None;
    let mut archived = false;
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
            StoredEventKind::AssistantToolCalls { text, calls } => {
                if let Some(turn_id) = &event.turn_id {
                    if !text.is_empty() {
                        turn_timeline.push(TurnTimelineItem::Text {
                            id: event.event_id.clone(),
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
                        "压缩了 {} 条历史消息，保留 {} 项用户约束和 {} 项近期工具结果",
                        summary.compacted_message_count,
                        summary.user_constraints.len(),
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
            StoredEventKind::TurnFailed { message } => {
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
                    Some(message.clone()),
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
            TurnTimelineItem::Text { text, .. } if text == "我先读取说明文件。"
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
            StoredEventKind::AssistantToolCalls { text, calls }
                if text.is_empty() && calls.is_empty()
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
            },
        );
        let mut value = serde_json::to_value(event).unwrap();
        value["schemaVersion"] = serde_json::json!(1);
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
        assert_eq!(repository.list_threads().await.unwrap()[0].title, "legacy");
        drop(repository);
        fs::remove_file(directory.path().join("k-coder.db")).unwrap();
        let rebuilt = JsonlThreadRepository::new(directory.path()).unwrap();
        assert_eq!(rebuilt.list_threads().await.unwrap()[0].id, thread_id);
    }
}
