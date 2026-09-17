use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};

use crate::protocol::{
    HistorySortDirection, ThreadItem, ThreadTurn, TodoItem, TokenUsage, TokenUsageDetails,
};
use crate::storage::{StoredEvent, StoredEventKind, ThreadSummary, TurnSnapshot};

pub const DATABASE_SCHEMA_VERSION: u32 = 11;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub path: String,
    pub trusted: bool,
    pub last_opened_at_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub schema_version: u32,
    pub trend_days: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub provider_calls: u64,
    pub cached_input_tokens: Option<u64>,
    pub uncached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    pub reply_output_tokens: Option<u64>,
    pub cache_hit_rate: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    pub daily: Vec<DailyUsageSummary>,
    pub models: Vec<ModelUsageSummary>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsageSummary {
    pub date: String,
    pub provider_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageSummary {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub provider_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub uncached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    pub reply_output_tokens: Option<u64>,
    pub cache_hit_rate: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct UsageAggregate {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    provider_calls: u64,
    cached_input_tokens: Option<u64>,
    uncached_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
}

impl UsageAggregate {
    fn reply_output_tokens(self) -> Option<u64> {
        self.reasoning_output_tokens
            .map(|reasoning| self.output_tokens.saturating_sub(reasoning))
    }

    fn cache_hit_rate(self) -> Option<f64> {
        let cached = self.cached_input_tokens?;
        (self.input_tokens > 0).then_some(cached as f64 / self.input_tokens as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryIndexOrder {
    pub event_index: u64,
    pub item_index: u32,
}

#[derive(Debug, Clone)]
pub struct HistoryIndexTurn {
    pub order: HistoryIndexOrder,
    pub turn: ThreadTurn,
}

#[derive(Debug, Clone)]
pub struct HistoryIndexItem {
    pub order: HistoryIndexOrder,
    pub item: ThreadItem,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryIndexMetadata {
    pub summary: ThreadSummary,
    pub last_turn: Option<TurnSnapshot>,
    pub todos: Vec<TodoItem>,
    pub last_usage: Option<TokenUsage>,
    #[serde(default)]
    pub context_usage: Option<TokenUsage>,
    pub unscoped_items: Vec<ThreadItem>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    #[error("projection database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("projection lock was poisoned")]
    Poisoned,
    #[error("projection data is invalid: {0}")]
    InvalidData(String),
}

#[derive(Debug, Clone)]
pub struct ProjectionDb {
    connection: Arc<Mutex<Connection>>,
    data_root: Option<PathBuf>,
}

impl ProjectionDb {
    pub fn open(data_root: &Path) -> Result<Self, ProjectionError> {
        std::fs::create_dir_all(data_root).map_err(|error| {
            ProjectionError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
        })?;
        let connection = Connection::open(data_root.join("k-coder.db"))?;
        migrate(&connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            data_root: Some(data_root.to_path_buf()),
        })
    }

    pub(crate) fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, ProjectionError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        operation(&mut connection).map_err(ProjectionError::Database)
    }

    #[cfg(test)]
    pub fn memory() -> Result<Self, ProjectionError> {
        let connection = Connection::open_in_memory()?;
        migrate(&connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            data_root: None,
        })
    }

    pub(crate) fn data_root(&self) -> Option<PathBuf> {
        self.data_root.clone()
    }

    pub fn replace_thread(
        &self,
        summary: &ThreadSummary,
        events: &[StoredEvent],
    ) -> Result<(), ProjectionError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO sessions(id,title,created_at_ms,updated_at_ms,archived,event_count,workspace_path,in_project)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(id) DO UPDATE SET title=excluded.title,updated_at_ms=excluded.updated_at_ms,
             archived=excluded.archived,event_count=excluded.event_count,workspace_path=excluded.workspace_path,
             in_project=excluded.in_project",
            params![summary.id, summary.title, summary.created_at_ms, summary.updated_at_ms,
                summary.archived as i64, events.len() as u64, summary.workspace_path,
                summary.in_project as i64],
        )?;
        transaction.execute("DELETE FROM usage WHERE thread_id=?1", [&summary.id])?;
        transaction.execute(
            "DELETE FROM indexed_events WHERE thread_id=?1",
            [&summary.id],
        )?;
        transaction.execute(
            "DELETE FROM history_turns WHERE thread_id=?1",
            [&summary.id],
        )?;
        transaction.execute(
            "DELETE FROM history_items WHERE thread_id=?1",
            [&summary.id],
        )?;
        transaction.execute(
            "DELETE FROM history_state WHERE thread_id=?1",
            [&summary.id],
        )?;
        for (sequence, event) in events.iter().enumerate() {
            insert_indexed_event(&transaction, sequence as u64, event)?;
            if let StoredEventKind::ProviderCallUsage {
                call_index,
                usage,
                details,
                provider,
                model,
            } = &event.kind
                && let Some(turn_id) = event.turn_id.as_deref()
            {
                insert_usage(
                    &transaction,
                    &summary.id,
                    turn_id,
                    *call_index,
                    event.created_at_ms,
                    *usage,
                    *details,
                    provider.as_deref(),
                    model.as_deref(),
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn append_event(&self, event: &StoredEvent) -> Result<(), ProjectionError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let transaction = connection.transaction()?;
        match &event.kind {
            StoredEventKind::ThreadCreated { title, in_project } => {
                transaction.execute(
                    "INSERT INTO sessions(id,title,created_at_ms,updated_at_ms,archived,event_count,workspace_path,in_project)
                     VALUES(?1,?2,?3,?3,0,1,NULL,?4)",
                    params![event.thread_id, title, event.created_at_ms, *in_project as i64],
                )?;
            }
            kind => {
                let automatic_title = if let StoredEventKind::UserMessage { message } = kind {
                    let title_is_owned: i64 = transaction.query_row(
                        "SELECT EXISTS(
                           SELECT 1 FROM indexed_events
                           WHERE thread_id=?1
                             AND json_extract(event_json,'$.type') IN ('user_message','thread_renamed'))",
                        [&event.thread_id],
                        |row| row.get(0),
                    )?;
                    (title_is_owned == 0 && message.role == crate::protocol::MessageRole::User)
                        .then(|| crate::storage::title_from_message(&message.visible_text()))
                } else {
                    None
                };
                let (title, archived, workspace_path): (Option<&str>, Option<i64>, Option<&str>) =
                    match kind {
                        StoredEventKind::ThreadRenamed { title } => (Some(title), None, None),
                        StoredEventKind::ThreadArchived | StoredEventKind::ThreadDeleted => {
                            (None, Some(1), None)
                        }
                        StoredEventKind::ThreadWorkspaceBound { path } => (None, None, Some(path)),
                        _ => (automatic_title.as_deref(), None, None),
                    };
                let changed = transaction.execute(
                    "UPDATE sessions SET
                       title=COALESCE(?2,title),
                       updated_at_ms=MAX(updated_at_ms,?3),
                       archived=COALESCE(?4,archived),
                       event_count=event_count+1,
                       workspace_path=COALESCE(?5,workspace_path)
                     WHERE id=?1",
                    params![
                        event.thread_id,
                        title,
                        event.created_at_ms,
                        archived,
                        workspace_path
                    ],
                )?;
                if changed != 1 {
                    return Err(ProjectionError::InvalidData(format!(
                        "session {} is missing from the projection",
                        event.thread_id
                    )));
                }
            }
        }
        let sequence: u64 = transaction.query_row(
            "SELECT COALESCE(MAX(sequence)+1,0) FROM indexed_events WHERE thread_id=?1",
            [&event.thread_id],
            |row| row.get(0),
        )?;
        insert_indexed_event(&transaction, sequence, event)?;
        if let StoredEventKind::ProviderCallUsage {
            call_index,
            usage,
            details,
            provider,
            model,
        } = &event.kind
            && let Some(turn_id) = event.turn_id.as_deref()
        {
            insert_usage(
                &transaction,
                &event.thread_id,
                turn_id,
                *call_index,
                event.created_at_ms,
                *usage,
                *details,
                provider.as_deref(),
                model.as_deref(),
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn history_index_is_current(&self, thread_id: &str) -> Result<bool, ProjectionError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        Ok(connection
            .query_row(
                "SELECT s.event_count=h.indexed_event_count
                 FROM sessions s JOIN history_state h ON h.thread_id=s.id WHERE s.id=?1",
                [thread_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some_and(|current| current != 0))
    }

    pub fn replace_history_index(
        &self,
        thread_id: &str,
        event_count: u64,
        metadata: &HistoryIndexMetadata,
        turns: &[HistoryIndexTurn],
        items: &[HistoryIndexItem],
    ) -> Result<(), ProjectionError> {
        let metadata_json = serde_json::to_string(metadata)
            .map_err(|error| ProjectionError::InvalidData(error.to_string()))?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM history_turns WHERE thread_id=?1", [thread_id])?;
        transaction.execute("DELETE FROM history_items WHERE thread_id=?1", [thread_id])?;
        for turn in turns {
            transaction.execute(
                "INSERT INTO history_turns(thread_id,event_index,item_index,turn_id,turn_json)
                 VALUES(?1,?2,?3,?4,?5)",
                params![
                    thread_id,
                    turn.order.event_index,
                    turn.order.item_index,
                    turn.turn.id,
                    serde_json::to_string(&turn.turn)
                        .map_err(|error| ProjectionError::InvalidData(error.to_string()))?
                ],
            )?;
        }
        for item in items {
            transaction.execute(
                "INSERT INTO history_items(thread_id,event_index,item_index,turn_id,item_id,item_json)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    thread_id,
                    item.order.event_index,
                    item.order.item_index,
                    item.item.turn_id,
                    item.item.id,
                    serde_json::to_string(&item.item)
                        .map_err(|error| ProjectionError::InvalidData(error.to_string()))?
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO history_state(thread_id,indexed_event_count,metadata_json)
             VALUES(?1,?2,?3)
             ON CONFLICT(thread_id) DO UPDATE SET
               indexed_event_count=excluded.indexed_event_count,
               metadata_json=excluded.metadata_json",
            params![thread_id, event_count, metadata_json],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn history_metadata(
        &self,
        thread_id: &str,
    ) -> Result<Option<HistoryIndexMetadata>, ProjectionError> {
        let json = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?
            .query_row(
                "SELECT metadata_json FROM history_state WHERE thread_id=?1",
                [thread_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        json.map(|json| {
            serde_json::from_str(&json)
                .map_err(|error| ProjectionError::InvalidData(error.to_string()))
        })
        .transpose()
    }

    pub fn history_turn_exists(
        &self,
        thread_id: &str,
        order: HistoryIndexOrder,
        turn_id: &str,
    ) -> Result<bool, ProjectionError> {
        Ok(self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?
            .query_row(
                "SELECT 1 FROM history_turns
                 WHERE thread_id=?1 AND event_index=?2 AND item_index=?3 AND turn_id=?4",
                params![thread_id, order.event_index, order.item_index, turn_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn history_item_exists(
        &self,
        thread_id: &str,
        order: HistoryIndexOrder,
        item_id: &str,
        turn_id: Option<&str>,
    ) -> Result<bool, ProjectionError> {
        Ok(self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?
            .query_row(
                "SELECT 1 FROM history_items
                 WHERE thread_id=?1 AND event_index=?2 AND item_index=?3 AND item_id=?4
                   AND (?5 IS NULL OR turn_id=?5)",
                params![
                    thread_id,
                    order.event_index,
                    order.item_index,
                    item_id,
                    turn_id
                ],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn history_has_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<bool, ProjectionError> {
        Ok(self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?
            .query_row(
                "SELECT 1 FROM history_turns WHERE thread_id=?1 AND turn_id=?2",
                params![thread_id, turn_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn history_turn_page(
        &self,
        thread_id: &str,
        cursor: Option<(HistoryIndexOrder, bool)>,
        limit: usize,
        sort_direction: HistorySortDirection,
    ) -> Result<Vec<HistoryIndexTurn>, ProjectionError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let direction = match sort_direction {
            HistorySortDirection::Asc => "ASC",
            HistorySortDirection::Desc => "DESC",
        };
        let rows = if let Some((order, inclusive)) = cursor {
            let comparison = history_comparison(sort_direction, inclusive);
            let sql = format!(
                "SELECT event_index,item_index,turn_json FROM history_turns
                 WHERE thread_id=?1 AND {comparison}
                 ORDER BY event_index {direction},item_index {direction} LIMIT ?4"
            );
            let mut statement = connection.prepare(&sql)?;
            statement
                .query_map(
                    params![thread_id, order.event_index, order.item_index, limit as u64],
                    |row| {
                        Ok((
                            row.get::<_, u64>(0)?,
                            row.get::<_, u32>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let sql = format!(
                "SELECT event_index,item_index,turn_json FROM history_turns
                 WHERE thread_id=?1
                 ORDER BY event_index {direction},item_index {direction} LIMIT ?2"
            );
            let mut statement = connection.prepare(&sql)?;
            statement
                .query_map(params![thread_id, limit as u64], |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter()
            .map(|(event_index, item_index, json)| {
                Ok(HistoryIndexTurn {
                    order: HistoryIndexOrder {
                        event_index,
                        item_index,
                    },
                    turn: serde_json::from_str(&json)
                        .map_err(|error| ProjectionError::InvalidData(error.to_string()))?,
                })
            })
            .collect()
    }

    pub fn history_item_page(
        &self,
        thread_id: &str,
        turn_id: Option<&str>,
        cursor: Option<(HistoryIndexOrder, bool)>,
        limit: usize,
        sort_direction: HistorySortDirection,
    ) -> Result<Vec<HistoryIndexItem>, ProjectionError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let direction = match sort_direction {
            HistorySortDirection::Asc => "ASC",
            HistorySortDirection::Desc => "DESC",
        };
        let rows = if let Some((order, inclusive)) = cursor {
            let comparison = history_comparison_with_offset(sort_direction, inclusive, 3, 4);
            let sql = format!(
                "SELECT event_index,item_index,item_json FROM history_items
                 WHERE thread_id=?1 AND (?2 IS NULL OR turn_id=?2) AND {comparison}
                 ORDER BY event_index {direction},item_index {direction} LIMIT ?5"
            );
            let mut statement = connection.prepare(&sql)?;
            statement
                .query_map(
                    params![
                        thread_id,
                        turn_id,
                        order.event_index,
                        order.item_index,
                        limit as u64
                    ],
                    |row| {
                        Ok((
                            row.get::<_, u64>(0)?,
                            row.get::<_, u32>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let sql = format!(
                "SELECT event_index,item_index,item_json FROM history_items
                 WHERE thread_id=?1 AND (?2 IS NULL OR turn_id=?2)
                 ORDER BY event_index {direction},item_index {direction} LIMIT ?3"
            );
            let mut statement = connection.prepare(&sql)?;
            statement
                .query_map(params![thread_id, turn_id, limit as u64], |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter()
            .map(|(event_index, item_index, json)| {
                Ok(HistoryIndexItem {
                    order: HistoryIndexOrder {
                        event_index,
                        item_index,
                    },
                    item: serde_json::from_str(&json)
                        .map_err(|error| ProjectionError::InvalidData(error.to_string()))?,
                })
            })
            .collect()
    }

    pub fn list_threads(&self) -> Result<Vec<ThreadSummary>, ProjectionError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let mut statement = connection.prepare(
            "SELECT id,title,created_at_ms,updated_at_ms,archived,workspace_path,in_project FROM sessions
             WHERE archived=0 ORDER BY updated_at_ms DESC",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(ThreadSummary {
                    schema_version: crate::protocol::PROTOCOL_VERSION,
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at_ms: row.get(2)?,
                    updated_at_ms: row.get(3)?,
                    archived: row.get::<_, i64>(4)? != 0,
                    workspace_path: row.get(5)?,
                    in_project: row.get::<_, i64>(6)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), ProjectionError> {
        self.connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?
            .execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [key, value],
            )?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>, ProjectionError> {
        Ok(self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn delete_setting(&self, key: &str) -> Result<(), ProjectionError> {
        self.connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?
            .execute("DELETE FROM settings WHERE key=?1", [key])?;
        Ok(())
    }

    pub fn upsert_project(&self, project: &ProjectRecord) -> Result<(), ProjectionError> {
        self.connection.lock().map_err(|_| ProjectionError::Poisoned)?.execute(
            "INSERT INTO projects(id,name,path,trusted,last_opened_at_ms) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(path) DO UPDATE SET name=excluded.name,trusted=excluded.trusted,last_opened_at_ms=excluded.last_opened_at_ms",
            params![project.id, project.name, project.path, project.trusted as i64, project.last_opened_at_ms],
        )?;
        Ok(())
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRecord>, ProjectionError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let mut statement = connection.prepare(
            "SELECT id,name,path,trusted,last_opened_at_ms FROM projects ORDER BY last_opened_at_ms DESC")?;
        Ok(statement
            .query_map([], |row| {
                Ok(ProjectRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    trusted: row.get::<_, i64>(3)? != 0,
                    last_opened_at_ms: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// 从项目清单移除。只影响项目登记，**不触碰任何会话或文件**。
    ///
    /// 匹配用路径的归属键（大小写/分隔符折叠），因为登记时写入的是
    /// `canonicalize` 后的路径，而调用方传进来的可能是不带 `\\?\` 前缀的写法。
    /// 返回被删除的行数，0 表示该项目本来就不在清单里（幂等，不算错误）。
    pub fn delete_project(&self, path: &str) -> Result<usize, ProjectionError> {
        let key = crate::workbench::workspace_path_key(path);
        let connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let candidates: Vec<String> = connection
            .prepare("SELECT path FROM projects")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut removed = 0usize;
        for candidate in candidates {
            if crate::workbench::workspace_path_key(&candidate) == key {
                removed +=
                    connection.execute("DELETE FROM projects WHERE path=?1", [&candidate])?;
            }
        }
        Ok(removed)
    }

    pub fn usage_summary(&self) -> Result<UsageSummary, ProjectionError> {
        const AGGREGATE_COLUMNS: &str = "COALESCE(SUM(input_tokens),0),
             COALESCE(SUM(output_tokens),0),
             COALESCE(SUM(total_tokens),0),
             COUNT(*),
             CASE WHEN COUNT(*) > 0 AND COUNT(cached_input_tokens) = COUNT(*)
               THEN SUM(cached_input_tokens) END,
             CASE WHEN COUNT(*) > 0 AND COUNT(uncached_input_tokens) = COUNT(*)
               THEN SUM(uncached_input_tokens) END,
             CASE WHEN COUNT(*) > 0 AND COUNT(cache_write_input_tokens) = COUNT(*)
               THEN SUM(cache_write_input_tokens) END,
             CASE WHEN COUNT(*) > 0 AND COUNT(reasoning_output_tokens) = COUNT(*)
               THEN SUM(reasoning_output_tokens) END";

        let connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let totals = connection.query_row(
            &format!("SELECT {AGGREGATE_COLUMNS} FROM usage"),
            [],
            |row| usage_aggregate_from_row(row, 0),
        )?;

        let mut daily_statement = connection.prepare(
            "SELECT date(created_at_ms / 1000, 'unixepoch', 'localtime'),
                    COUNT(*), SUM(input_tokens), SUM(output_tokens), SUM(total_tokens)
             FROM usage
             WHERE created_at_ms > 0
               AND date(created_at_ms / 1000, 'unixepoch', 'localtime') >=
                   date('now', 'localtime', '-29 days')
               AND date(created_at_ms / 1000, 'unixepoch', 'localtime') <=
                   date('now', 'localtime')
             GROUP BY date(created_at_ms / 1000, 'unixepoch', 'localtime')
             ORDER BY date(created_at_ms / 1000, 'unixepoch', 'localtime') ASC",
        )?;
        let daily = daily_statement
            .query_map([], |row| {
                Ok(DailyUsageSummary {
                    date: row.get(0)?,
                    provider_calls: row.get(1)?,
                    input_tokens: row.get(2)?,
                    output_tokens: row.get(3)?,
                    total_tokens: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut model_statement = connection.prepare(&format!(
            "SELECT provider, model, {AGGREGATE_COLUMNS}
             FROM usage
             GROUP BY provider, model
             ORDER BY SUM(total_tokens) DESC, provider ASC, model ASC"
        ))?;
        let models = model_statement
            .query_map([], |row| {
                let aggregate = usage_aggregate_from_row(row, 2)?;
                Ok(ModelUsageSummary {
                    provider: row.get(0)?,
                    model: row.get(1)?,
                    provider_calls: aggregate.provider_calls,
                    input_tokens: aggregate.input_tokens,
                    output_tokens: aggregate.output_tokens,
                    total_tokens: aggregate.total_tokens,
                    cached_input_tokens: aggregate.cached_input_tokens,
                    uncached_input_tokens: aggregate.uncached_input_tokens,
                    cache_write_input_tokens: aggregate.cache_write_input_tokens,
                    reasoning_output_tokens: aggregate.reasoning_output_tokens,
                    reply_output_tokens: aggregate.reply_output_tokens(),
                    cache_hit_rate: aggregate.cache_hit_rate(),
                    estimated_cost_usd: None,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(UsageSummary {
            schema_version: 2,
            trend_days: 30,
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            total_tokens: totals.total_tokens,
            provider_calls: totals.provider_calls,
            cached_input_tokens: totals.cached_input_tokens,
            uncached_input_tokens: totals.uncached_input_tokens,
            cache_write_input_tokens: totals.cache_write_input_tokens,
            reasoning_output_tokens: totals.reasoning_output_tokens,
            reply_output_tokens: totals.reply_output_tokens(),
            cache_hit_rate: totals.cache_hit_rate(),
            estimated_cost_usd: None,
            daily,
            models,
        })
    }

    #[cfg(test)]
    pub fn indexed_event_ids(&self, thread_id: &str) -> Result<Vec<String>, ProjectionError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let mut statement = connection.prepare(
            "SELECT event_id FROM indexed_events WHERE thread_id=?1 ORDER BY sequence ASC",
        )?;
        Ok(statement
            .query_map([thread_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    }
}

fn usage_aggregate_from_row(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> Result<UsageAggregate, rusqlite::Error> {
    Ok(UsageAggregate {
        input_tokens: row.get(offset)?,
        output_tokens: row.get(offset + 1)?,
        total_tokens: row.get(offset + 2)?,
        provider_calls: row.get(offset + 3)?,
        cached_input_tokens: row.get(offset + 4)?,
        uncached_input_tokens: row.get(offset + 5)?,
        cache_write_input_tokens: row.get(offset + 6)?,
        reasoning_output_tokens: row.get(offset + 7)?,
    })
}

fn insert_indexed_event(
    connection: &Connection,
    sequence: u64,
    event: &StoredEvent,
) -> Result<(), ProjectionError> {
    connection.execute(
        "INSERT INTO indexed_events(thread_id,sequence,event_id,turn_id,created_at_ms,event_json)
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            event.thread_id,
            sequence,
            event.event_id,
            event.turn_id,
            event.created_at_ms,
            serde_json::to_string(event)
                .map_err(|error| ProjectionError::InvalidData(error.to_string()))?
        ],
    )?;
    Ok(())
}

fn history_comparison(sort_direction: HistorySortDirection, inclusive: bool) -> String {
    history_comparison_with_offset(sort_direction, inclusive, 2, 3)
}

fn history_comparison_with_offset(
    sort_direction: HistorySortDirection,
    inclusive: bool,
    event_parameter: usize,
    item_parameter: usize,
) -> String {
    let (event_operator, item_operator) = match (sort_direction, inclusive) {
        (HistorySortDirection::Asc, false) => (">", ">"),
        (HistorySortDirection::Asc, true) => (">", ">="),
        (HistorySortDirection::Desc, false) => ("<", "<"),
        (HistorySortDirection::Desc, true) => ("<", "<="),
    };
    format!(
        "(event_index {event_operator} ?{event_parameter} OR \
         (event_index=?{event_parameter} AND item_index {item_operator} ?{item_parameter}))"
    )
}

fn insert_usage(
    connection: &Connection,
    thread_id: &str,
    turn_id: &str,
    call_index: u32,
    created_at_ms: u64,
    usage: TokenUsage,
    details: TokenUsageDetails,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO usage(
           thread_id,turn_id,call_index,input_tokens,output_tokens,total_tokens,created_at_ms,
           provider,model,cached_input_tokens,uncached_input_tokens,cache_write_input_tokens,
           reasoning_output_tokens)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
         ON CONFLICT(thread_id,turn_id,call_index) DO UPDATE SET
           input_tokens=excluded.input_tokens,
           output_tokens=excluded.output_tokens,
           total_tokens=excluded.total_tokens,
           created_at_ms=excluded.created_at_ms,
           provider=excluded.provider,
           model=excluded.model,
           cached_input_tokens=excluded.cached_input_tokens,
           uncached_input_tokens=excluded.uncached_input_tokens,
           cache_write_input_tokens=excluded.cache_write_input_tokens,
           reasoning_output_tokens=excluded.reasoning_output_tokens",
        params![
            thread_id,
            turn_id,
            call_index,
            usage.input_tokens,
            usage.output_tokens,
            usage.total_tokens,
            created_at_ms,
            provider,
            model,
            details.cached_input_tokens,
            details.uncached_input_tokens,
            details.cache_write_input_tokens,
            details.reasoning_output_tokens,
        ],
    )?;
    Ok(())
}

fn migrate(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);")?;
    let version: u32 = connection.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    if version < 1 {
        connection.execute_batch(
            "BEGIN;
             CREATE TABLE sessions(id TEXT PRIMARY KEY,title TEXT NOT NULL,created_at_ms INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL,archived INTEGER NOT NULL DEFAULT 0,event_count INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
             CREATE TABLE usage(id INTEGER PRIMARY KEY AUTOINCREMENT,thread_id TEXT NOT NULL,turn_id TEXT NOT NULL,
               call_index INTEGER NOT NULL,input_tokens INTEGER NOT NULL,output_tokens INTEGER NOT NULL,total_tokens INTEGER NOT NULL);
             CREATE INDEX usage_thread_turn ON usage(thread_id,turn_id);
             INSERT INTO schema_migrations(version,applied_at) VALUES(1,datetime('now'));
             COMMIT;")?;
    }
    if version < 2 {
        connection.execute_batch(
            "BEGIN;
             CREATE TABLE projects(id TEXT PRIMARY KEY,name TEXT NOT NULL,path TEXT NOT NULL UNIQUE,
               trusted INTEGER NOT NULL DEFAULT 0,last_opened_at_ms INTEGER NOT NULL);
             INSERT INTO schema_migrations(version,applied_at) VALUES(2,datetime('now'));
             COMMIT;",
        )?;
    }
    if version < 3 {
        connection.execute_batch(
            "BEGIN;
             ALTER TABLE sessions ADD COLUMN workspace_path TEXT;
             INSERT INTO schema_migrations(version,applied_at) VALUES(3,datetime('now'));
             COMMIT;",
        )?;
    }
    if version < 4 {
        connection.execute_batch(
            "BEGIN;
             CREATE UNIQUE INDEX IF NOT EXISTS usage_unique_call
               ON usage(thread_id,turn_id,call_index);
             CREATE TABLE indexed_events(
               thread_id TEXT NOT NULL,
               sequence INTEGER NOT NULL,
               event_id TEXT NOT NULL UNIQUE,
               turn_id TEXT,
               created_at_ms INTEGER NOT NULL,
               event_json TEXT NOT NULL,
               PRIMARY KEY(thread_id,sequence));
             CREATE INDEX indexed_events_turn ON indexed_events(thread_id,turn_id,sequence);
             CREATE TABLE history_state(
               thread_id TEXT PRIMARY KEY,
               indexed_event_count INTEGER NOT NULL,
               metadata_json TEXT NOT NULL);
             CREATE TABLE history_turns(
               thread_id TEXT NOT NULL,
               event_index INTEGER NOT NULL,
               item_index INTEGER NOT NULL,
               turn_id TEXT NOT NULL,
               turn_json TEXT NOT NULL,
               PRIMARY KEY(thread_id,event_index,item_index));
             CREATE UNIQUE INDEX history_turn_id ON history_turns(thread_id,turn_id);
             CREATE TABLE history_items(
               thread_id TEXT NOT NULL,
               event_index INTEGER NOT NULL,
               item_index INTEGER NOT NULL,
               turn_id TEXT,
               item_id TEXT NOT NULL,
               item_json TEXT NOT NULL,
               PRIMARY KEY(thread_id,event_index,item_index));
             CREATE INDEX history_items_turn_order
               ON history_items(thread_id,turn_id,event_index,item_index);
             INSERT INTO schema_migrations(version,applied_at) VALUES(4,datetime('now'));
             COMMIT;",
        )?;
    }
    if version < 5 {
        connection.execute_batch(
            "BEGIN;
             ALTER TABLE sessions ADD COLUMN in_project INTEGER NOT NULL DEFAULT 1;
             INSERT INTO schema_migrations(version,applied_at) VALUES(5,datetime('now'));
             COMMIT;",
        )?;
    }
    if version < 6 {
        connection.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS knowledge_collections(
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL,
               scope TEXT NOT NULL,
               scope_key TEXT NOT NULL,
               enabled INTEGER NOT NULL DEFAULT 0,
               deleted INTEGER NOT NULL DEFAULT 0,
               created_at_ms INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL,
               UNIQUE(scope_key,name));
             CREATE INDEX IF NOT EXISTS knowledge_collections_enabled ON knowledge_collections(enabled,deleted);
             CREATE TABLE IF NOT EXISTS knowledge_sources(
               id TEXT PRIMARY KEY,
               collection_id TEXT NOT NULL,
               workspace_id TEXT NOT NULL,
               relative_path TEXT NOT NULL,
               size_bytes INTEGER NOT NULL,
               modified_at_ms INTEGER NOT NULL,
               content_hash TEXT,
               active_revision_id TEXT,
               active_embedding_model TEXT,
               active_embedding_dimension INTEGER NOT NULL DEFAULT 0,
               active_embedding_encoding_format TEXT NOT NULL DEFAULT 'float',
               embedding_status TEXT NOT NULL DEFAULT 'lexical_only',
               state TEXT NOT NULL,
               last_error_code TEXT,
               created_at_ms INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL,
               UNIQUE(collection_id,workspace_id,relative_path));
             CREATE INDEX IF NOT EXISTS knowledge_sources_collection ON knowledge_sources(collection_id);
             CREATE TABLE IF NOT EXISTS knowledge_revisions(
               id TEXT PRIMARY KEY,
               source_id TEXT NOT NULL,
               revision_hash TEXT NOT NULL,
               parser_version TEXT NOT NULL,
               chunker_version TEXT NOT NULL,
               embedding_provider TEXT NOT NULL DEFAULT 'none',
               embedding_model TEXT,
               embedding_dimension INTEGER NOT NULL DEFAULT 0,
               embedding_encoding_format TEXT NOT NULL DEFAULT 'float',
               embedding_status TEXT NOT NULL DEFAULT 'lexical_only',
               active INTEGER NOT NULL DEFAULT 0,
               created_at_ms INTEGER NOT NULL,
               UNIQUE(source_id,revision_hash));
             CREATE INDEX IF NOT EXISTS knowledge_revisions_active ON knowledge_revisions(source_id,active);
             CREATE TABLE IF NOT EXISTS knowledge_chunks(
               id TEXT PRIMARY KEY,
               revision_id TEXT NOT NULL,
               ordinal INTEGER NOT NULL,
               title TEXT NOT NULL,
               text TEXT NOT NULL,
               terms TEXT NOT NULL,
               token_estimate INTEGER NOT NULL,
               start_line INTEGER NOT NULL,
               end_line INTEGER NOT NULL,
               UNIQUE(revision_id,ordinal));
             CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_chunks_fts USING fts5(
               chunk_id UNINDEXED, revision_id UNINDEXED, title, text, terms,
               tokenize='unicode61 remove_diacritics 0');
             CREATE TABLE IF NOT EXISTS knowledge_chunk_embeddings(
               chunk_id TEXT NOT NULL,
               revision_id TEXT NOT NULL,
               provider TEXT NOT NULL,
               model TEXT NOT NULL,
               dimension INTEGER NOT NULL,
               encoding_format TEXT NOT NULL DEFAULT 'float',
               vector BLOB NOT NULL,
               vector_hash TEXT NOT NULL,
               created_at_ms INTEGER NOT NULL,
               PRIMARY KEY(chunk_id,provider,model,dimension,encoding_format));
             CREATE TABLE IF NOT EXISTS knowledge_index_jobs(
               id TEXT PRIMARY KEY,
               source_id TEXT NOT NULL,
               requested_revision_hash TEXT,
               stage TEXT NOT NULL DEFAULT 'parse',
               embedding_mode TEXT NOT NULL DEFAULT 'lexical_only',
               processed_chunks INTEGER NOT NULL DEFAULT 0,
               total_chunks INTEGER NOT NULL DEFAULT 0,
               embedding_requests INTEGER NOT NULL DEFAULT 0,
               retry_count INTEGER NOT NULL DEFAULT 0,
               last_http_status INTEGER,
               state TEXT NOT NULL,
               processed_bytes INTEGER NOT NULL DEFAULT 0,
               total_bytes INTEGER NOT NULL DEFAULT 0,
               error_code TEXT,
               error_message TEXT,
               created_at_ms INTEGER NOT NULL,
               started_at_ms INTEGER,
               completed_at_ms INTEGER);
             CREATE INDEX IF NOT EXISTS knowledge_index_jobs_source ON knowledge_index_jobs(source_id,state,created_at_ms);
             INSERT INTO schema_migrations(version,applied_at) VALUES(6,datetime('now'));
             COMMIT;",
        )?;
    }
    if version < 7 {
        connection.execute_batch(
            "BEGIN;
             ALTER TABLE knowledge_index_jobs ADD COLUMN chunk_count INTEGER NOT NULL DEFAULT 0;
             INSERT INTO schema_migrations(version,applied_at) VALUES(7,datetime('now'));
             COMMIT;",
        )?;
    }
    if version < 8 {
        connection.execute_batch(
            "BEGIN;
             ALTER TABLE knowledge_index_jobs ADD COLUMN vector_count INTEGER NOT NULL DEFAULT 0;
             INSERT INTO schema_migrations(version,applied_at) VALUES(8,datetime('now'));
             COMMIT;",
        )?;
    }
    if version < 9 {
        connection.execute_batch(
            "BEGIN;
             ALTER TABLE usage ADD COLUMN created_at_ms INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE usage ADD COLUMN provider TEXT;
             ALTER TABLE usage ADD COLUMN model TEXT;
             ALTER TABLE usage ADD COLUMN cached_input_tokens INTEGER;
             ALTER TABLE usage ADD COLUMN uncached_input_tokens INTEGER;
             ALTER TABLE usage ADD COLUMN cache_write_input_tokens INTEGER;
             ALTER TABLE usage ADD COLUMN reasoning_output_tokens INTEGER;
             UPDATE usage
             SET created_at_ms = COALESCE((
               SELECT indexed_events.created_at_ms
               FROM indexed_events
               WHERE indexed_events.thread_id = usage.thread_id
                 AND indexed_events.turn_id = usage.turn_id
                 AND json_extract(indexed_events.event_json, '$.type') = 'provider_call_usage'
                 AND COALESCE(
                   json_extract(indexed_events.event_json, '$.data.call_index'),
                   json_extract(indexed_events.event_json, '$.data.callIndex')
                 ) = usage.call_index
               ORDER BY indexed_events.sequence ASC
               LIMIT 1
             ), 0);
             CREATE INDEX usage_created_at ON usage(created_at_ms);
             CREATE INDEX usage_provider_model ON usage(provider,model);
             INSERT INTO schema_migrations(version,applied_at) VALUES(9,datetime('now'));
             COMMIT;",
        )?;
    }
    if version < 10 {
        // Knowledge and memory extension tables. Existing knowledge_collections /
        // knowledge_sources / knowledge_revisions / knowledge_chunks / knowledge_chunk_embeddings
        // stay untouched, so lexical and semantic retrieval keep working while the structured
        // knowledge and memory projections are added.
        //
        // Unlike the earlier inline `BEGIN; ... COMMIT;` batches this block runs in a checked
        // transaction: a failing statement rolls the whole block back on the same connection and
        // never records version 10.
        let transaction = connection.unchecked_transaction()?;
        transaction.execute_batch(
            "CREATE TABLE memories(
               id TEXT PRIMARY KEY,
               scope_type TEXT NOT NULL,
               scope_id TEXT,
               memory_type TEXT NOT NULL,
               normalized_key TEXT NOT NULL,
               content TEXT NOT NULL,
               source_type TEXT NOT NULL,
               source_ref TEXT,
               confidence REAL NOT NULL CHECK(confidence >= 0 AND confidence <= 1),
               sensitivity TEXT NOT NULL DEFAULT 'normal',
               status TEXT NOT NULL DEFAULT 'active',
               revision INTEGER NOT NULL DEFAULT 1,
               expires_at_ms INTEGER,
               created_at_ms INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL,
               UNIQUE(scope_type,scope_id,normalized_key,revision));
             CREATE INDEX memories_scope_status ON memories(scope_type,scope_id,status);
             CREATE INDEX memories_normalized_key ON memories(normalized_key);
             CREATE INDEX memories_created_at ON memories(created_at_ms);
             CREATE TABLE memory_candidates(
               id TEXT PRIMARY KEY,
               operation TEXT NOT NULL,
               target_memory_id TEXT,
               scope_type TEXT NOT NULL,
               scope_id TEXT,
               memory_type TEXT NOT NULL,
               content TEXT NOT NULL,
               normalized_key TEXT NOT NULL,
               reason TEXT NOT NULL,
               confidence REAL NOT NULL CHECK(confidence >= 0 AND confidence <= 1),
               requires_review INTEGER NOT NULL,
               status TEXT NOT NULL DEFAULT 'pending',
               source_turn_id TEXT,
               created_at_ms INTEGER NOT NULL,
               reviewed_at_ms INTEGER);
             CREATE INDEX memory_candidates_scope_status
               ON memory_candidates(scope_type,scope_id,status);
             CREATE INDEX memory_candidates_normalized_key ON memory_candidates(normalized_key);
             CREATE INDEX memory_candidates_created_at ON memory_candidates(created_at_ms);
             CREATE TABLE knowledge_entities(
               id TEXT PRIMARY KEY,
               collection_id TEXT NOT NULL,
               entity_type TEXT NOT NULL,
               name TEXT NOT NULL,
               normalized_name TEXT NOT NULL,
               description TEXT,
               confidence REAL NOT NULL CHECK(confidence >= 0 AND confidence <= 1),
               status TEXT NOT NULL DEFAULT 'active',
               created_at_ms INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL);
             CREATE INDEX knowledge_entities_collection
               ON knowledge_entities(collection_id,status);
             CREATE INDEX knowledge_entities_normalized_name
               ON knowledge_entities(normalized_name);
             CREATE INDEX knowledge_entities_created_at ON knowledge_entities(created_at_ms);
             CREATE TABLE knowledge_facts(
               id TEXT PRIMARY KEY,
               subject_entity_id TEXT NOT NULL,
               predicate TEXT NOT NULL,
               object_entity_id TEXT,
               object_text TEXT,
               source_chunk_id TEXT NOT NULL,
               source_revision_id TEXT NOT NULL,
               confidence REAL NOT NULL CHECK(confidence >= 0 AND confidence <= 1),
               valid_from_ms INTEGER,
               valid_to_ms INTEGER,
               status TEXT NOT NULL DEFAULT 'candidate',
               created_at_ms INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL,
               CHECK (object_entity_id IS NOT NULL OR object_text IS NOT NULL));
             CREATE INDEX knowledge_facts_source_chunk
               ON knowledge_facts(source_chunk_id);
             CREATE INDEX knowledge_facts_subject
               ON knowledge_facts(subject_entity_id,predicate,status);
             CREATE INDEX knowledge_facts_created_at ON knowledge_facts(created_at_ms);
             CREATE TABLE knowledge_retrieval_events(
               id TEXT PRIMARY KEY,
               thread_id TEXT NOT NULL,
               turn_id TEXT NOT NULL,
               query_hash TEXT NOT NULL,
               retrieval_mode TEXT NOT NULL,
               result_count INTEGER NOT NULL,
               selected_citation_count INTEGER NOT NULL,
               latency_ms INTEGER NOT NULL,
               created_at_ms INTEGER NOT NULL);
             CREATE INDEX knowledge_retrieval_events_thread_turn
               ON knowledge_retrieval_events(thread_id,turn_id);
             CREATE INDEX knowledge_retrieval_events_created_at
               ON knowledge_retrieval_events(created_at_ms);
             CREATE TABLE knowledge_feedback(
               id TEXT PRIMARY KEY,
               citation_id TEXT NOT NULL,
               feedback_type TEXT NOT NULL,
               created_at_ms INTEGER NOT NULL);
             CREATE INDEX knowledge_feedback_citation ON knowledge_feedback(citation_id);
             CREATE INDEX knowledge_feedback_created_at ON knowledge_feedback(created_at_ms);",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version,applied_at) VALUES(10,datetime('now'))",
            [],
        )?;
        transaction.commit()?;
    }
    if version < 11 {
        // Retrieval feedback must be attributable to the chunk it rated, otherwise the ranking
        // signal in `knowledge::retrieval` has no long-lived source: the in-process citation map is
        // dropped on restart. Both columns stay nullable so the v10 rows keep their meaning —
        // `chunk_id IS NULL` simply reads as "source unknown" and never matches a chunk.
        let transaction = connection.unchecked_transaction()?;
        transaction.execute_batch(
            "ALTER TABLE knowledge_feedback ADD COLUMN chunk_id TEXT;
             ALTER TABLE knowledge_feedback ADD COLUMN source_revision_id TEXT;
             CREATE INDEX knowledge_feedback_chunk
               ON knowledge_feedback(chunk_id,feedback_type);",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version,applied_at) VALUES(11,datetime('now'))",
            [],
        )?;
        transaction.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_history_metadata_defaults_the_context_usage_baseline() {
        let metadata = HistoryIndexMetadata {
            summary: ThreadSummary {
                schema_version: 1,
                id: "thread-1".to_string(),
                title: "Legacy thread".to_string(),
                created_at_ms: 1,
                updated_at_ms: 2,
                archived: false,
                in_project: true,
                workspace_path: None,
            },
            last_turn: None,
            todos: Vec::new(),
            last_usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 2,
                total_tokens: 12,
            }),
            context_usage: Some(TokenUsage {
                input_tokens: 8,
                output_tokens: 2,
                total_tokens: 10,
            }),
            unscoped_items: Vec::new(),
        };
        let mut legacy = serde_json::to_value(metadata).unwrap();
        legacy.as_object_mut().unwrap().remove("contextUsage");

        let restored: HistoryIndexMetadata = serde_json::from_value(legacy).unwrap();

        assert_eq!(restored.context_usage, None);
        assert_eq!(restored.last_usage.unwrap().total_tokens, 12);
    }

    #[test]
    fn delete_project_matches_by_normalized_key_and_is_idempotent() {
        let db = ProjectionDb::memory().unwrap();
        let record = |id: &str, path: &str| ProjectRecord {
            id: id.to_string(),
            name: id.to_string(),
            path: path.to_string(),
            trusted: true,
            last_opened_at_ms: 1,
        };
        // 同一个目录的两种写法（大小写 + 尾分隔符）都应被同一次删除命中。
        db.upsert_project(&record("a", r"D:\code\App")).unwrap();
        db.upsert_project(&record("b", "d:/code/app/")).unwrap();
        db.upsert_project(&record("c", r"D:\code\other")).unwrap();

        let removed = db.delete_project(r"\\?\D:\Code\APP").unwrap();

        assert_eq!(removed, 2, "同一归属键的多行必须一次全部删除");
        let remaining = db.list_projects().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "c");

        // 幂等：再删一次不该报错，也不该影响其他项目。
        assert_eq!(db.delete_project(r"D:\code\APP").unwrap(), 0);
        assert_eq!(db.list_projects().unwrap().len(), 1);
    }

    #[test]
    fn migrates_every_published_database_version_forward() {
        let db = ProjectionDb::memory().unwrap();
        db.set_setting("theme", "dark").unwrap();
        assert_eq!(db.setting("theme").unwrap().as_deref(), Some("dark"));
        let version: u32 = db
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, DATABASE_SCHEMA_VERSION);
        let columns = db
            .connection
            .lock()
            .unwrap()
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "workspace_path"));
        assert!(columns.iter().any(|column| column == "in_project"));
        let usage_columns = db
            .connection
            .lock()
            .unwrap()
            .prepare("PRAGMA table_info(usage)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for required in [
            "created_at_ms",
            "provider",
            "model",
            "cached_input_tokens",
            "uncached_input_tokens",
            "cache_write_input_tokens",
            "reasoning_output_tokens",
        ] {
            assert!(
                usage_columns.iter().any(|column| column == required),
                "usage table should contain {required}"
            );
        }
        let tables = db
            .connection
            .lock()
            .unwrap()
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type='table' AND name IN ('indexed_events','history_state','history_turns','history_items')
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tables.len(), 4);
        let knowledge_job_columns = db
            .connection
            .lock()
            .unwrap()
            .prepare("PRAGMA table_info(knowledge_index_jobs)")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, Option<String>>(4)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            knowledge_job_columns.iter().any(|(name, default)| {
                name == "vector_count" && default.as_deref() == Some("0")
            })
        );
        let embedding_table_exists: bool = db
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='knowledge_chunk_embeddings')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(embedding_table_exists);
    }

    const EXTENSION_TABLES: [&str; 6] = [
        "memories",
        "memory_candidates",
        "knowledge_entities",
        "knowledge_facts",
        "knowledge_retrieval_events",
        "knowledge_feedback",
    ];

    fn sqlite_names(connection: &Connection, query: &str) -> Vec<String> {
        connection
            .prepare(query)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn migration_creates_the_knowledge_and_memory_tables_without_touching_the_knowledge_index() {
        let db = ProjectionDb::memory().unwrap();
        let connection = db.connection.lock().unwrap();
        let tables = sqlite_names(
            &connection,
            "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
        );
        for required in EXTENSION_TABLES {
            assert!(
                tables.iter().any(|table| table == required),
                "schema v10 should create {required}"
            );
        }
        // Lexical and semantic retrieval keep working because the existing projection survives.
        for preserved in [
            "knowledge_collections",
            "knowledge_sources",
            "knowledge_revisions",
            "knowledge_chunks",
            "knowledge_chunk_embeddings",
            "knowledge_index_jobs",
        ] {
            assert!(
                tables.iter().any(|table| table == preserved),
                "schema v10 must keep {preserved}"
            );
        }
    }

    #[test]
    fn migration_adds_the_scope_status_normalized_key_and_source_chunk_indexes() {
        let db = ProjectionDb::memory().unwrap();
        let connection = db.connection.lock().unwrap();
        let indexes = sqlite_names(
            &connection,
            "SELECT name FROM sqlite_master WHERE type='index' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        );
        let expected: [(&str, &str, &[&str]); 7] = [
            (
                "memories_scope_status",
                "memories",
                &["scope_type", "scope_id", "status"],
            ),
            ("memories_normalized_key", "memories", &["normalized_key"]),
            ("memories_created_at", "memories", &["created_at_ms"]),
            (
                "memory_candidates_scope_status",
                "memory_candidates",
                &["scope_type", "scope_id", "status"],
            ),
            (
                "knowledge_facts_source_chunk",
                "knowledge_facts",
                &["source_chunk_id"],
            ),
            (
                "knowledge_entities_normalized_name",
                "knowledge_entities",
                &["normalized_name"],
            ),
            (
                "knowledge_retrieval_events_created_at",
                "knowledge_retrieval_events",
                &["created_at_ms"],
            ),
        ];
        for (name, table, columns) in expected {
            assert!(
                indexes.iter().any(|index| index == name),
                "missing index {name}"
            );
            let info = connection
                .prepare(&format!("PRAGMA index_info({name})"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(2))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(info, columns, "index {name} should cover {columns:?}");
            let indexed_table: String = connection
                .query_row(
                    "SELECT tbl_name FROM sqlite_master WHERE type='index' AND name=?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(indexed_table, table);
        }
    }

    #[test]
    fn scope_status_normalized_key_and_source_chunk_lookups_avoid_full_scans() {
        let db = ProjectionDb::memory().unwrap();
        let connection = db.connection.lock().unwrap();
        for (query, index) in [
            (
                "SELECT id FROM memories WHERE scope_type='user' AND status='active'",
                "memories_scope_status",
            ),
            (
                "SELECT id FROM memories WHERE normalized_key='use pnpm'",
                "memories_normalized_key",
            ),
            (
                "SELECT id FROM knowledge_facts WHERE source_chunk_id='chunk-1'",
                "knowledge_facts_source_chunk",
            ),
        ] {
            let plan = connection
                .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join(" | ");
            assert!(
                plan.contains(index),
                "expected the query planner to use {index}, got: {plan}"
            );
            assert!(
                !plan.contains("SCAN memories") && !plan.contains("SCAN knowledge_facts"),
                "expected an index lookup instead of a full scan, got: {plan}"
            );
        }
    }

    #[test]
    fn repeated_migration_runs_are_idempotent() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        migrate(&connection).unwrap();
        let versions = sqlite_names(
            &connection,
            "SELECT CAST(version AS TEXT) FROM schema_migrations ORDER BY version",
        );
        let expected = (1..=DATABASE_SCHEMA_VERSION)
            .map(|version| version.to_string())
            .collect::<Vec<_>>();
        assert_eq!(versions, expected);
        let extension_tables = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN
                   ('memories','memory_candidates','knowledge_entities','knowledge_facts',
                    'knowledge_retrieval_events','knowledge_feedback')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(extension_tables, EXTENSION_TABLES.len() as i64);
    }

    #[test]
    fn a_failing_migration_block_rolls_back_without_recording_the_version() {
        let connection = Connection::open_in_memory().unwrap();
        // A pre-existing `memories` table makes the schema v10 block fail at its first statement,
        // so the v11 block is never reached.
        connection
            .execute_batch("CREATE TABLE memories(id TEXT PRIMARY KEY);")
            .unwrap();

        assert!(migrate(&connection).is_err());

        let recorded: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version=10",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            recorded, 0,
            "a rolled back block must not record its version"
        );
        let version: u32 = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            version, 9,
            "the schema must stop at the last successfully applied version"
        );
        let leaked: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN
                   ('memory_candidates','knowledge_entities','knowledge_facts',
                    'knowledge_retrieval_events','knowledge_feedback')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            leaked, 0,
            "a failing block must not leave partial tables behind"
        );
    }

    #[test]
    fn a_failing_feedback_migration_rolls_back_its_columns_and_index() {
        let connection = Connection::open_in_memory().unwrap();
        // An unrelated table already owns the index name the v11 block creates, so v10 applies
        // cleanly and only the v11 block fails — which is what makes the column rollback visible.
        connection
            .execute_batch(
                "CREATE TABLE scratch(id TEXT);
                 CREATE INDEX knowledge_feedback_chunk ON scratch(id);",
            )
            .unwrap();

        assert!(migrate(&connection).is_err());

        let recorded: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version=?1",
                [DATABASE_SCHEMA_VERSION],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            recorded, 0,
            "a rolled back block must not record its version"
        );
        let version: u32 = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, DATABASE_SCHEMA_VERSION - 1);
        let columns = sqlite_names(
            &connection,
            "SELECT name FROM pragma_table_info('knowledge_feedback')",
        );
        assert!(
            !columns.iter().any(|name| name == "chunk_id"),
            "a rolled back block must not leave its columns behind, got: {columns:?}"
        );
        assert!(
            !columns.iter().any(|name| name == "source_revision_id"),
            "a rolled back block must not leave its columns behind, got: {columns:?}"
        );
    }

    #[test]
    fn migrating_an_existing_v10_database_binds_legacy_feedback_to_no_chunk() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        // Rebuild the published v10 shape: drop the v11 index and columns, then seed a feedback row
        // written before chunk attribution existed.
        connection
            .execute_batch(
                "DROP INDEX knowledge_feedback_chunk;
                 ALTER TABLE knowledge_feedback DROP COLUMN chunk_id;
                 ALTER TABLE knowledge_feedback DROP COLUMN source_revision_id;
                 DELETE FROM schema_migrations WHERE version=11;
                 INSERT INTO knowledge_feedback(id,citation_id,feedback_type,created_at_ms)
                   VALUES('feedback-legacy','citation-legacy','useful',5);",
            )
            .unwrap();

        migrate(&connection).unwrap();

        let version: u32 = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, DATABASE_SCHEMA_VERSION);
        let legacy: (String, Option<String>) = connection
            .query_row(
                "SELECT feedback_type,chunk_id FROM knowledge_feedback WHERE id='feedback-legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(legacy.0, "useful");
        assert_eq!(
            legacy.1, None,
            "a pre-v11 feedback row must stay readable with an unknown source"
        );
    }

    #[test]
    fn migrating_an_existing_v9_database_adds_the_tables_without_touching_existing_rows() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        // Roll the schema back to the published v9 shape and seed rows a real upgrade would meet.
        // Dropping `knowledge_feedback` also drops the v11 index built on top of it.
        connection
            .execute_batch(
                "DROP TABLE knowledge_feedback;
                 DROP TABLE knowledge_retrieval_events;
                 DROP TABLE knowledge_facts;
                 DROP TABLE knowledge_entities;
                 DROP TABLE memory_candidates;
                 DROP TABLE memories;
                 DELETE FROM schema_migrations WHERE version>=10;
                 INSERT INTO sessions(id,title,created_at_ms,updated_at_ms,archived,event_count,workspace_path,in_project)
                   VALUES('thread-legacy','Legacy thread',1,2,0,3,'D:\\work',1);
                 INSERT INTO knowledge_collections(id,name,scope,scope_key,enabled,deleted,created_at_ms,updated_at_ms)
                   VALUES('collection-legacy','Legacy docs','workspace','D:\\work',1,0,1,1);
                 INSERT INTO knowledge_sources(id,collection_id,workspace_id,relative_path,size_bytes,modified_at_ms,state,created_at_ms,updated_at_ms)
                   VALUES('source-legacy','collection-legacy','D:\\work','docs/guide.md',42,7,'ready',1,1);",
            )
            .unwrap();

        migrate(&connection).unwrap();

        let version: u32 = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, DATABASE_SCHEMA_VERSION);
        let title: String = connection
            .query_row(
                "SELECT title FROM sessions WHERE id='thread-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "Legacy thread");
        let knowledge_rows: i64 = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM knowledge_collections WHERE id='collection-legacy')
                      + (SELECT COUNT(*) FROM knowledge_sources WHERE id='source-legacy')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            knowledge_rows, 2,
            "the knowledge index must survive the upgrade"
        );
        let extension_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN
                   ('memories','memory_candidates','knowledge_entities','knowledge_facts',
                    'knowledge_retrieval_events','knowledge_feedback')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(extension_tables, EXTENSION_TABLES.len() as i64);
    }

    #[test]
    fn usage_summary_exposes_detailed_totals_models_and_recent_daily_series() {
        let db = ProjectionDb::memory().unwrap();
        let now_ms = chrono::Utc::now().timestamp_millis();
        db.with_connection(|connection| {
            for values in [
                (
                    "thread-1", "turn-1", 0, 100_u64, 30_u64, 130_u64, now_ms, "openai",
                    "gpt-test", 40_u64, 40_u64, 20_u64, 10_u64,
                ),
                (
                    "thread-1",
                    "turn-1",
                    1,
                    200_u64,
                    50_u64,
                    250_u64,
                    now_ms - 86_400_000,
                    "openai",
                    "gpt-test",
                    100_u64,
                    100_u64,
                    0_u64,
                    20_u64,
                ),
                (
                    "thread-2",
                    "turn-2",
                    0,
                    50_u64,
                    20_u64,
                    70_u64,
                    now_ms,
                    "google",
                    "gemini-test",
                    10_u64,
                    40_u64,
                    0_u64,
                    5_u64,
                ),
                (
                    "thread-1",
                    "turn-old",
                    0,
                    8_u64,
                    2_u64,
                    10_u64,
                    now_ms - 31 * 86_400_000,
                    "openai",
                    "gpt-test",
                    2_u64,
                    6_u64,
                    0_u64,
                    1_u64,
                ),
            ] {
                connection.execute(
                    "INSERT INTO usage(
                       thread_id,turn_id,call_index,input_tokens,output_tokens,total_tokens,
                       created_at_ms,provider,model,cached_input_tokens,uncached_input_tokens,
                       cache_write_input_tokens,reasoning_output_tokens)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                    params![
                        values.0, values.1, values.2, values.3, values.4, values.5, values.6,
                        values.7, values.8, values.9, values.10, values.11, values.12
                    ],
                )?;
            }
            Ok(())
        })
        .unwrap();

        let value = serde_json::to_value(db.usage_summary().unwrap()).unwrap();
        assert_eq!(value["schemaVersion"], 2);
        assert_eq!(value["trendDays"], 30);
        assert_eq!(value["providerCalls"], 4);
        assert_eq!(value["inputTokens"], 358);
        assert_eq!(value["outputTokens"], 102);
        assert_eq!(value["totalTokens"], 460);
        assert_eq!(value["cachedInputTokens"], 152);
        assert_eq!(value["uncachedInputTokens"], 186);
        assert_eq!(value["cacheWriteInputTokens"], 20);
        assert_eq!(value["reasoningOutputTokens"], 36);
        assert_eq!(value["replyOutputTokens"], 66);
        assert_eq!(value["estimatedCostUsd"], serde_json::Value::Null);
        assert!((value["cacheHitRate"].as_f64().unwrap() - (152.0 / 358.0)).abs() < 1e-9);

        let models = value["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["provider"], "openai");
        assert_eq!(models[0]["model"], "gpt-test");
        assert_eq!(models[0]["providerCalls"], 3);
        assert_eq!(models[0]["totalTokens"], 390);
        assert_eq!(models[1]["provider"], "google");
        assert_eq!(models[1]["model"], "gemini-test");

        let daily = value["daily"].as_array().unwrap();
        assert_eq!(daily.len(), 2);
        assert_eq!(
            daily
                .iter()
                .map(|day| day["totalTokens"].as_u64().unwrap())
                .sum::<u64>(),
            450
        );
    }

    #[test]
    fn usage_summary_keeps_unreported_detail_unknown() {
        let db = ProjectionDb::memory().unwrap();
        db.with_connection(|connection| {
            connection.execute(
                "INSERT INTO usage(
                   thread_id,turn_id,call_index,input_tokens,output_tokens,total_tokens,created_at_ms)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    "legacy-thread",
                    "legacy-turn",
                    0,
                    12_u64,
                    3_u64,
                    15_u64,
                    chrono::Utc::now().timestamp_millis()
                ],
            )?;
            Ok(())
        })
        .unwrap();

        let value = serde_json::to_value(db.usage_summary().unwrap()).unwrap();
        for field in [
            "cachedInputTokens",
            "uncachedInputTokens",
            "cacheWriteInputTokens",
            "reasoningOutputTokens",
            "replyOutputTokens",
            "cacheHitRate",
        ] {
            assert_eq!(value[field], serde_json::Value::Null, "{field}");
        }
        assert_eq!(value["models"][0]["provider"], serde_json::Value::Null);
        assert_eq!(value["models"][0]["model"], serde_json::Value::Null);
    }

    #[test]
    fn cache_hit_rate_uses_input_total_when_cache_write_is_partially_reported() {
        let db = ProjectionDb::memory().unwrap();
        db.with_connection(|connection| {
            connection.execute_batch(
                "INSERT INTO usage(
                   thread_id,turn_id,call_index,input_tokens,output_tokens,total_tokens,
                   cached_input_tokens,uncached_input_tokens,cache_write_input_tokens)
                 VALUES('thread-1','turn-1',0,100,10,110,40,40,20);
                 INSERT INTO usage(
                   thread_id,turn_id,call_index,input_tokens,output_tokens,total_tokens,
                   cached_input_tokens,uncached_input_tokens,cache_write_input_tokens)
                 VALUES('thread-2','turn-2',0,100,10,110,40,60,NULL);",
            )?;
            Ok(())
        })
        .unwrap();

        let summary = db.usage_summary().unwrap();

        assert_eq!(summary.cache_write_input_tokens, None);
        assert_eq!(summary.cache_hit_rate, Some(0.4));
    }

    #[test]
    fn v9_migration_backfills_usage_dates_from_indexed_events() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
                 INSERT INTO schema_migrations(version,applied_at) VALUES(8,datetime('now'));
                 CREATE TABLE usage(
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   thread_id TEXT NOT NULL,
                   turn_id TEXT NOT NULL,
                   call_index INTEGER NOT NULL,
                   input_tokens INTEGER NOT NULL,
                   output_tokens INTEGER NOT NULL,
                   total_tokens INTEGER NOT NULL);
                 CREATE UNIQUE INDEX usage_unique_call ON usage(thread_id,turn_id,call_index);
                 CREATE TABLE indexed_events(
                   thread_id TEXT NOT NULL,
                   sequence INTEGER NOT NULL,
                   event_id TEXT NOT NULL UNIQUE,
                   turn_id TEXT,
                   created_at_ms INTEGER NOT NULL,
                   event_json TEXT NOT NULL,
                   PRIMARY KEY(thread_id,sequence));",
            )
            .unwrap();
        let mut event = StoredEvent::new(
            "thread-1",
            Some("turn-1".into()),
            StoredEventKind::ProviderCallUsage {
                call_index: 2,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    total_tokens: 12,
                },
                details: TokenUsageDetails::default(),
                provider: None,
                model: None,
            },
        );
        event.created_at_ms = 1_725_734_567_890;
        connection
            .execute(
                "INSERT INTO usage(thread_id,turn_id,call_index,input_tokens,output_tokens,total_tokens)
                 VALUES('thread-1','turn-1',2,10,2,12)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO indexed_events(thread_id,sequence,event_id,turn_id,created_at_ms,event_json)
                 VALUES(?1,0,?2,?3,?4,?5)",
                params![
                    event.thread_id,
                    event.event_id,
                    event.turn_id,
                    event.created_at_ms,
                    serde_json::to_string(&event).unwrap()
                ],
            )
            .unwrap();

        migrate(&connection).unwrap();

        let created_at_ms: u64 = connection
            .query_row("SELECT created_at_ms FROM usage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(created_at_ms, event.created_at_ms);
    }
}
