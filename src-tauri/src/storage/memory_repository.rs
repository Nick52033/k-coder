//! Versioned fact log and SQLite projection for the memory data layer.
//!
//! Task 1 of the knowledge and memory extension only establishes persistence: the `memories`
//! and `memory_candidates` tables, the append-only fact log under `runtime-data/memory/events.jsonl`,
//! and the deterministic rebuild path. Domain policy (deduplication, conflict resolution, TTL
//! calculation, sensitivity grading and review decisions) belongs to the `memory` module and is
//! deliberately absent here.
//!
//! Ordering follows the design document: every mutation appends the fact event first and only then
//! updates the projection. A failed projection write therefore leaves a durable fact that the next
//! rebuild replays, never a projection row without an auditable event.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::persistence::{ProjectionDb, ProjectionError};
use crate::storage::event_validation::{
    validate_bounded, validate_confidence, validate_id, validate_optional_bounded,
    validate_optional_id, validate_token,
};
use crate::storage::now_ms;

pub const MEMORY_EVENT_SCHEMA_VERSION: u32 = 1;
pub const MEMORY_SCOPE_USER: &str = "user";
pub const MEMORY_STATUS_ACTIVE: &str = "active";
pub const MEMORY_STATUS_DELETED: &str = "deleted";
pub const CANDIDATE_STATUS_PENDING: &str = "pending";

pub const MAX_MEMORY_CONTENT_CHARS: usize = 4_000;
pub const MAX_MEMORY_NORMALIZED_KEY_CHARS: usize = 240;
pub const MAX_MEMORY_REASON_CHARS: usize = 2_000;
pub const MAX_MEMORY_SOURCE_REF_CHARS: usize = 512;
pub const MAX_MEMORY_SCOPE_ID_CHARS: usize = 240;
pub const DEFAULT_MEMORY_PAGE_SIZE: u32 = 100;
pub const MAX_MEMORY_PAGE_SIZE: u32 = 500;

const MEMORY_COLUMNS: &str = "id,scope_type,scope_id,memory_type,normalized_key,content,source_type,\
     source_ref,confidence,sensitivity,status,revision,expires_at_ms,created_at_ms,updated_at_ms";

const CANDIDATE_COLUMNS: &str = "id,operation,target_memory_id,scope_type,scope_id,memory_type,content,\
     normalized_key,reason,confidence,requires_review,status,source_turn_id,created_at_ms,\
     reviewed_at_ms";

/// A projected memory row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecord {
    pub id: String,
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub memory_type: String,
    pub normalized_key: String,
    pub content: String,
    pub source_type: String,
    pub source_ref: Option<String>,
    pub confidence: f64,
    pub sensitivity: String,
    pub status: String,
    pub revision: u64,
    pub expires_at_ms: Option<u64>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// One `(scope, normalized_key)` group holding more than one active row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateKeyGroup {
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub normalized_key: String,
    pub count: u64,
}

