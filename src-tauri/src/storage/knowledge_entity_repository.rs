//! Versioned fact log and SQLite projection for the structured knowledge layer.
//!
//! Task 1 of the knowledge and memory extension only establishes persistence for the
//! `knowledge_entities`, `knowledge_facts`, `knowledge_retrieval_events` and `knowledge_feedback`
//! tables plus the append-only log under `runtime-data/knowledge/structured-events.jsonl`.
//! Entity normalization, conflict resolution, expiry policy and the read-only relation query belong
//! to later tasks and are deliberately absent here.
//!
//! The existing `knowledge/events.jsonl` log keeps owning collections, sources, revisions, chunks
//! and embeddings, so lexical and semantic retrieval stay untouched.
//!
//! Facts are never physically deleted: a removed source revision flips the dependent facts to
//! `expired`, which keeps the audit trail and the historical citation intact.
//!
//! Feedback rows additionally carry an optional `chunk_id` / `source_revision_id` pair (schema v11)
//! so the retrieval ranking can attribute a rating to the chunk it rated. Rows predating v11 keep
//! `chunk_id IS NULL` and are simply ignored by that signal.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::persistence::{ProjectionDb, ProjectionError};
use crate::storage::event_validation::{
    validate_bounded, validate_confidence, validate_enum, validate_hash, validate_id,
    validate_non_negative, validate_optional_bounded, validate_optional_id, validate_token,
};
use crate::storage::now_ms;

pub const KNOWLEDGE_ENTITY_EVENT_SCHEMA_VERSION: u32 = 1;
pub const FACT_STATUS_CANDIDATE: &str = "candidate";
pub const FACT_STATUS_ACTIVE: &str = "active";
pub const FACT_STATUS_DISPUTED: &str = "disputed";
pub const FACT_STATUS_EXPIRED: &str = "expired";
pub const FACT_STATUS_REJECTED: &str = "rejected";
pub const FACT_STATUSES: [&str; 5] = [
    FACT_STATUS_CANDIDATE,
    FACT_STATUS_ACTIVE,
    FACT_STATUS_DISPUTED,
    FACT_STATUS_EXPIRED,
    FACT_STATUS_REJECTED,
];
pub const FEEDBACK_TYPES: [&str; 4] = [USEFUL_FEEDBACK_TYPE, "irrelevant", "outdated", "wrong"];
pub const USEFUL_FEEDBACK_TYPE: &str = "useful";
/// Upper bound on the chunk ids one ranking pass may ask feedback for. The four recall channels can
/// contribute 24 candidates each, so this covers a fully overlapping candidate set.
pub const MAX_FEEDBACK_CHUNK_BATCH: usize = 128;

pub const MAX_ENTITY_NAME_CHARS: usize = 256;
pub const MAX_ENTITY_DESCRIPTION_CHARS: usize = 2_000;
pub const MAX_PREDICATE_CHARS: usize = 128;
pub const MAX_OBJECT_TEXT_CHARS: usize = 2_000;
pub const DEFAULT_KNOWLEDGE_PAGE_SIZE: u32 = 100;
pub const MAX_KNOWLEDGE_PAGE_SIZE: u32 = 500;

const ENTITY_COLUMNS: &str = "id,collection_id,entity_type,name,normalized_name,description,\
     confidence,status,created_at_ms,updated_at_ms";

const FACT_COLUMNS: &str = "id,subject_entity_id,predicate,object_entity_id,object_text,\
     source_chunk_id,source_revision_id,confidence,valid_from_ms,valid_to_ms,status,created_at_ms,\
     updated_at_ms";

/// Same columns as [`ENTITY_COLUMNS`], qualified for a query that joins `knowledge_collections`.
///
/// `knowledge_collections` also has `id`, `name`, `created_at_ms` and `updated_at_ms`, so an
/// unqualified column list would be ambiguous there.
const ENTITY_COLUMNS_JOINED: &str = "e.id,e.collection_id,e.entity_type,e.name,e.normalized_name,\
     e.description,e.confidence,e.status,e.created_at_ms,e.updated_at_ms";

const RETRIEVAL_COLUMNS: &str = "id,thread_id,turn_id,query_hash,retrieval_mode,result_count,\
     selected_citation_count,latency_ms,created_at_ms";

const FEEDBACK_COLUMNS: &str =
    "id,citation_id,feedback_type,created_at_ms,chunk_id,source_revision_id";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEntityRecord {
    pub id: String,
    pub collection_id: String,
    pub entity_type: String,
    pub name: String,
    pub normalized_name: String,
    pub description: Option<String>,
    pub confidence: f64,
    pub status: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEntityWrite {
    pub id: String,
    pub collection_id: String,
    pub entity_type: String,
    pub name: String,
    pub normalized_name: String,
    pub description: Option<String>,
    pub confidence: f64,
    pub status: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeFactRecord {
    pub id: String,
    pub subject_entity_id: String,
    pub predicate: String,
    pub object_entity_id: Option<String>,
    pub object_text: Option<String>,
    pub source_chunk_id: String,
    pub source_revision_id: String,
    pub confidence: f64,
    pub valid_from_ms: Option<u64>,
    pub valid_to_ms: Option<u64>,
    pub status: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// A fact always carries the chunk and revision it was derived from, so `active` facts stay
/// traceable and a removed revision can expire them without losing the citation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeFactWrite {
    pub id: String,
    pub subject_entity_id: String,
    pub predicate: String,
    pub object_entity_id: Option<String>,
    pub object_text: Option<String>,
    pub source_chunk_id: String,
    pub source_revision_id: String,
    pub confidence: f64,
    pub valid_from_ms: Option<u64>,
    pub valid_to_ms: Option<u64>,
    pub status: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeRetrievalEventRecord {
    pub id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub query_hash: String,
    pub retrieval_mode: String,
    pub result_count: u64,
    pub selected_citation_count: u64,
    pub latency_ms: u64,
    pub created_at_ms: u64,
}

/// A user rating for a returned citation.
///
/// `chunk_id` / `source_revision_id` are optional because schema v10 predates chunk attribution:
/// a legacy row simply has no long-lived source and is excluded from every per-chunk ranking
/// signal. `citation_id` stays required so a rating is always traceable to what the user saw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeFeedbackRecord {
    pub id: String,
    pub citation_id: String,
    pub feedback_type: String,
    pub created_at_ms: u64,
    #[serde(default)]
    pub chunk_id: Option<String>,
    #[serde(default)]
    pub source_revision_id: Option<String>,
}

/// Per-chunk feedback totals used by the retrieval ranking signal.
///
/// `useful` counts the positive rating; `negative` folds `irrelevant` / `outdated` / `wrong`
/// together because the ranking only needs "did the user reject this chunk".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeFeedbackTotal {
    pub chunk_id: String,
    pub useful: u64,
    pub negative: u64,
}

/// One `active` fact joined with its endpoints and the citation it was derived from.
///
/// This is the read model of the relation query: a fact is never returned without the source it
/// came from, so a caller cannot present an unsourced relation. `source_path` / `locator` are
/// optional because the citation provenance is joined, not copied: if the chunk row is gone the
/// fact would already have been expired by `knowledge_facts_expired_for_revision`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeRelationRecord {
    pub fact_id: String,
    pub subject_entity_id: String,
    pub subject_name: String,
    pub predicate: String,
    pub object_entity_id: Option<String>,
    pub object_entity_name: Option<String>,
    pub object_text: Option<String>,
    pub confidence: f64,
    pub source_chunk_id: String,
    pub source_revision_id: String,
    pub source_path: Option<String>,
    pub locator: Option<String>,
    pub updated_at_ms: u64,
}

