use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};

use crate::protocol::{HistorySortDirection, ThreadItem, ThreadTurn, TodoItem, TokenUsage};
use crate::storage::{StoredEvent, StoredEventKind, ThreadSummary, TurnSnapshot};

pub const DATABASE_SCHEMA_VERSION: u32 = 5;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub path: String,
    pub trusted: bool,
    pub last_opened_at_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub provider_calls: u64,
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
        })
    }

    #[cfg(test)]
    pub fn memory() -> Result<Self, ProjectionError> {
        let connection = Connection::open_in_memory()?;
        migrate(&connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
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
            let (turn_id, call_index, usage) = match &event.kind {
                StoredEventKind::ProviderCallUsage { call_index, usage } => {
                    (event.turn_id.as_deref(), *call_index, Some(*usage))
                }
                _ => (None, 0, None),
            };
            if let (Some(turn_id), Some(usage)) = (turn_id, usage) {
                insert_usage(&transaction, &summary.id, turn_id, call_index, usage)?;
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
        if let StoredEventKind::ProviderCallUsage { call_index, usage } = event.kind
            && let Some(turn_id) = event.turn_id.as_deref()
        {
            insert_usage(&transaction, &event.thread_id, turn_id, call_index, usage)?;
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

    pub fn usage_summary(&self) -> Result<UsageSummary, ProjectionError> {
        Ok(self.connection.lock().map_err(|_| ProjectionError::Poisoned)?.query_row(
            "SELECT COALESCE(SUM(input_tokens),0),COALESCE(SUM(output_tokens),0),COALESCE(SUM(total_tokens),0),COUNT(*) FROM usage",
            [], |row| Ok(UsageSummary { input_tokens: row.get(0)?, output_tokens: row.get(1)?,
                total_tokens: row.get(2)?, provider_calls: row.get(3)? }))?)
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
    usage: TokenUsage,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO usage(thread_id,turn_id,call_index,input_tokens,output_tokens,total_tokens)
         VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(thread_id,turn_id,call_index) DO UPDATE SET
           input_tokens=excluded.input_tokens,
           output_tokens=excluded.output_tokens,
           total_tokens=excluded.total_tokens",
        params![
            thread_id,
            turn_id,
            call_index,
            usage.input_tokens,
            usage.output_tokens,
            usage.total_tokens
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
