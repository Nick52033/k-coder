use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};

use crate::protocol::TokenUsage;
use crate::storage::{StoredEvent, StoredEventKind, ThreadSummary};

pub const DATABASE_SCHEMA_VERSION: u32 = 2;

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

#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    #[error("projection database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("projection lock was poisoned")]
    Poisoned,
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
            "INSERT INTO sessions(id,title,created_at_ms,updated_at_ms,archived,event_count)
             VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(id) DO UPDATE SET title=excluded.title,updated_at_ms=excluded.updated_at_ms,
             archived=excluded.archived,event_count=excluded.event_count",
            params![summary.id, summary.title, summary.created_at_ms, summary.updated_at_ms,
                summary.archived as i64, events.len() as u64],
        )?;
        transaction.execute("DELETE FROM usage WHERE thread_id=?1", [&summary.id])?;
        for event in events {
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

    pub fn list_threads(&self) -> Result<Vec<ThreadSummary>, ProjectionError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let mut statement = connection.prepare(
            "SELECT id,title,created_at_ms,updated_at_ms,archived FROM sessions
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
         VALUES(?1,?2,?3,?4,?5,?6)",
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
    }
}