/// One fact candidate waiting for review, joined with the collection it belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeFactCandidateRecord {
    pub fact: KnowledgeFactRecord,
    pub collection_id: String,
    pub subject_name: String,
    pub object_entity_name: Option<String>,
    pub source_path: Option<String>,
    pub locator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeStatusChange {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEntityEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub created_at_ms: u64,
    #[serde(flatten)]
    pub kind: KnowledgeEntityEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KnowledgeEntityEventKind {
    #[serde(rename = "knowledge_entity_upserted")]
    EntityUpserted(KnowledgeEntityWrite),
    #[serde(rename = "knowledge_entity_status_changed")]
    EntityStatusChanged(KnowledgeStatusChange),
    #[serde(rename = "knowledge_fact_recorded")]
    FactRecorded(KnowledgeFactWrite),
    #[serde(rename = "knowledge_fact_status_changed")]
    FactStatusChanged(KnowledgeStatusChange),
    #[serde(rename = "knowledge_facts_expired_for_revision")]
    FactsExpiredForRevision { source_revision_id: String },
    #[serde(rename = "knowledge_retrieval_recorded")]
    RetrievalRecorded(KnowledgeRetrievalEventRecord),
    #[serde(rename = "knowledge_feedback_recorded")]
    FeedbackRecorded(KnowledgeFeedbackRecord),
}

impl KnowledgeEntityEvent {
    fn validate(&self) -> Result<(), ProjectionError> {
        if self.schema_version != KNOWLEDGE_ENTITY_EVENT_SCHEMA_VERSION {
            return Err(ProjectionError::InvalidData(format!(
                "unsupported knowledge entity event schema {}",
                self.schema_version
            )));
        }
        validate_id(&self.event_id, "eventId")?;
        match &self.kind {
            KnowledgeEntityEventKind::EntityUpserted(entity) => entity.validate(),
            KnowledgeEntityEventKind::EntityStatusChanged(change) => {
                validate_id(&change.id, "id")?;
                validate_token(&change.status, "status")
            }
            KnowledgeEntityEventKind::FactRecorded(fact) => fact.validate(),
            KnowledgeEntityEventKind::FactStatusChanged(change) => {
                validate_id(&change.id, "id")?;
                validate_enum(&change.status, "status", &FACT_STATUSES)
            }
            KnowledgeEntityEventKind::FactsExpiredForRevision { source_revision_id } => {
                validate_id(source_revision_id, "sourceRevisionId")
            }
            KnowledgeEntityEventKind::RetrievalRecorded(event) => event.validate(),
            KnowledgeEntityEventKind::FeedbackRecorded(feedback) => feedback.validate(),
        }
    }
}

impl KnowledgeEntityWrite {
    fn validate(&self) -> Result<(), ProjectionError> {
        validate_id(&self.id, "id")?;
        validate_id(&self.collection_id, "collectionId")?;
        validate_token(&self.entity_type, "entityType")?;
        validate_token(&self.status, "status")?;
        validate_bounded(&self.name, "name", 1, MAX_ENTITY_NAME_CHARS)?;
        validate_bounded(
            &self.normalized_name,
            "normalizedName",
            1,
            MAX_ENTITY_NAME_CHARS,
        )?;
        validate_optional_bounded(
            self.description.as_deref(),
            "description",
            MAX_ENTITY_DESCRIPTION_CHARS,
        )?;
        validate_confidence(self.confidence)
    }
}

impl KnowledgeFactWrite {
    fn validate(&self) -> Result<(), ProjectionError> {
        validate_id(&self.id, "id")?;
        validate_id(&self.subject_entity_id, "subjectEntityId")?;
        validate_id(&self.source_chunk_id, "sourceChunkId")?;
        validate_id(&self.source_revision_id, "sourceRevisionId")?;
        validate_optional_id(self.object_entity_id.as_deref(), "objectEntityId")?;
        validate_bounded(&self.predicate, "predicate", 1, MAX_PREDICATE_CHARS)?;
        validate_optional_bounded(
            self.object_text.as_deref(),
            "objectText",
            MAX_OBJECT_TEXT_CHARS,
        )?;
        if self.object_entity_id.is_none() && self.object_text.is_none() {
            return Err(ProjectionError::InvalidData(
                "a fact requires either objectEntityId or objectText".into(),
            ));
        }
        validate_enum(&self.status, "status", &FACT_STATUSES)?;
        validate_confidence(self.confidence)
    }
}

impl KnowledgeRetrievalEventRecord {
    fn validate(&self) -> Result<(), ProjectionError> {
        validate_id(&self.id, "id")?;
        validate_id(&self.thread_id, "threadId")?;
        validate_id(&self.turn_id, "turnId")?;
        validate_token(&self.retrieval_mode, "retrievalMode")?;
        // The design forbids persisting the raw query, so only an opaque digest is accepted.
        validate_hash(&self.query_hash, "queryHash")?;
        for (value, field) in [
            (self.result_count, "resultCount"),
            (self.selected_citation_count, "selectedCitationCount"),
            (self.latency_ms, "latencyMs"),
        ] {
            validate_non_negative(value as i64, field)?;
        }
        if self.selected_citation_count > self.result_count {
            return Err(ProjectionError::InvalidData(
                "selectedCitationCount must not exceed resultCount".into(),
            ));
        }
        Ok(())
    }
}

impl KnowledgeFeedbackRecord {
    fn validate(&self) -> Result<(), ProjectionError> {
        validate_id(&self.id, "id")?;
        validate_id(&self.citation_id, "citationId")?;
        validate_enum(&self.feedback_type, "feedbackType", &FEEDBACK_TYPES)?;
        // A rating may only claim a chunk when it also names the revision that chunk came from,
        // otherwise a stale chunk id could outlive the revision it was measured against.
        match (&self.chunk_id, &self.source_revision_id) {
            (None, None) => Ok(()),
            (Some(chunk_id), Some(revision_id)) => {
                validate_id(chunk_id, "chunkId")?;
                validate_id(revision_id, "sourceRevisionId")
            }
            _ => Err(ProjectionError::InvalidData(
                "chunkId and sourceRevisionId must be provided together".into(),
            )),
        }
    }
}

#[derive(Clone)]
pub struct KnowledgeEntityRepository {
    db: ProjectionDb,
    events_path: Option<PathBuf>,
    append_lock: Arc<Mutex<()>>,
}

impl KnowledgeEntityRepository {
    pub fn new(db: ProjectionDb) -> Self {
        let events_path = db
            .data_root()
            .map(|root| root.join("knowledge/structured-events.jsonl"));
        Self {
            db,
            events_path,
            append_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn db(&self) -> &ProjectionDb {
        &self.db
    }

    /// Appends the fact event and only then updates the projection.
    pub fn append(&self, kind: KnowledgeEntityEventKind) -> Result<(), ProjectionError> {
        let event = KnowledgeEntityEvent {
            schema_version: KNOWLEDGE_ENTITY_EVENT_SCHEMA_VERSION,
            event_id: Uuid::new_v4().to_string(),
            created_at_ms: now_ms(),
            kind,
        };
        self.append_event(&event)?;
        self.apply_event(&event)
    }

    fn append_event(&self, event: &KnowledgeEntityEvent) -> Result<(), ProjectionError> {
        let Some(path) = self.events_path.as_ref() else {
            return Ok(());
        };
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let parent = path.parent().ok_or_else(|| {
            ProjectionError::InvalidData("knowledge entity event path has no parent".into())
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            ProjectionError::InvalidData(format!(
                "create knowledge entity event directory: {error}"
            ))
        })?;
        let line = serde_json::to_string(event).map_err(|error| {
            ProjectionError::InvalidData(format!("serialize knowledge entity event: {error}"))
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| {
                ProjectionError::InvalidData(format!("open knowledge entity event log: {error}"))
            })?;
        file.write_all(line.as_bytes())
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_data())
            .map_err(|error| {
                ProjectionError::InvalidData(format!("append knowledge entity event: {error}"))
            })
    }

    /// Rebuilds the structured knowledge projection from the fact log. Every record is parsed and
    /// validated before any row is touched, so a corrupted log closes the rebuild without emptying
    /// the projection.
    pub fn rebuild_projection(&self) -> Result<(), ProjectionError> {
        let Some(path) = self.events_path.as_ref() else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(path).map_err(|error| {
            ProjectionError::InvalidData(format!("read knowledge entity event log: {error}"))
        })?;
        if content.trim().is_empty() {
            return Ok(());
        }
        let lines = content.split('\n').collect::<Vec<_>>();
        let has_trailing_newline = content.ends_with('\n');
        let mut events = Vec::new();
        for (index, raw_line) in lines.iter().enumerate() {
            let line = raw_line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let event = match serde_json::from_str::<KnowledgeEntityEvent>(line) {
                Ok(event) => event,
                Err(error) if !has_trailing_newline && index + 1 == lines.len() => {
                    let _ = error;
                    break;
                }
                Err(error) => {
                    return Err(ProjectionError::InvalidData(format!(
                        "invalid knowledge entity event at line {}: {error}",
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
            transaction.execute("DELETE FROM knowledge_feedback", [])?;
            transaction.execute("DELETE FROM knowledge_retrieval_events", [])?;
            transaction.execute("DELETE FROM knowledge_facts", [])?;
            transaction.execute("DELETE FROM knowledge_entities", [])?;
            transaction.commit()?;
            Ok(())
        })
    }

    fn apply_event(&self, event: &KnowledgeEntityEvent) -> Result<(), ProjectionError> {
        event.validate()?;
        self.db
            .with_connection(|connection| {
                let transaction = connection.transaction()?;
                match &event.kind {
                    KnowledgeEntityEventKind::EntityUpserted(entity) => {
                        transaction.execute(
                            "INSERT INTO knowledge_entities(id,collection_id,entity_type,name,
                               normalized_name,description,confidence,status,created_at_ms,
                               updated_at_ms)
                             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                             ON CONFLICT(id) DO UPDATE SET collection_id=excluded.collection_id,
                               entity_type=excluded.entity_type,name=excluded.name,
                               normalized_name=excluded.normalized_name,
                               description=excluded.description,confidence=excluded.confidence,
                               status=excluded.status,updated_at_ms=excluded.updated_at_ms",
                            params![
                                entity.id,
                                entity.collection_id,
                                entity.entity_type,
                                entity.name,
                                entity.normalized_name,
                                entity.description,
                                entity.confidence,
                                entity.status,
                                entity.created_at_ms as i64,
                                event.created_at_ms as i64,
                            ],
                        )?;
                    }
                    KnowledgeEntityEventKind::EntityStatusChanged(change) => {
                        require_row(transaction.execute(
                            "UPDATE knowledge_entities SET status=?2,updated_at_ms=?3 WHERE id=?1",
                            params![change.id, change.status, event.created_at_ms as i64],
                        )?)?;
                    }
                    KnowledgeEntityEventKind::FactRecorded(fact) => {
                        transaction.execute(
                            "INSERT INTO knowledge_facts(id,subject_entity_id,predicate,
                               object_entity_id,object_text,source_chunk_id,source_revision_id,
                               confidence,valid_from_ms,valid_to_ms,status,created_at_ms,updated_at_ms)
                             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
                             ON CONFLICT(id) DO UPDATE SET
                               subject_entity_id=excluded.subject_entity_id,
                               predicate=excluded.predicate,
                               object_entity_id=excluded.object_entity_id,
                               object_text=excluded.object_text,
                               source_chunk_id=excluded.source_chunk_id,
                               source_revision_id=excluded.source_revision_id,
                               confidence=excluded.confidence,
                               valid_from_ms=excluded.valid_from_ms,
                               valid_to_ms=excluded.valid_to_ms,status=excluded.status,
                               updated_at_ms=excluded.updated_at_ms",
                            params![
                                fact.id,
                                fact.subject_entity_id,
                                fact.predicate,
                                fact.object_entity_id,
                                fact.object_text,
                                fact.source_chunk_id,
                                fact.source_revision_id,
                                fact.confidence,
                                fact.valid_from_ms.map(|value| value as i64),
                                fact.valid_to_ms.map(|value| value as i64),
                                fact.status,
                                fact.created_at_ms as i64,
                                event.created_at_ms as i64,
                            ],
                        )?;
                    }
                    KnowledgeEntityEventKind::FactStatusChanged(change) => {
                        require_row(transaction.execute(
                            "UPDATE knowledge_facts SET status=?2,updated_at_ms=?3 WHERE id=?1",
                            params![change.id, change.status, event.created_at_ms as i64],
                        )?)?;
                    }
                    KnowledgeEntityEventKind::FactsExpiredForRevision { source_revision_id } => {
                        // Expiry is a status transition, never a delete, so the citation and the
                        // audit trail survive a source removal.
                        transaction.execute(
                            "UPDATE knowledge_facts SET status=?2,updated_at_ms=?3
                             WHERE source_revision_id=?1 AND status<>?2",
                            params![
                                source_revision_id,
                                FACT_STATUS_EXPIRED,
                                event.created_at_ms as i64
                            ],
                        )?;
                    }
                    KnowledgeEntityEventKind::RetrievalRecorded(retrieval) => {
                        transaction.execute(
                            "INSERT INTO knowledge_retrieval_events(id,thread_id,turn_id,
                               query_hash,retrieval_mode,result_count,selected_citation_count,
                               latency_ms,created_at_ms)
                             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                             ON CONFLICT(id) DO NOTHING",
                            params![
                                retrieval.id,
                                retrieval.thread_id,
                                retrieval.turn_id,
                                retrieval.query_hash,
                                retrieval.retrieval_mode,
                                retrieval.result_count as i64,
                                retrieval.selected_citation_count as i64,
                                retrieval.latency_ms as i64,
                                retrieval.created_at_ms as i64,
                            ],
                        )?;
                    }
                    KnowledgeEntityEventKind::FeedbackRecorded(feedback) => {
                        transaction.execute(
                            "INSERT INTO knowledge_feedback(id,citation_id,feedback_type,
                               created_at_ms,chunk_id,source_revision_id)
                             VALUES(?1,?2,?3,?4,?5,?6)
                             ON CONFLICT(id) DO NOTHING",
                            params![
                                feedback.id,
                                feedback.citation_id,
                                feedback.feedback_type,
                                feedback.created_at_ms as i64,
                                feedback.chunk_id,
                                feedback.source_revision_id,
                            ],
                        )?;
                    }
                }
                transaction.commit()?;
                Ok(())
            })
            .map_err(|error| match error {
                ProjectionError::Database(rusqlite::Error::QueryReturnedNoRows) => {
                    ProjectionError::InvalidData(
                        "knowledge entity event targets a record that does not exist".into(),
                    )
                }
                other => other,
            })
    }

    pub fn get_entity(&self, id: &str) -> Result<Option<KnowledgeEntityRecord>, ProjectionError> {
        self.db.with_connection(|connection| {
            connection
                .query_row(
                    &format!("SELECT {ENTITY_COLUMNS} FROM knowledge_entities WHERE id=?1"),
                    [id],
                    map_entity,
                )
                .optional()
        })
    }

    pub fn list_entities(
        &self,
        collection_id: &str,
        status: &str,
        limit: u32,
    ) -> Result<Vec<KnowledgeEntityRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_KNOWLEDGE_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {ENTITY_COLUMNS} FROM knowledge_entities
                 WHERE collection_id=?1 AND status=?2 ORDER BY updated_at_ms DESC,id ASC LIMIT ?3"
            ))?;
            let rows = statement.query_map(params![collection_id, status, limit], map_entity)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn get_fact(&self, id: &str) -> Result<Option<KnowledgeFactRecord>, ProjectionError> {
        self.db.with_connection(|connection| {
            connection
                .query_row(
                    &format!("SELECT {FACT_COLUMNS} FROM knowledge_facts WHERE id=?1"),
                    [id],
                    map_fact,
                )
                .optional()
        })
    }

    pub fn list_facts(
        &self,
        subject_entity_id: &str,
        status: &str,
        limit: u32,
    ) -> Result<Vec<KnowledgeFactRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_KNOWLEDGE_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {FACT_COLUMNS} FROM knowledge_facts
                 WHERE subject_entity_id=?1 AND status=?2 ORDER BY updated_at_ms DESC,id ASC LIMIT ?3"
            ))?;
            let rows = statement.query_map(params![subject_entity_id, status, limit], map_fact)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn list_facts_for_revision(
        &self,
        source_revision_id: &str,
        limit: u32,
    ) -> Result<Vec<KnowledgeFactRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_KNOWLEDGE_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {FACT_COLUMNS} FROM knowledge_facts
                 WHERE source_revision_id=?1 ORDER BY id ASC LIMIT ?2"
            ))?;
            let rows = statement.query_map(params![source_revision_id, limit], map_fact)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn list_retrieval_events(
        &self,
        thread_id: &str,
        limit: u32,
    ) -> Result<Vec<KnowledgeRetrievalEventRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_KNOWLEDGE_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {RETRIEVAL_COLUMNS} FROM knowledge_retrieval_events
                 WHERE thread_id=?1 ORDER BY created_at_ms DESC,id ASC LIMIT ?2"
            ))?;
            let rows = statement.query_map(params![thread_id, limit], map_retrieval_event)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn list_feedback(
        &self,
        citation_id: &str,
    ) -> Result<Vec<KnowledgeFeedbackRecord>, ProjectionError> {
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {FEEDBACK_COLUMNS} FROM knowledge_feedback
                 WHERE citation_id=?1 ORDER BY created_at_ms ASC,id ASC"
            ))?;
            let rows = statement.query_map([citation_id], map_feedback)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// Aggregate the ratings that are attributable to a chunk.
    ///
    /// The input is deduplicated and capped at [`MAX_FEEDBACK_CHUNK_BATCH`], so a caller can pass a
    /// whole retrieval candidate set without turning the ranking pass into an unbounded query.
    /// Rows written before chunk attribution existed (`chunk_id IS NULL`) never appear here.
    pub fn feedback_totals_for_chunks(
        &self,
        chunk_ids: &[String],
    ) -> Result<Vec<KnowledgeFeedbackTotal>, ProjectionError> {
        let mut unique = Vec::new();
        for chunk_id in chunk_ids {
            if chunk_id.is_empty() || unique.iter().any(|seen| seen == chunk_id) {
                continue;
            }
            unique.push(chunk_id.clone());
            if unique.len() >= MAX_FEEDBACK_CHUNK_BATCH {
                break;
            }
        }
        if unique.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; unique.len()].join(",");
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT chunk_id,
                        SUM(CASE WHEN feedback_type=? THEN 1 ELSE 0 END),
                        SUM(CASE WHEN feedback_type<>? THEN 1 ELSE 0 END)
                 FROM knowledge_feedback
                 WHERE chunk_id IN ({placeholders})
                 GROUP BY chunk_id ORDER BY chunk_id ASC"
            ))?;
            let mut parameters: Vec<&dyn rusqlite::ToSql> = vec![&USEFUL_FEEDBACK_TYPE];
            parameters.push(&USEFUL_FEEDBACK_TYPE);
            for chunk_id in &unique {
                parameters.push(chunk_id);
            }
            let rows = statement.query_map(parameters.as_slice(), |row| {
                Ok(KnowledgeFeedbackTotal {
                    chunk_id: row.get(0)?,
                    useful: row.get::<_, i64>(1)?.max(0) as u64,
                    negative: row.get::<_, i64>(2)?.max(0) as u64,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// The row a normalized name already resolves to inside one collection.
    ///
    /// Normalization is host-owned, so this is the only way an entity is identified; a caller never
    /// looks an entity up by the raw name it was proposed with.
    pub fn find_entity_by_normalized_name(
        &self,
        collection_id: &str,
        normalized_name: &str,
    ) -> Result<Option<KnowledgeEntityRecord>, ProjectionError> {
        self.db.with_connection(|connection| {
            connection
                .query_row(
                    &format!(
                        "SELECT {ENTITY_COLUMNS} FROM knowledge_entities
                         WHERE collection_id=?1 AND normalized_name=?2
                         ORDER BY updated_at_ms DESC,id ASC LIMIT 1"
                    ),
                    params![collection_id, normalized_name],
                    map_entity,
                )
                .optional()
        })
    }

    /// `active` entities with this normalized name in one workspace scope.
    pub fn list_entities_by_normalized_name(
        &self,
        scope_key: &str,
        normalized_name: &str,
        limit: u32,
    ) -> Result<Vec<KnowledgeEntityRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_KNOWLEDGE_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {ENTITY_COLUMNS_JOINED} FROM knowledge_entities e
                 JOIN knowledge_collections c ON c.id=e.collection_id
                   AND c.enabled=1 AND c.deleted=0
                 WHERE c.scope_key=?1 AND e.normalized_name=?2 AND e.status='active'
                 ORDER BY e.updated_at_ms DESC,e.id ASC LIMIT ?3"
            ))?;
            let rows =
                statement.query_map(params![scope_key, normalized_name, limit], map_entity)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// The `active` facts already asserted for one subject and predicate, used for conflict
    /// detection. The comparison happens in the domain layer, not here.
    pub fn list_active_facts_for_subject_predicate(
        &self,
        subject_entity_id: &str,
        predicate: &str,
    ) -> Result<Vec<KnowledgeFactRecord>, ProjectionError> {
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {FACT_COLUMNS} FROM knowledge_facts
                 WHERE subject_entity_id=?1 AND predicate=?2 AND status='active'
                 ORDER BY updated_at_ms DESC,id ASC"
            ))?;
            let rows = statement.query_map(params![subject_entity_id, predicate], map_fact)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// Facts of one status in a collection, joined with the endpoints and the citation they came
    /// from. This is what the review queue reads.
    pub fn list_facts_by_status_for_collection(
        &self,
        collection_id: &str,
        status: &str,
        limit: u32,
    ) -> Result<Vec<KnowledgeFactCandidateRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_KNOWLEDGE_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT f.id,f.subject_entity_id,f.predicate,f.object_entity_id,f.object_text,
                        f.source_chunk_id,f.source_revision_id,f.confidence,f.valid_from_ms,
                        f.valid_to_ms,f.status,f.created_at_ms,f.updated_at_ms,
                        se.collection_id,se.name,oe.name,s.relative_path,k.start_line,k.end_line
                 FROM knowledge_facts f
                 JOIN knowledge_entities se ON se.id=f.subject_entity_id
                 LEFT JOIN knowledge_entities oe ON oe.id=f.object_entity_id
                 LEFT JOIN knowledge_chunks k ON k.id=f.source_chunk_id
                 LEFT JOIN knowledge_revisions r ON r.id=f.source_revision_id
                 LEFT JOIN knowledge_sources s ON s.id=r.source_id
                 WHERE se.collection_id=?1 AND f.status=?2
                 ORDER BY f.updated_at_ms DESC,f.id ASC LIMIT ?3",
            )?;
            let rows =
                statement.query_map(params![collection_id, status, limit], map_fact_candidate)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// The relation read model: `active` facts whose subject carries this normalized name, inside
    /// one workspace scope, each with the citation it was derived from.
    pub fn list_relations(
        &self,
        scope_key: &str,
        normalized_name: &str,
        limit: u32,
    ) -> Result<Vec<KnowledgeRelationRecord>, ProjectionError> {
        let limit = limit.clamp(1, MAX_KNOWLEDGE_PAGE_SIZE);
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT f.id,f.subject_entity_id,f.predicate,f.object_entity_id,f.object_text,
                        f.source_chunk_id,f.source_revision_id,f.confidence,f.status,f.updated_at_ms,
                        se.name,oe.name,s.relative_path,k.start_line,k.end_line
                 FROM knowledge_facts f
                 JOIN knowledge_entities se ON se.id=f.subject_entity_id AND se.status='active'
                 JOIN knowledge_collections c ON c.id=se.collection_id
                   AND c.enabled=1 AND c.deleted=0
                 LEFT JOIN knowledge_entities oe ON oe.id=f.object_entity_id
                 LEFT JOIN knowledge_chunks k ON k.id=f.source_chunk_id
                 LEFT JOIN knowledge_revisions r ON r.id=f.source_revision_id
                 LEFT JOIN knowledge_sources s ON s.id=r.source_id
                 WHERE c.scope_key=?1 AND f.status='active' AND se.normalized_name=?2
                 ORDER BY f.updated_at_ms DESC,f.id ASC LIMIT ?3",
            )?;
            let rows = statement.query_map(params![scope_key, normalized_name, limit], map_relation)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }
}