/// The payload of a `memory_upserted` fact. The record carries its own `created_at_ms` so a
/// replayed update never rewrites the original creation time; the enclosing event timestamp is the
/// moment the fact was recorded and becomes `updated_at_ms`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryWrite {
    pub id: String,
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub memory_type: String,
    pub normalized_key: String,
    pub content: String,
    pub source_type: String,
    pub source_ref: Option<String>,
    pub confidence: f64,
    pub sensitivity: String,
    pub status: String,
    pub revision: u64,
    pub expires_at_ms: Option<u64>,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatusChange {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCandidateRecord {
    pub id: String,
    pub operation: String,
    pub target_memory_id: Option<String>,
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub memory_type: String,
    pub content: String,
    pub normalized_key: String,
    pub reason: String,
    pub confidence: f64,
    pub requires_review: bool,
    pub status: String,
    pub source_turn_id: Option<String>,
    pub created_at_ms: u64,
    pub reviewed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateReview {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub created_at_ms: u64,
    #[serde(flatten)]
    pub kind: MemoryEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MemoryEventKind {
    #[serde(rename = "memory_upserted")]
    MemoryUpserted(MemoryWrite),
    #[serde(rename = "memory_status_changed")]
    MemoryStatusChanged(MemoryStatusChange),
    #[serde(rename = "memory_candidate_recorded")]
    CandidateRecorded(MemoryCandidateRecord),
    #[serde(rename = "memory_candidate_reviewed")]
    CandidateReviewed(CandidateReview),
}

impl MemoryEvent {
    fn validate(&self) -> Result<(), ProjectionError> {
        if self.schema_version != MEMORY_EVENT_SCHEMA_VERSION {
            return Err(ProjectionError::InvalidData(format!(
                "unsupported memory event schema {}",
                self.schema_version
            )));
        }
        validate_id(&self.event_id, "eventId")?;
        match &self.kind {
            MemoryEventKind::MemoryUpserted(write) => write.validate(),
            MemoryEventKind::MemoryStatusChanged(change) => {
                validate_id(&change.id, "id")?;
                validate_token(&change.status, "status")
            }
            MemoryEventKind::CandidateRecorded(candidate) => candidate.validate(),
            MemoryEventKind::CandidateReviewed(review) => {
                validate_id(&review.id, "id")?;
                validate_token(&review.status, "status")
            }
        }
    }
}

impl MemoryWrite {
    fn validate(&self) -> Result<(), ProjectionError> {
        validate_id(&self.id, "id")?;
        validate_token(&self.scope_type, "scopeType")?;
        validate_token(&self.memory_type, "memoryType")?;
        validate_token(&self.source_type, "sourceType")?;
        validate_token(&self.sensitivity, "sensitivity")?;
        validate_token(&self.status, "status")?;
        validate_optional_bounded(
            self.scope_id.as_deref(),
            "scopeId",
            MAX_MEMORY_SCOPE_ID_CHARS,
        )?;
        validate_bounded(
            &self.normalized_key,
            "normalizedKey",
            1,
            MAX_MEMORY_NORMALIZED_KEY_CHARS,
        )?;
        validate_bounded(&self.content, "content", 1, MAX_MEMORY_CONTENT_CHARS)?;
        validate_optional_bounded(
            self.source_ref.as_deref(),
            "sourceRef",
            MAX_MEMORY_SOURCE_REF_CHARS,
        )?;
        validate_confidence(self.confidence)?;
        if self.revision < 1 {
            return Err(ProjectionError::InvalidData(
                "revision must be at least 1".into(),
            ));
        }
        Ok(())
    }
}

impl MemoryCandidateRecord {
    fn validate(&self) -> Result<(), ProjectionError> {
        validate_id(&self.id, "id")?;
        validate_token(&self.operation, "operation")?;
        validate_token(&self.scope_type, "scopeType")?;
        validate_token(&self.memory_type, "memoryType")?;
        validate_token(&self.status, "status")?;
        validate_optional_id(self.target_memory_id.as_deref(), "targetMemoryId")?;
        validate_optional_id(self.source_turn_id.as_deref(), "sourceTurnId")?;
        validate_optional_bounded(
            self.scope_id.as_deref(),
            "scopeId",
            MAX_MEMORY_SCOPE_ID_CHARS,
        )?;
        validate_bounded(
            &self.normalized_key,
            "normalizedKey",
            1,
            MAX_MEMORY_NORMALIZED_KEY_CHARS,
        )?;
        validate_bounded(&self.content, "content", 1, MAX_MEMORY_CONTENT_CHARS)?;
        validate_bounded(&self.reason, "reason", 1, MAX_MEMORY_REASON_CHARS)?;
        validate_confidence(self.confidence)?;
        Ok(())
    }
}

/// Deterministic dedup key derived from content only. Task 2 owns the final normalization rules;
/// this keeps the legacy backfill reproducible.
pub fn normalize_memory_key(content: &str) -> String {
    let mut key = String::new();
    let mut pending_space = false;
    for character in content.trim().chars() {
        if character.is_whitespace() {
            pending_space = !key.is_empty();
            continue;
        }
        if pending_space {
            key.push(' ');
            pending_space = false;
        }
        for lowered in character.to_lowercase() {
            key.push(lowered);
        }
        if key.chars().count() >= MAX_MEMORY_NORMALIZED_KEY_CHARS {
            break;
        }
    }
    key
}

#[derive(Clone)]
pub struct MemoryRepository {
    db: ProjectionDb,
    events_path: Option<PathBuf>,
    legacy_path: Option<PathBuf>,
    append_lock: Arc<Mutex<()>>,
}

impl MemoryRepository {
    pub fn new(db: ProjectionDb) -> Self {
        let events_path = db.data_root().map(|root| root.join("memory/events.jsonl"));
        let legacy_path = db
            .data_root()
            .map(|root| root.join("advanced/memories.jsonl"));
        Self {
            db,
            events_path,
            legacy_path,
            append_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn db(&self) -> &ProjectionDb {
        &self.db
    }

    /// Appends the fact event and only then updates the projection.
    pub fn append(&self, kind: MemoryEventKind) -> Result<(), ProjectionError> {
        let event = MemoryEvent {
            schema_version: MEMORY_EVENT_SCHEMA_VERSION,
            event_id: Uuid::new_v4().to_string(),
            created_at_ms: now_ms(),
            kind,
        };
        self.append_event(&event)?;
        self.apply_event(&event)
    }

    fn append_event(&self, event: &MemoryEvent) -> Result<(), ProjectionError> {
        let Some(path) = self.events_path.as_ref() else {
            return Ok(());
        };
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let parent = path.parent().ok_or_else(|| {
            ProjectionError::InvalidData("memory event path has no parent".into())
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            ProjectionError::InvalidData(format!("create memory event directory: {error}"))
        })?;
        let line = serde_json::to_string(event).map_err(|error| {
            ProjectionError::InvalidData(format!("serialize memory event: {error}"))
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| {
                ProjectionError::InvalidData(format!("open memory event log: {error}"))
            })?;
        file.write_all(line.as_bytes())
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_data())
            .map_err(|error| ProjectionError::InvalidData(format!("append memory event: {error}")))
    }

    /// Rebuilds the whole memory projection from the fact log. Every record is parsed and validated
    /// before any row is touched, so a corrupted log closes the rebuild without emptying the
    /// projection. When no log exists yet the legacy `advanced/memories.jsonl` is folded in once.
    pub fn rebuild_projection(&self) -> Result<(), ProjectionError> {
        let Some(path) = self.events_path.as_ref() else {
            return Ok(());
        };
        if !path.exists() {
            return self.backfill_legacy_memories();
        }
        let content = fs::read_to_string(path).map_err(|error| {
            ProjectionError::InvalidData(format!("read memory event log: {error}"))
        })?;
        if content.trim().is_empty() {
            return self.backfill_legacy_memories();
        }
        let lines = content.split('\n').collect::<Vec<_>>();
        let has_trailing_newline = content.ends_with('\n');
        let mut events = Vec::new();
        for (index, raw_line) in lines.iter().enumerate() {
            let line = raw_line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let event = match serde_json::from_str::<MemoryEvent>(line) {
                Ok(event) => event,
                Err(error) if !has_trailing_newline && index + 1 == lines.len() => {
                    // A process can be interrupted after writing a partial final line. Earlier
                    // durable facts stay valid and the next mutation appends a clean record.
                    let _ = error;
                    break;
                }
                Err(error) => {
                    return Err(ProjectionError::InvalidData(format!(
                        "invalid memory event at line {}: {error}",
                        index + 1
                    )));
                }
            };
            event.validate()?;
            events.push(event);
        }
        self.clear_projection()?;
        for event in &events {
            self.apply_event(event)?;
        }
        Ok(())
    }

    fn clear_projection(&self) -> Result<(), ProjectionError> {
        self.db.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute("DELETE FROM memory_candidates", [])?;
            transaction.execute("DELETE FROM memories", [])?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// One-time read-only projection of the legacy `advanced/memories.jsonl` log. The original file
    /// is never modified or deleted, and the newest revision per memory id wins, matching the legacy
    /// `MemoryStore::latest_unlocked` semantics.
    fn backfill_legacy_memories(&self) -> Result<(), ProjectionError> {
        let Some(path) = self.legacy_path.as_ref() else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(path).map_err(|error| {
            ProjectionError::InvalidData(format!("read legacy memory log: {error}"))
        })?;
        let mut latest = HashMap::<String, LegacyMemoryView>::new();
        for raw_line in content.split('\n') {
            let line = raw_line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let memory = serde_json::from_str::<LegacyMemoryView>(line).map_err(|error| {
                ProjectionError::InvalidData(format!("invalid legacy memory record: {error}"))
            })?;
            if memory.id.trim().is_empty() {
                return Err(ProjectionError::InvalidData(
                    "legacy memory record has an empty id".into(),
                ));
            }
            let replace = latest
                .get(&memory.id)
                .is_none_or(|current| current.revision < memory.revision);
            if replace {
                latest.insert(memory.id.clone(), memory);
            }
        }
        let mut ids = latest.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        for id in ids {
            let legacy = latest.remove(&id).expect("legacy memory was collected");
            let legacy_updated_at_ms = legacy.updated_at_ms;
            let write = legacy.into_write();
            let event = MemoryEvent {
                schema_version: MEMORY_EVENT_SCHEMA_VERSION,
                event_id: Uuid::new_v4().to_string(),
                created_at_ms: write.created_at_ms.max(legacy_updated_at_ms),
                kind: MemoryEventKind::MemoryUpserted(write),
            };
            self.append_event(&event)?;
            self.apply_event(&event)?;
        }
        Ok(())
    }

    fn apply_event(&self, event: &MemoryEvent) -> Result<(), ProjectionError> {
        event.validate()?;
        self.db
            .with_connection(|connection| {
                let transaction = connection.transaction()?;
                match &event.kind {
                    MemoryEventKind::MemoryUpserted(write) => {
                        transaction.execute(
                            "INSERT INTO memories(id,scope_type,scope_id,memory_type,normalized_key,
                               content,source_type,source_ref,confidence,sensitivity,status,revision,
                               expires_at_ms,created_at_ms,updated_at_ms)
                             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                             ON CONFLICT(id) DO UPDATE SET scope_type=excluded.scope_type,
                               scope_id=excluded.scope_id,memory_type=excluded.memory_type,
                               normalized_key=excluded.normalized_key,content=excluded.content,
                               source_type=excluded.source_type,source_ref=excluded.source_ref,
                               confidence=excluded.confidence,sensitivity=excluded.sensitivity,
                               status=excluded.status,revision=excluded.revision,
                               expires_at_ms=excluded.expires_at_ms,
                               updated_at_ms=excluded.updated_at_ms",
                            params![
                                write.id,
                                write.scope_type,
                                write.scope_id,
                                write.memory_type,
                                write.normalized_key,
                                write.content,
                                write.source_type,
                                write.source_ref,
                                write.confidence,
                                write.sensitivity,
                                write.status,
                                write.revision as i64,
                                write.expires_at_ms.map(|value| value as i64),
                                write.created_at_ms as i64,
                                event.created_at_ms as i64,
                            ],
                        )?;
                    }
                    MemoryEventKind::MemoryStatusChanged(change) => {
                        let changed = transaction.execute(
                            "UPDATE memories SET status=?2,updated_at_ms=?3 WHERE id=?1",
                            params![change.id, change.status, event.created_at_ms as i64],
                        )?;
                        if changed == 0 {
                            return Err(rusqlite::Error::QueryReturnedNoRows);
                        }
                    }
                    MemoryEventKind::CandidateRecorded(candidate) => {
                        transaction.execute(
                            "INSERT INTO memory_candidates(id,operation,target_memory_id,scope_type,
                               scope_id,memory_type,content,normalized_key,reason,confidence,
                               requires_review,status,source_turn_id,created_at_ms,reviewed_at_ms)
                             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,NULL)
                             ON CONFLICT(id) DO UPDATE SET operation=excluded.operation,
                               target_memory_id=excluded.target_memory_id,
                               scope_type=excluded.scope_type,scope_id=excluded.scope_id,
                               memory_type=excluded.memory_type,content=excluded.content,
                               normalized_key=excluded.normalized_key,reason=excluded.reason,
                               confidence=excluded.confidence,
                               requires_review=excluded.requires_review,status=excluded.status,
                               source_turn_id=excluded.source_turn_id",
                            params![
                                candidate.id,
                                candidate.operation,
                                candidate.target_memory_id,
                                candidate.scope_type,
                                candidate.scope_id,
                                candidate.memory_type,
                                candidate.content,
                                candidate.normalized_key,
                                candidate.reason,
                                candidate.confidence,
                                candidate.requires_review as i64,
                                candidate.status,
                                candidate.source_turn_id,
                                candidate.created_at_ms as i64,
                            ],
                        )?;
                    }
                    MemoryEventKind::CandidateReviewed(review) => {
                        let changed = transaction.execute(
                            "UPDATE memory_candidates SET status=?2,reviewed_at_ms=?3 WHERE id=?1",
                            params![review.id, review.status, event.created_at_ms as i64],
                        )?;
                        if changed == 0 {
                            return Err(rusqlite::Error::QueryReturnedNoRows);
                        }
                    }
                }
                transaction.commit()?;
                Ok(())
            })
            .map_err(|error| match error {
                ProjectionError::Database(rusqlite::Error::QueryReturnedNoRows) => {
                    ProjectionError::InvalidData(
                        "memory event targets a record that does not exist".into(),
                    )
                }
                other => other,
            })
    }

    pub fn get(&self, id: &str) -> Result<Option<MemoryRecord>, ProjectionError> {
        self.db.with_connection(|connection| {
            connection
                .query_row(
                    &format!("SELECT {MEMORY_COLUMNS} FROM memories WHERE id=?1"),
                    [id],
                    map_memory,
                )
                .optional()
        })
    }

    pub fn list(
        &self,
        scope_type: &str,
        scope_id: Option<&str>,
        status: &str,
        limit: u32,
    ) -> Result<Vec<MemoryRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_MEMORY_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {MEMORY_COLUMNS} FROM memories
                 WHERE scope_type=?1 AND (?2 IS NULL OR scope_id=?2) AND status=?3
                 ORDER BY updated_at_ms DESC,id ASC LIMIT ?4"
            ))?;
            let rows =
                statement.query_map(params![scope_type, scope_id, status, limit], map_memory)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn count(&self, scope_type: &str, status: &str) -> Result<u64, ProjectionError> {
        self.db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM memories WHERE scope_type=?1 AND status=?2",
                    params![scope_type, status],
                    |row| row.get::<_, i64>(0),
                )
            })
            .map(|count| count as u64)
    }

    /// Counts one scope, including its id, so the settings page can show a per-scope total.
    pub fn count_in_scope(
        &self,
        scope_type: &str,
        scope_id: Option<&str>,
        status: &str,
    ) -> Result<u64, ProjectionError> {
        self.db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM memories
                     WHERE scope_type=?1 AND (?2 IS NULL OR scope_id=?2) AND status=?3",
                    params![scope_type, scope_id, status],
                    |row| row.get::<_, i64>(0),
                )
            })
            .map(|count| count as u64)
    }

    /// Keyset page ordered by `(updated_at_ms DESC, id ASC)`.
    ///
    /// A keyset cursor is used instead of `OFFSET` because memories keep being written while the
    /// settings page is open: an offset would skip or repeat rows as `updated_at_ms` changes, while
    /// the `(timestamp, id)` pair stays stable.
    pub fn list_page(
        &self,
        scope_type: &str,
        scope_id: Option<&str>,
        status: &str,
        after: Option<(u64, &str)>,
        limit: u32,
    ) -> Result<Vec<MemoryRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_MEMORY_PAGE_SIZE);
        let after_timestamp = after.map(|(updated_at_ms, _)| updated_at_ms as i64);
        let after_id = after.map(|(_, id)| id);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {MEMORY_COLUMNS} FROM memories
                 WHERE scope_type=?1 AND (?2 IS NULL OR scope_id=?2) AND status=?3
                   AND (?4 IS NULL OR updated_at_ms < ?4 OR (updated_at_ms = ?4 AND id > ?5))
                 ORDER BY updated_at_ms DESC,id ASC LIMIT ?6"
            ))?;
            let rows = statement.query_map(
                params![
                    scope_type,
                    scope_id,
                    status,
                    after_timestamp,
                    after_id,
                    limit
                ],
                map_memory,
            )?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// Every projected row sharing a deduplication key, newest first. Backed by
    /// `memories_normalized_key`, so the memory service can classify a write as a duplicate, a
    /// conflict or a cross-scope update without scanning the table.
    pub fn list_by_key(
        &self,
        normalized_key: &str,
        limit: u32,
    ) -> Result<Vec<MemoryRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_MEMORY_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {MEMORY_COLUMNS} FROM memories WHERE normalized_key=?1
                 ORDER BY updated_at_ms DESC,id ASC LIMIT ?2"
            ))?;
            let rows = statement.query_map(params![normalized_key, limit], map_memory)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// Every active row in `id` order, for maintenance sweeps.
    ///
    /// Ordered by id rather than by `updated_at_ms` on purpose: the expiry sweep rewrites
    /// `updated_at_ms` as it goes, and a keyset over a column the sweep mutates would skip rows.
    pub fn list_active_by_id(
        &self,
        after_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<MemoryRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_MEMORY_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {MEMORY_COLUMNS} FROM memories
                 WHERE status=?1 AND (?2 IS NULL OR id > ?2)
                 ORDER BY id ASC LIMIT ?3"
            ))?;
            let rows =
                statement.query_map(params![MEMORY_STATUS_ACTIVE, after_id, limit], map_memory)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// The newest active rows across every scope, for bounded maintenance input.
    ///
    /// Unlike `list_active_by_id`, this is a single bounded page ordered by recency: the maintenance
    /// prompt lists at most a few dozen memories, and "what the user touched last" is the useful
    /// slice. It is a read-only helper — nothing in the maintenance path depends on its ordering.
    pub fn list_recent_active(&self, limit: u32) -> Result<Vec<MemoryRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_MEMORY_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {MEMORY_COLUMNS} FROM memories
                 WHERE status=?1
                 ORDER BY updated_at_ms DESC, id ASC LIMIT ?2"
            ))?;
            let rows = statement.query_map(params![MEMORY_STATUS_ACTIVE, limit], map_memory)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// Groups of active rows that share one deduplication key inside one scope.
    ///
    /// A healthy projection has one row per `(scope, normalized_key)`, so this normally returns
    /// nothing. It can still find real duplicates: the Task 1 backfill mapped legacy memories by id,
    /// and two legacy rows whose content normalizes to the same key become two rows here. The memory
    /// service resolves those groups; storage only reports them.
    pub fn duplicate_key_groups(
        &self,
        limit: u32,
    ) -> Result<Vec<DuplicateKeyGroup>, ProjectionError> {
        let limit = limit.clamp(1, MAX_MEMORY_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT scope_type,scope_id,normalized_key,COUNT(*) FROM memories
                 WHERE status=?1
                 GROUP BY scope_type,scope_id,normalized_key
                 HAVING COUNT(*) > 1
                 ORDER BY scope_type ASC,scope_id ASC,normalized_key ASC LIMIT ?2",
            )?;
            let rows = statement.query_map(params![MEMORY_STATUS_ACTIVE, limit], |row| {
                Ok(DuplicateKeyGroup {
                    scope_type: row.get(0)?,
                    scope_id: row.get(1)?,
                    normalized_key: row.get(2)?,
                    count: row.get::<_, i64>(3)?.max(0) as u64,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// Every active row sharing one deduplication key inside one scope, newest revision first.
    pub fn list_active_by_key_in_scope(
        &self,
        scope_type: &str,
        scope_id: Option<&str>,
        normalized_key: &str,
        limit: u32,
    ) -> Result<Vec<MemoryRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_MEMORY_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {MEMORY_COLUMNS} FROM memories
                 WHERE status=?1 AND scope_type=?2 AND (?3 IS NULL OR scope_id=?3)
                   AND normalized_key=?4
                 ORDER BY updated_at_ms DESC,revision DESC,id ASC LIMIT ?5"
            ))?;
            let rows = statement.query_map(
                params![
                    MEMORY_STATUS_ACTIVE,
                    scope_type,
                    scope_id,
                    normalized_key,
                    limit
                ],
                map_memory,
            )?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn get_candidate(
        &self,
        id: &str,
    ) -> Result<Option<MemoryCandidateRecord>, ProjectionError> {
        self.db.with_connection(|connection| {
            connection
                .query_row(
                    &format!("SELECT {CANDIDATE_COLUMNS} FROM memory_candidates WHERE id=?1"),
                    [id],
                    map_candidate,
                )
                .optional()
        })
    }

    pub fn list_candidates(
        &self,
        status: &str,
        limit: u32,
    ) -> Result<Vec<MemoryCandidateRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_MEMORY_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {CANDIDATE_COLUMNS} FROM memory_candidates
                 WHERE status=?1 ORDER BY created_at_ms ASC,id ASC LIMIT ?2"
            ))?;
            let rows = statement.query_map(params![status, limit], map_candidate)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }
}