fn require_row(changed: usize) -> Result<(), rusqlite::Error> {
    if changed == 0 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    Ok(())
}

fn map_entity(row: &rusqlite::Row<'_>) -> Result<KnowledgeEntityRecord, rusqlite::Error> {
    Ok(KnowledgeEntityRecord {
        id: row.get(0)?,
        collection_id: row.get(1)?,
        entity_type: row.get(2)?,
        name: row.get(3)?,
        normalized_name: row.get(4)?,
        description: row.get(5)?,
        confidence: row.get(6)?,
        status: row.get(7)?,
        created_at_ms: row.get::<_, i64>(8)?.max(0) as u64,
        updated_at_ms: row.get::<_, i64>(9)?.max(0) as u64,
    })
}

fn map_fact(row: &rusqlite::Row<'_>) -> Result<KnowledgeFactRecord, rusqlite::Error> {
    Ok(KnowledgeFactRecord {
        id: row.get(0)?,
        subject_entity_id: row.get(1)?,
        predicate: row.get(2)?,
        object_entity_id: row.get(3)?,
        object_text: row.get(4)?,
        source_chunk_id: row.get(5)?,
        source_revision_id: row.get(6)?,
        confidence: row.get(7)?,
        valid_from_ms: row
            .get::<_, Option<i64>>(8)?
            .map(|value| value.max(0) as u64),
        valid_to_ms: row
            .get::<_, Option<i64>>(9)?
            .map(|value| value.max(0) as u64),
        status: row.get(10)?,
        created_at_ms: row.get::<_, i64>(11)?.max(0) as u64,
        updated_at_ms: row.get::<_, i64>(12)?.max(0) as u64,
    })
}

/// `L<start>-<end>` for a joined chunk row, or `None` when the chunk row is gone.
fn joined_locator(
    row: &rusqlite::Row<'_>,
    start_index: usize,
    end_index: usize,
) -> Result<Option<String>, rusqlite::Error> {
    let start = row.get::<_, Option<i64>>(start_index)?;
    let end = row.get::<_, Option<i64>>(end_index)?;
    Ok(match (start, end) {
        (Some(start), Some(end)) => Some(format!("L{start}-{end}")),
        _ => None,
    })
}

fn map_relation(row: &rusqlite::Row<'_>) -> Result<KnowledgeRelationRecord, rusqlite::Error> {
    Ok(KnowledgeRelationRecord {
        fact_id: row.get(0)?,
        subject_entity_id: row.get(1)?,
        predicate: row.get(2)?,
        object_entity_id: row.get(3)?,
        object_text: row.get(4)?,
        source_chunk_id: row.get(5)?,
        source_revision_id: row.get(6)?,
        confidence: row.get(7)?,
        subject_name: row.get(10)?,
        object_entity_name: row.get(11)?,
        source_path: row.get(12)?,
        locator: joined_locator(row, 13, 14)?,
        updated_at_ms: row.get::<_, i64>(9)?.max(0) as u64,
    })
}