fn map_memory(row: &rusqlite::Row<'_>) -> Result<MemoryRecord, rusqlite::Error> {
    Ok(MemoryRecord {
        id: row.get(0)?,
        scope_type: row.get(1)?,
        scope_id: row.get(2)?,
        memory_type: row.get(3)?,
        normalized_key: row.get(4)?,
        content: row.get(5)?,
        source_type: row.get(6)?,
        source_ref: row.get(7)?,
        confidence: row.get(8)?,
        sensitivity: row.get(9)?,
        status: row.get(10)?,
        revision: row.get::<_, i64>(11)?.max(0) as u64,
        expires_at_ms: row
            .get::<_, Option<i64>>(12)?
            .map(|value| value.max(0) as u64),
        created_at_ms: row.get::<_, i64>(13)?.max(0) as u64,
        updated_at_ms: row.get::<_, i64>(14)?.max(0) as u64,
    })
}

fn map_candidate(row: &rusqlite::Row<'_>) -> Result<MemoryCandidateRecord, rusqlite::Error> {
    Ok(MemoryCandidateRecord {
        id: row.get(0)?,
        operation: row.get(1)?,
        target_memory_id: row.get(2)?,
        scope_type: row.get(3)?,
        scope_id: row.get(4)?,
        memory_type: row.get(5)?,
        content: row.get(6)?,
        normalized_key: row.get(7)?,
        reason: row.get(8)?,
        confidence: row.get(9)?,
        requires_review: row.get::<_, i64>(10)? != 0,
        status: row.get(11)?,
        source_turn_id: row.get(12)?,
        created_at_ms: row.get::<_, i64>(13)?.max(0) as u64,
        reviewed_at_ms: row
            .get::<_, Option<i64>>(14)?
            .map(|value| value.max(0) as u64),
    })
}