fn map_fact_candidate(
    row: &rusqlite::Row<'_>,
) -> Result<KnowledgeFactCandidateRecord, rusqlite::Error> {
    Ok(KnowledgeFactCandidateRecord {
        fact: KnowledgeFactRecord {
            id: row.get(0)?,
            subject_entity_id: row.get(1)?,
            predicate: row.get(2)?,
            object_entity_id: row.get(3)?,
            object_text: row.get(4)?,
            source_chunk_id: row.get(5)?,
            source_revision_id: row.get(6)?,
            confidence: row.get(7)?,
            valid_from_ms: row
                .get::<_, Option<i64>>(8)?
                .map(|value| value.max(0) as u64),
            valid_to_ms: row
                .get::<_, Option<i64>>(9)?
                .map(|value| value.max(0) as u64),
            status: row.get(10)?,
            created_at_ms: row.get::<_, i64>(11)?.max(0) as u64,
            updated_at_ms: row.get::<_, i64>(12)?.max(0) as u64,
        },
        collection_id: row.get(13)?,
        subject_name: row.get(14)?,
        object_entity_name: row.get(15)?,
        source_path: row.get(16)?,
        locator: joined_locator(row, 17, 18)?,
    })
}

fn map_retrieval_event(
    row: &rusqlite::Row<'_>,
) -> Result<KnowledgeRetrievalEventRecord, rusqlite::Error> {
    Ok(KnowledgeRetrievalEventRecord {
        id: row.get(0)?,
        thread_id: row.get(1)?,
        turn_id: row.get(2)?,
        query_hash: row.get(3)?,
        retrieval_mode: row.get(4)?,
        result_count: row.get::<_, i64>(5)?.max(0) as u64,
        selected_citation_count: row.get::<_, i64>(6)?.max(0) as u64,
        latency_ms: row.get::<_, i64>(7)?.max(0) as u64,
        created_at_ms: row.get::<_, i64>(8)?.max(0) as u64,
    })
}

fn map_feedback(row: &rusqlite::Row<'_>) -> Result<KnowledgeFeedbackRecord, rusqlite::Error> {
    Ok(KnowledgeFeedbackRecord {
        id: row.get(0)?,
        citation_id: row.get(1)?,
        feedback_type: row.get(2)?,
        created_at_ms: row.get::<_, i64>(3)?.max(0) as u64,
        chunk_id: row.get(4)?,
        source_revision_id: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository(data_root: &std::path::Path) -> KnowledgeEntityRepository {
        KnowledgeEntityRepository::new(ProjectionDb::open(data_root).unwrap())
    }

    fn entity(id: &str, name: &str) -> KnowledgeEntityWrite {
        KnowledgeEntityWrite {
            id: id.to_owned(),
            collection_id: "collection-1".to_owned(),
            entity_type: "component".to_owned(),
            name: name.to_owned(),
            normalized_name: name.to_lowercase(),
            description: Some("A workspace component".to_owned()),
            confidence: 0.8,
            status: "active".to_owned(),
            created_at_ms: 500,
        }
    }

    fn fact(id: &str, status: &str) -> KnowledgeFactWrite {
        KnowledgeFactWrite {
            id: id.to_owned(),
            subject_entity_id: "entity-1".to_owned(),
            predicate: "stores_data_in".to_owned(),
            object_entity_id: None,
            object_text: Some("SQLite".to_owned()),
            source_chunk_id: "chunk-1".to_owned(),
            source_revision_id: "revision-1".to_owned(),
            confidence: 0.75,
            valid_from_ms: Some(100),
            valid_to_ms: None,
            status: status.to_owned(),
            created_at_ms: 600,
        }
    }

    #[test]
    fn entities_and_facts_are_projected_and_rebuild_from_the_fact_log() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        repository
            .append(KnowledgeEntityEventKind::EntityUpserted(entity(
                "entity-1", "Storage",
            )))
            .unwrap();
        repository
            .append(KnowledgeEntityEventKind::FactRecorded(fact(
                "fact-1",
                FACT_STATUS_ACTIVE,
            )))
            .unwrap();

        let stored = repository.get_entity("entity-1").unwrap().unwrap();
        assert_eq!(stored.name, "Storage");
        assert_eq!(stored.created_at_ms, 500);
        let stored_fact = repository.get_fact("fact-1").unwrap().unwrap();
        assert_eq!(stored_fact.status, FACT_STATUS_ACTIVE);
        assert_eq!(stored_fact.source_chunk_id, "chunk-1");
        assert_eq!(stored_fact.created_at_ms, 600);

        repository.clear_projection().unwrap();
        assert!(repository.get_entity("entity-1").unwrap().is_none());

        repository.rebuild_projection().unwrap();

        assert_eq!(
            repository.get_entity("entity-1").unwrap().unwrap().name,
            "Storage"
        );
        let replayed = repository.get_fact("fact-1").unwrap().unwrap();
        assert_eq!(replayed.status, FACT_STATUS_ACTIVE);
        assert_eq!(replayed.created_at_ms, 600);
        assert_eq!(replayed.valid_from_ms, Some(100));
    }

    #[test]
    fn facts_of_a_removed_revision_expire_instead_of_being_purged() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        repository
            .append(KnowledgeEntityEventKind::FactRecorded(fact(
                "fact-1",
                FACT_STATUS_ACTIVE,
            )))
            .unwrap();
        repository
            .append(KnowledgeEntityEventKind::FactRecorded(fact(
                "fact-2",
                FACT_STATUS_ACTIVE,
            )))
            .unwrap();

        repository
            .append(KnowledgeEntityEventKind::FactsExpiredForRevision {
                source_revision_id: "revision-1".to_owned(),
            })
            .unwrap();

        let facts = repository
            .list_facts_for_revision("revision-1", 10)
            .unwrap();
        assert_eq!(facts.len(), 2, "expiry must not delete the fact rows");
        assert!(facts.iter().all(|fact| fact.status == FACT_STATUS_EXPIRED));
        assert!(facts.iter().all(|fact| fact.source_chunk_id == "chunk-1"));
        assert!(
            repository
                .list_facts("entity-1", FACT_STATUS_ACTIVE, 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn retrieval_events_and_feedback_are_append_only_facts() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        repository
            .append(KnowledgeEntityEventKind::RetrievalRecorded(
                KnowledgeRetrievalEventRecord {
                    id: "retrieval-1".to_owned(),
                    thread_id: "thread-1".to_owned(),
                    turn_id: "turn-1".to_owned(),
                    query_hash: "0123456789abcdef".to_owned(),
                    retrieval_mode: "hybrid".to_owned(),
                    result_count: 4,
                    selected_citation_count: 2,
                    latency_ms: 37,
                    created_at_ms: 900,
                },
            ))
            .unwrap();
        repository
            .append(KnowledgeEntityEventKind::FeedbackRecorded(
                KnowledgeFeedbackRecord {
                    id: "feedback-1".to_owned(),
                    citation_id: "citation-1".to_owned(),
                    feedback_type: "useful".to_owned(),
                    created_at_ms: 1_000,
                    chunk_id: Some("chunk-1".to_owned()),
                    source_revision_id: Some("revision-1".to_owned()),
                },
            ))
            .unwrap();

        let events = repository.list_retrieval_events("thread-1", 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].result_count, 4);
        assert_eq!(events[0].selected_citation_count, 2);
        let feedback = repository.list_feedback("citation-1").unwrap();
        assert_eq!(feedback.len(), 1);
        assert_eq!(feedback[0].feedback_type, "useful");
        assert_eq!(feedback[0].chunk_id.as_deref(), Some("chunk-1"));
        assert_eq!(
            feedback[0].source_revision_id.as_deref(),
            Some("revision-1")
        );

        repository.clear_projection().unwrap();
        repository.rebuild_projection().unwrap();
        assert_eq!(
            repository
                .list_retrieval_events("thread-1", 10)
                .unwrap()
                .len(),
            1
        );
        let rebuilt = repository.list_feedback("citation-1").unwrap();
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].chunk_id.as_deref(), Some("chunk-1"));
    }

    #[test]
    fn feedback_totals_fold_the_negative_ratings_and_ignore_unattributed_rows() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());

        for (id, feedback_type, chunk_id) in [
            ("feedback-1", "useful", Some("chunk-1")),
            ("feedback-2", "useful", Some("chunk-1")),
            ("feedback-3", "irrelevant", Some("chunk-1")),
            ("feedback-4", "outdated", Some("chunk-1")),
            ("feedback-5", "wrong", Some("chunk-2")),
            ("feedback-6", "useful", None),
        ] {
            repository
                .append(KnowledgeEntityEventKind::FeedbackRecorded(
                    KnowledgeFeedbackRecord {
                        id: id.to_owned(),
                        citation_id: format!("citation-{id}"),
                        feedback_type: feedback_type.to_owned(),
                        created_at_ms: 100,
                        chunk_id: chunk_id.map(str::to_owned),
                        source_revision_id: chunk_id.map(|_| "revision-1".to_owned()),
                    },
                ))
                .unwrap();
        }

        let totals = repository
            .feedback_totals_for_chunks(&[
                "chunk-1".to_owned(),
                "chunk-2".to_owned(),
                "chunk-1".to_owned(),
                "chunk-missing".to_owned(),
            ])
            .unwrap();
        assert_eq!(
            totals,
            vec![
                KnowledgeFeedbackTotal {
                    chunk_id: "chunk-1".to_owned(),
                    useful: 2,
                    negative: 2,
                },
                KnowledgeFeedbackTotal {
                    chunk_id: "chunk-2".to_owned(),
                    useful: 0,
                    negative: 1,
                },
            ],
            "unattributed rows and unknown chunks must not appear"
        );

        assert!(
            repository
                .feedback_totals_for_chunks(&[])
                .unwrap()
                .is_empty()
        );
        assert!(
            repository
                .feedback_totals_for_chunks(&["".to_owned()])
                .unwrap()
                .is_empty(),
            "a blank chunk id must never widen the query"
        );
    }

    #[test]
    fn feedback_that_claims_a_chunk_without_a_revision_is_rejected() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());

        for (chunk_id, source_revision_id) in [
            (Some("chunk-1".to_owned()), None),
            (None, Some("revision-1".to_owned())),
            (Some(String::new()), Some("revision-1".to_owned())),
        ] {
            assert!(
                matches!(
                    repository.append(KnowledgeEntityEventKind::FeedbackRecorded(
                        KnowledgeFeedbackRecord {
                            id: "feedback-1".to_owned(),
                            citation_id: "citation-1".to_owned(),
                            feedback_type: "useful".to_owned(),
                            created_at_ms: 1,
                            chunk_id,
                            source_revision_id,
                        }
                    )),
                    Err(ProjectionError::InvalidData(_))
                ),
                "a partial chunk binding must be rejected"
            );
        }
        assert!(repository.list_feedback("citation-1").unwrap().is_empty());
    }

    #[test]
    fn facts_without_a_source_object_or_valid_status_are_rejected() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());

        let mut objectless = fact("fact-1", FACT_STATUS_CANDIDATE);
        objectless.object_text = None;
        objectless.object_entity_id = None;
        assert!(matches!(
            repository.append(KnowledgeEntityEventKind::FactRecorded(objectless)),
            Err(ProjectionError::InvalidData(_))
        ));

        let mut sourceless = fact("fact-2", FACT_STATUS_CANDIDATE);
        sourceless.source_chunk_id = String::new();
        assert!(matches!(
            repository.append(KnowledgeEntityEventKind::FactRecorded(sourceless)),
            Err(ProjectionError::InvalidData(_))
        ));

        let mut unknown_status = fact("fact-3", "guessed");
        unknown_status.status = "guessed".to_owned();
        assert!(matches!(
            repository.append(KnowledgeEntityEventKind::FactRecorded(unknown_status)),
            Err(ProjectionError::InvalidData(_))
        ));

        let mut nan_confidence = fact("fact-4", FACT_STATUS_CANDIDATE);
        nan_confidence.confidence = f64::INFINITY;
        assert!(matches!(
            repository.append(KnowledgeEntityEventKind::FactRecorded(nan_confidence)),
            Err(ProjectionError::InvalidData(_))
        ));

        assert!(repository.get_fact("fact-1").unwrap().is_none());
        assert!(repository.get_fact("fact-4").unwrap().is_none());
    }

    #[test]
    fn retrieval_events_reject_raw_queries_and_impossible_counts() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());

        let mut raw_query = KnowledgeRetrievalEventRecord {
            id: "retrieval-1".to_owned(),
            thread_id: "thread-1".to_owned(),
            turn_id: "turn-1".to_owned(),
            query_hash: "how do I index the docs".to_owned(),
            retrieval_mode: "hybrid".to_owned(),
            result_count: 3,
            selected_citation_count: 1,
            latency_ms: 12,
            created_at_ms: 1,
        };
        assert!(matches!(
            repository.append(KnowledgeEntityEventKind::RetrievalRecorded(
                raw_query.clone()
            )),
            Err(ProjectionError::InvalidData(_))
        ));

        raw_query.query_hash = "abcdef0123456789".to_owned();
        raw_query.selected_citation_count = 9;
        assert!(matches!(
            repository.append(KnowledgeEntityEventKind::RetrievalRecorded(raw_query)),
            Err(ProjectionError::InvalidData(_))
        ));

        assert!(
            repository
                .list_retrieval_events("thread-1", 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unknown_feedback_types_and_status_changes_for_missing_rows_fail() {
        let data_root = tempfile::tempdir().unwrap();
        let repository = repository(data_root.path());
        assert!(matches!(
            repository.append(KnowledgeEntityEventKind::FeedbackRecorded(
                KnowledgeFeedbackRecord {
                    id: "feedback-1".to_owned(),
                    citation_id: "citation-1".to_owned(),
                    feedback_type: "amazing".to_owned(),
                    created_at_ms: 1,
                    chunk_id: None,
                    source_revision_id: None,
                }
            )),
            Err(ProjectionError::InvalidData(_))
        ));
        assert!(matches!(
            repository.append(KnowledgeEntityEventKind::FactStatusChanged(
                KnowledgeStatusChange {
                    id: "missing".to_owned(),
                    status: FACT_STATUS_REJECTED.to_owned(),
                }
            )),
            Err(ProjectionError::InvalidData(_))
        ));
    }
}