/// Legacy `advanced/memories.jsonl` record, kept read-only for compatibility.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyMemoryView {
    #[serde(default)]
    id: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    expires_at_ms: u64,
    #[serde(default)]
    created_at_ms: u64,
    #[serde(default)]
    updated_at_ms: u64,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    revision: u64,
}

impl LegacyMemoryView {
    fn into_write(self) -> MemoryWrite {
        let source = self.source.trim().to_owned();
        let created_at_ms = if self.created_at_ms == 0 {
            if self.updated_at_ms == 0 {
                now_ms()
            } else {
                self.updated_at_ms
            }
        } else {
            self.created_at_ms
        };
        MemoryWrite {
            id: self.id,
            // Legacy memories were global user notes created through the user-facing `remember`
            // tool, so they project into the user scope with user provenance.
            scope_type: MEMORY_SCOPE_USER.to_owned(),
            scope_id: None,
            memory_type: "fact".to_owned(),
            normalized_key: normalize_memory_key(&self.content),
            content: self.content,
            source_type: "user".to_owned(),
            source_ref: (!source.is_empty()).then_some(source),
            confidence: 1.0,
            sensitivity: "normal".to_owned(),
            status: if self.deleted {
                MEMORY_STATUS_DELETED.to_owned()
            } else {
                MEMORY_STATUS_ACTIVE.to_owned()
            },
            revision: self.revision.max(1),
            expires_at_ms: (self.expires_at_ms > 0).then_some(self.expires_at_ms),
            created_at_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository(data_root: &std::path::Path) -> MemoryRepository {
        MemoryRepository::new(ProjectionDb::open(data_root).unwrap())
    }

    fn write(id: &str, content: &str, revision: u64) -> MemoryWrite {
        MemoryWrite {
            id: id.to_owned(),
            scope_type: MEMORY_SCOPE_USER.to_owned(),
            scope_id: None,
            memory_type: "preference".to_owned(),
            normalized_key: normalize_memory_key(content),
            content: content.to_owned(),
            source_type: "user".to_owned(),
            source_ref: Some("settings".to_owned()),
            confidence: 0.9,
            sensitivity: "normal".to_owned(),
            status: MEMORY_STATUS_ACTIVE.to_owned(),
            revision,
            expires_at_ms: None,
            created_at_ms: 1_000,
        }
    }

    #[test]
    fn appending_a_memory_updates_the_projection_and_the_fact_log() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        repository
            .append(MemoryEventKind::MemoryUpserted(write(
                "memory-1",
                "Use pnpm for scripts",
                1,
            )))
            .unwrap();

        let stored = repository.get("memory-1").unwrap().unwrap();
        assert_eq!(stored.content, "Use pnpm for scripts");
        assert_eq!(stored.revision, 1);
        assert_eq!(stored.status, MEMORY_STATUS_ACTIVE);
        assert_eq!(
            repository
                .count(MEMORY_SCOPE_USER, MEMORY_STATUS_ACTIVE)
                .unwrap(),
            1
        );

        let log = fs::read_to_string(data_root.path().join("memory/events.jsonl")).unwrap();
        assert!(log.contains("memory_upserted"));
        assert!(log.contains("Use pnpm for scripts"));
    }

    #[test]
    fn memory_events_rebuild_the_projection_after_the_tables_are_emptied() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        repository
            .append(MemoryEventKind::MemoryUpserted(write(
                "memory-1",
                "Prefer pnpm",
                1,
            )))
            .unwrap();
        repository
            .append(MemoryEventKind::MemoryUpserted(write(
                "memory-1",
                "Prefer pnpm workspaces",
                2,
            )))
            .unwrap();
        repository
            .append(MemoryEventKind::CandidateRecorded(MemoryCandidateRecord {
                id: "candidate-1".to_owned(),
                operation: "create".to_owned(),
                target_memory_id: None,
                scope_type: MEMORY_SCOPE_USER.to_owned(),
                scope_id: None,
                memory_type: "preference".to_owned(),
                content: "Prefer pnpm".to_owned(),
                normalized_key: normalize_memory_key("Prefer pnpm"),
                reason: "the user stated a tool preference".to_owned(),
                confidence: 0.7,
                requires_review: true,
                status: CANDIDATE_STATUS_PENDING.to_owned(),
                source_turn_id: Some("turn-1".to_owned()),
                created_at_ms: 5,
                reviewed_at_ms: None,
            }))
            .unwrap();

        repository.clear_projection().unwrap();
        assert!(repository.get("memory-1").unwrap().is_none());
        assert!(
            repository
                .list_candidates(CANDIDATE_STATUS_PENDING, 10)
                .unwrap()
                .is_empty()
        );

        repository.rebuild_projection().unwrap();

        let stored = repository.get("memory-1").unwrap().unwrap();
        assert_eq!(stored.content, "Prefer pnpm workspaces");
        assert_eq!(stored.revision, 2);
        assert_eq!(
            stored.created_at_ms, 1_000,
            "replay must keep the original creation time"
        );
        let candidates = repository
            .list_candidates(CANDIDATE_STATUS_PENDING, 10)
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].requires_review);
        assert_eq!(candidates[0].source_turn_id.as_deref(), Some("turn-1"));
    }

    #[test]
    fn reviewing_a_candidate_records_the_decision_without_touching_creation_time() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        repository
            .append(MemoryEventKind::CandidateRecorded(MemoryCandidateRecord {
                id: "candidate-1".to_owned(),
                operation: "delete".to_owned(),
                target_memory_id: Some("memory-1".to_owned()),
                scope_type: MEMORY_SCOPE_USER.to_owned(),
                scope_id: None,
                memory_type: "fact".to_owned(),
                content: "Remove the stale note".to_owned(),
                normalized_key: normalize_memory_key("Remove the stale note"),
                reason: "the note contradicts the current workspace".to_owned(),
                confidence: 0.5,
                requires_review: true,
                status: CANDIDATE_STATUS_PENDING.to_owned(),
                source_turn_id: None,
                created_at_ms: 11,
                reviewed_at_ms: None,
            }))
            .unwrap();
        repository
            .append(MemoryEventKind::CandidateReviewed(CandidateReview {
                id: "candidate-1".to_owned(),
                status: "rejected".to_owned(),
            }))
            .unwrap();

        let stored = repository.get_candidate("candidate-1").unwrap().unwrap();
        assert_eq!(stored.status, "rejected");
        assert_eq!(stored.created_at_ms, 11);
        assert!(stored.reviewed_at_ms.is_some());
        assert!(
            repository
                .list_candidates(CANDIDATE_STATUS_PENDING, 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn deleting_a_memory_keeps_an_auditable_row_instead_of_purging_it() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        repository
            .append(MemoryEventKind::MemoryUpserted(write(
                "memory-1",
                "Temporary",
                1,
            )))
            .unwrap();
        repository
            .append(MemoryEventKind::MemoryStatusChanged(MemoryStatusChange {
                id: "memory-1".to_owned(),
                status: MEMORY_STATUS_DELETED.to_owned(),
            }))
            .unwrap();

        let stored = repository.get("memory-1").unwrap().unwrap();
        assert_eq!(stored.status, MEMORY_STATUS_DELETED);
        assert_eq!(
            repository
                .count(MEMORY_SCOPE_USER, MEMORY_STATUS_ACTIVE)
                .unwrap(),
            0
        );
    }

    #[test]
    fn legacy_memory_jsonl_is_projected_once_without_being_deleted() {
        let data_root = tempfile::tempdir().unwrap();
        let legacy_dir = data_root.path().join("advanced");
        fs::create_dir_all(&legacy_dir).unwrap();
        let legacy_path = legacy_dir.join("memories.jsonl");
        fs::write(
            &legacy_path,
            concat!(
                "{\"schemaVersion\":10,\"id\":\"legacy-1\",\"content\":\"Use pnpm\",\"source\":\"user instruction\",",
                "\"expiresAtMs\":1000,\"createdAtMs\":10,\"updatedAtMs\":20,\"deleted\":false,\"revision\":1}\n",
                "{\"schemaVersion\":10,\"id\":\"legacy-1\",\"content\":\"Use pnpm workspaces\",\"source\":\"user instruction\",",
                "\"expiresAtMs\":1000,\"createdAtMs\":10,\"updatedAtMs\":30,\"deleted\":false,\"revision\":2}\n",
                "{\"schemaVersion\":10,\"id\":\"legacy-2\",\"content\":\"Retired note\",\"source\":\"user instruction\",",
                "\"expiresAtMs\":1000,\"createdAtMs\":11,\"updatedAtMs\":12,\"deleted\":true,\"revision\":2}\n"
            ),
        )
        .unwrap();

        let repository = repository(data_root.path());
        repository.rebuild_projection().unwrap();

        let newest = repository.get("legacy-1").unwrap().unwrap();
        assert_eq!(newest.content, "Use pnpm workspaces");
        assert_eq!(newest.revision, 2);
        assert_eq!(newest.scope_type, MEMORY_SCOPE_USER);
        assert_eq!(newest.source_type, "user");
        assert_eq!(newest.source_ref.as_deref(), Some("user instruction"));
        assert_eq!(newest.created_at_ms, 10);
        assert_eq!(newest.updated_at_ms, 30);
        assert_eq!(newest.expires_at_ms, Some(1000));
        assert_eq!(
            repository.get("legacy-2").unwrap().unwrap().status,
            MEMORY_STATUS_DELETED
        );
        assert!(legacy_path.exists(), "the legacy log must stay readable");

        // The backfill wrote facts, so a second rebuild replays the log instead of re-importing.
        repository.rebuild_projection().unwrap();
        assert_eq!(
            repository
                .count(MEMORY_SCOPE_USER, MEMORY_STATUS_ACTIVE)
                .unwrap(),
            1
        );
        assert_eq!(
            repository
                .count(MEMORY_SCOPE_USER, MEMORY_STATUS_DELETED)
                .unwrap(),
            1
        );
        let log = fs::read_to_string(data_root.path().join("memory/events.jsonl")).unwrap();
        assert_eq!(log.matches("memory_upserted").count(), 2);
    }

    #[test]
    fn a_corrupted_fact_log_closes_the_rebuild_without_emptying_the_projection() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        repository
            .append(MemoryEventKind::MemoryUpserted(write(
                "memory-1", "Keep me", 1,
            )))
            .unwrap();

        let log_path = data_root.path().join("memory/events.jsonl");
        let mut log = fs::read_to_string(&log_path).unwrap();
        log.push_str("{\"schemaVersion\":1,\"eventId\":\"broken\"\n");
        log.push_str("{\"schemaVersion\":9,\"eventId\":\"future\",\"createdAtMs\":1,\"type\":\"memory_status_changed\",\"data\":{\"id\":\"memory-1\",\"status\":\"deleted\"}}\n");
        fs::write(&log_path, log).unwrap();

        assert!(repository.rebuild_projection().is_err());
        assert!(repository.get("memory-1").unwrap().is_some());
    }

    #[test]
    fn invalid_memory_payloads_are_rejected_before_any_row_is_written() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());

        let mut blank = write("memory-1", "   ", 1);
        blank.content = String::new();
        assert!(matches!(
            repository.append(MemoryEventKind::MemoryUpserted(blank)),
            Err(ProjectionError::InvalidData(_))
        ));

        let mut infinite = write("memory-2", "content", 1);
        infinite.confidence = f64::NAN;
        assert!(matches!(
            repository.append(MemoryEventKind::MemoryUpserted(infinite)),
            Err(ProjectionError::InvalidData(_))
        ));

        let mut zero_revision = write("memory-3", "content", 0);
        zero_revision.revision = 0;
        assert!(matches!(
            repository.append(MemoryEventKind::MemoryUpserted(zero_revision)),
            Err(ProjectionError::InvalidData(_))
        ));

        let mut bad_scope = write("memory-4", "content", 1);
        bad_scope.scope_type = "User Scope".to_owned();
        assert!(matches!(
            repository.append(MemoryEventKind::MemoryUpserted(bad_scope)),
            Err(ProjectionError::InvalidData(_))
        ));

        assert_eq!(
            repository
                .count(MEMORY_SCOPE_USER, MEMORY_STATUS_ACTIVE)
                .unwrap(),
            0
        );
    }

    #[test]
    fn status_changes_for_unknown_memories_fail_instead_of_creating_rows() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        let error = repository
            .append(MemoryEventKind::MemoryStatusChanged(MemoryStatusChange {
                id: "missing".to_owned(),
                status: MEMORY_STATUS_DELETED.to_owned(),
            }))
            .unwrap_err();
        assert!(matches!(error, ProjectionError::InvalidData(_)));
        assert_eq!(
            repository
                .count(MEMORY_SCOPE_USER, MEMORY_STATUS_DELETED)
                .unwrap(),
            0
        );
    }

    #[test]
    fn normalize_memory_key_is_deterministic_and_bounded() {
        assert_eq!(normalize_memory_key("  Use   PNPM\n"), "use pnpm");
        let long = "a".repeat(MAX_MEMORY_NORMALIZED_KEY_CHARS * 2);
        assert_eq!(
            normalize_memory_key(&long).chars().count(),
            MAX_MEMORY_NORMALIZED_KEY_CHARS
        );
    }

    /// Writes fact events with explicit timestamps so pagination order stays deterministic; the
    /// public `append` stamps `now_ms()` and would make two writes race on the same millisecond.
    fn append_raw(data_root: &std::path::Path, lines: &[String]) {
        let path = data_root.join("memory/events.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        for line in lines {
            file.write_all(line.as_bytes()).unwrap();
            file.write_all(b"\n").unwrap();
        }
    }

    fn raw_upsert(
        id: &str,
        scope_type: &str,
        scope_id: Option<&str>,
        content: &str,
        updated_at_ms: u64,
    ) -> String {
        serde_json::json!({
            "schemaVersion": MEMORY_EVENT_SCHEMA_VERSION,
            "eventId": format!("event-{id}"),
            "createdAtMs": updated_at_ms,
            "type": "memory_upserted",
            "data": {
                "id": id,
                "scopeType": scope_type,
                "scopeId": scope_id,
                "memoryType": "fact",
                "normalizedKey": normalize_memory_key(content),
                "content": content,
                "sourceType": "user",
                "sourceRef": null,
                "confidence": 1.0,
                "sensitivity": "normal",
                "status": "active",
                "revision": 1,
                "expiresAtMs": null,
                "createdAtMs": updated_at_ms,
            }
        })
        .to_string()
    }

    #[test]
    fn the_keyset_page_walks_every_row_once_in_a_stable_order() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        append_raw(
            data_root.path(),
            &[
                raw_upsert("memory-a", "project", Some("proj-1"), "first", 300),
                raw_upsert("memory-b", "project", Some("proj-1"), "second", 300),
                raw_upsert("memory-c", "project", Some("proj-1"), "third", 200),
                raw_upsert("memory-d", "project", Some("proj-2"), "other scope", 400),
                raw_upsert("memory-e", "user", None, "global", 400),
            ],
        );
        repository.rebuild_projection().unwrap();

        let mut seen = Vec::new();
        let mut cursor: Option<(u64, String)> = None;
        loop {
            let after = cursor
                .as_ref()
                .map(|(timestamp, id)| (*timestamp, id.as_str()));
            let page = repository
                .list_page("project", Some("proj-1"), "active", after, 2)
                .unwrap();
            if page.is_empty() {
                break;
            }
            let exhausted = page.len() < 2;
            for record in &page {
                seen.push(record.id.clone());
            }
            let last = page.last().expect("a non-empty page has a last row");
            cursor = Some((last.updated_at_ms, last.id.clone()));
            if exhausted {
                break;
            }
        }
        // `memory-a` and `memory-b` share a timestamp, so the id tie-break must keep them ordered.
        assert_eq!(seen, vec!["memory-a", "memory-b", "memory-c"]);

        assert_eq!(
            repository
                .count_in_scope("project", Some("proj-1"), "active")
                .unwrap(),
            3
        );
        assert_eq!(
            repository
                .count_in_scope("project", Some("proj-2"), "active")
                .unwrap(),
            1
        );
        assert_eq!(
            repository.count_in_scope("user", None, "active").unwrap(),
            1
        );
    }

    #[test]
    fn key_lookups_return_every_scope_newest_first() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        append_raw(
            data_root.path(),
            &[
                raw_upsert("memory-a", "project", Some("proj-1"), "Prefer pnpm", 300),
                raw_upsert("memory-b", "user", None, "Prefer pnpm", 200),
                raw_upsert("memory-c", "user", None, "A different note", 100),
            ],
        );
        repository.rebuild_projection().unwrap();

        let rows = repository
            .list_by_key(&normalize_memory_key("prefer pnpm"), 10)
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "memory-a");
        assert_eq!(rows[0].scope_type, "project");
        assert_eq!(rows[1].id, "memory-b");
        assert_eq!(rows[1].scope_type, MEMORY_SCOPE_USER);
        assert!(
            repository
                .list_by_key(&normalize_memory_key("absent"), 10)
                .unwrap()
                .is_empty()
        );
    }
}
