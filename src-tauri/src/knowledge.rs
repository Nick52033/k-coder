//! Local-first knowledge collections, bounded indexing and lexical retrieval.
//!
//! The first implementation deliberately keeps the projection rebuildable: the
//! source file remains the source of truth and SQLite contains only metadata,
//! chunks and FTS projections. Semantic vectors are an optional extension point
//! and never weaken the workspace or citation boundaries.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::logging::StructuredLogger;
use crate::persistence::ProjectionDb;
use crate::protocol::{ToolDefinition, ToolResult, ToolRisk};
use crate::providers::CredentialStore;
use crate::tools::{ToolContext, ToolError, ToolHandler};

const PARSER_VERSION: &str = "text-v1";
const CHUNKER_VERSION: &str = "paragraph-v1";
const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 24 * 1024;
const MAX_RESULTS: usize = 6;
const MAX_CITATIONS: usize = 500;
const EMBEDDING_PROVIDER: &str = "siliconflow";
const EMBEDDING_MODEL: &str = "BAAI/bge-m3";
const EMBEDDING_ENDPOINT: &str = "https://api.siliconflow.cn/v1/embeddings";
const EMBEDDING_ENCODING: &str = "float";
const EMBEDDING_CREDENTIAL: &str = "embedding-api-key:siliconflow";
const QUERY_EMBEDDING_CACHE_CAPACITY: usize = 128;
const QUERY_EMBEDDING_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSettings {
    pub enabled: bool,
    pub auto_search: bool,
    pub max_results: usize,
    pub max_chunk_tokens: usize,
    pub knowledge_budget_percent: usize,
    pub semantic_enabled: bool,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_dimension: usize,
    pub embedding_configured: bool,
    pub embedding_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingSettings {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub semantic_enabled: bool,
    pub encoding_format: String,
    pub batch_size: usize,
    pub timeout_ms: u64,
    pub max_vector_scan_chunks: usize,
    pub model_max_input_tokens: usize,
    pub vector_dimension: usize,
    pub embedding_configured: bool,
    pub embedding_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeCollection {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub scope_key: String,
    pub enabled: bool,
    pub source_count: usize,
    pub indexed_chunk_count: usize,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSource {
    pub source_id: String,
    pub relative_path: String,
    pub size_bytes: u64,
    pub content_hash_prefix: Option<String>,
    pub active_revision_id: Option<String>,
    pub active_embedding_model: Option<String>,
    pub active_embedding_dimension: usize,
    pub active_embedding_encoding_format: String,
    pub embedding_status: String,
    pub state: String,
    pub chunk_count: usize,
    pub last_indexed_at_ms: Option<u64>,
    pub last_error_code: Option<String>,
    pub initial_job_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeIndexJob {
    pub job_id: String,
    pub source_id: String,
    pub state: String,
    pub stage: String,
    pub embedding_mode: String,
    pub processed_bytes: u64,
    pub total_bytes: u64,
    pub processed_chunks: usize,
    pub total_chunks: usize,
    pub embedding_requests: usize,
    pub retry_count: usize,
    pub last_http_status: Option<u16>,
    pub chunk_count: usize,
    pub vector_count: usize,
    pub error_code: Option<String>,
    pub created_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}

/// 索引进度事件名。前端通过 `listen("knowledge-index-progress")` 订阅。
pub const KNOWLEDGE_PROGRESS_EVENT_NAME: &str = "knowledge-index-progress";
pub const KNOWLEDGE_PROGRESS_SCHEMA_VERSION: u32 = 1;
/// 高速阶段（embedding 批次）合并进度事件的最小间隔。
const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);
/// 已收口 job 的有界去重集合上限，避免长时间运行后无限增长。
const MAX_OBSERVED_JOBS: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeIndexProgress {
    pub schema_version: u32,
    pub job_id: String,
    pub source_id: String,
    pub collection_id: Option<String>,
    pub state: String,
    pub stage: String,
    pub processed_bytes: u64,
    pub total_bytes: u64,
    pub processed_chunks: usize,
    pub total_chunks: usize,
    pub percent: u8,
    pub embedding_requests: usize,
    pub retry_count: usize,
    pub last_http_status: Option<u16>,
    pub error_code: Option<String>,
    pub elapsed_ms: u64,
    pub timestamp_ms: u64,
}

/// 进度事件的宿主侧出口。Rust 侧不直接依赖 Tauri，由 `commands` 提供实现。
pub trait KnowledgeProgressSink: Send + Sync {
    fn publish(&self, progress: KnowledgeIndexProgress);
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeIndexMetrics {
    pub jobs_queued: usize,
    pub jobs_completed: usize,
    pub jobs_reused: usize,
    pub jobs_failed: usize,
    pub jobs_cancelled: usize,
    pub chunks_indexed: usize,
    pub vectors_indexed: usize,
    pub embedding_requests: usize,
    pub embedding_retries: usize,
    pub total_index_duration_ms: u64,
    pub average_index_duration_ms: u64,
    pub last_error_code: Option<String>,
    pub last_completed_at_ms: Option<u64>,
}

impl KnowledgeIndexMetrics {
    fn refresh_average(&mut self) {
        let finished = self
            .jobs_completed
            .saturating_add(self.jobs_reused)
            .saturating_add(self.jobs_failed)
            .saturating_add(self.jobs_cancelled);
        self.average_index_duration_ms = if finished == 0 {
            0
        } else {
            self.total_index_duration_ms / finished as u64
        };
    }

    #[cfg(test)]
    fn finished_jobs(&self) -> usize {
        self.jobs_completed + self.jobs_reused + self.jobs_failed + self.jobs_cancelled
    }
}

/// 随 `KnowledgeService` 克隆共享的观测出口，因此 worker 与后续附加的宿主出口看到同一份状态。
#[derive(Clone, Default)]
struct KnowledgeObservers {
    progress: Arc<Mutex<Option<Arc<dyn KnowledgeProgressSink>>>>,
    logger: Arc<Mutex<Option<StructuredLogger>>>,
}

/// 单个索引任务的观测上下文：进度事件需要的稳定字段只查一次。
#[derive(Clone)]
struct IndexJobContext {
    collection_id: Option<String>,
    started_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSearchResult {
    pub citation_id: String,
    pub title: String,
    pub path: String,
    pub locator: String,
    pub preview: String,
    pub revision: String,
    pub score: f64,
    pub lexical_rank: usize,
    pub semantic_rank: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSearchResponse {
    pub success: bool,
    pub results: Vec<KnowledgeSearchResult>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeCitation {
    pub citation_id: String,
    pub path: String,
    pub locator: String,
    pub text: String,
    pub revision: String,
    pub is_current_revision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertCollectionRequest {
    pub id: Option<String>,
    pub name: String,
    pub scope: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddSourceRequest {
    pub collection_id: String,
    pub workspace_relative_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetEmbeddingSettingsRequest {
    pub semantic_enabled: bool,
    pub batch_size: usize,
    pub timeout_ms: u64,
    pub max_vector_scan_chunks: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingConnectionTest {
    pub connected: bool,
    pub latency_ms: u64,
    pub http_status: Option<u16>,
    pub model: String,
    pub vector_dimension: usize,
    pub usage: Option<serde_json::Value>,
    pub trace_id: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum KnowledgeError {
    #[error("{code}: {message}")]
    Coded { code: &'static str, message: String },
    #[error("knowledge storage failed: {0}")]
    Storage(String),
}

impl KnowledgeError {
    fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded {
            code,
            message: message.into(),
        }
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::Coded { code, .. } => code,
            Self::Storage(_) => "KC_STORAGE",
        }
    }
}

#[derive(Clone)]
pub struct KnowledgeService {
    db: ProjectionDb,
    repository: KnowledgeRepository,
    credentials: Arc<dyn CredentialStore>,
    citations: Arc<Mutex<HashMap<String, CitationRecord>>>,
    active_jobs: Arc<Mutex<HashMap<String, CancellationToken>>>,
    source_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
    enqueue_lock: Arc<Mutex<()>>,
    worker_notify: Arc<Notify>,
    worker_started: Arc<AtomicBool>,
    embedding_endpoint: Arc<str>,
    query_embedding_cache: Arc<Mutex<QueryEmbeddingCache>>,
    observers: KnowledgeObservers,
    metrics: Arc<Mutex<KnowledgeIndexMetrics>>,
    observed_jobs: Arc<Mutex<HashSet<String>>>,
}

#[derive(Debug, Clone)]
struct CitationRecord {
    thread_id: String,
    turn_id: String,
    chunk_id: String,
    citation: KnowledgeCitation,
}

#[derive(Debug, Default)]
struct PurgedKnowledgeRows {
    chunk_ids: HashSet<String>,
    chunk_count: usize,
    vector_count: usize,
}

#[derive(Debug, Default)]
struct QueryEmbeddingCache {
    entries: HashMap<String, (Vec<f32>, Instant)>,
    lru: VecDeque<String>,
}

const KNOWLEDGE_EVENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct KnowledgeEvent {
    schema_version: u32,
    event_id: String,
    created_at_ms: u64,
    #[serde(flatten)]
    kind: KnowledgeEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "data")]
enum KnowledgeEventKind {
    #[serde(rename = "knowledge_enabled_changed")]
    KnowledgeEnabledChanged { enabled: bool },
    #[serde(rename = "knowledge_collection_upserted")]
    CollectionUpserted {
        id: String,
        name: String,
        scope: String,
        scope_key: String,
        enabled: bool,
    },
    #[serde(rename = "knowledge_collection_deleted")]
    CollectionDeleted { id: String },
    #[serde(rename = "knowledge_source_registered")]
    SourceRegistered {
        id: String,
        collection_id: String,
        workspace_id: String,
        relative_path: String,
        size_bytes: u64,
        modified_at_ms: u64,
    },
    #[serde(rename = "knowledge_source_deleted")]
    SourceDeleted { id: String },
}

#[derive(Clone)]
struct KnowledgeRepository {
    db: ProjectionDb,
    events_path: Option<PathBuf>,
    append_lock: Arc<Mutex<()>>,
}

impl KnowledgeRepository {
    fn new(db: ProjectionDb) -> Self {
        let events_path = db
            .data_root()
            .map(|root| root.join("knowledge").join("events.jsonl"));
        Self {
            db,
            events_path,
            append_lock: Arc::new(Mutex::new(())),
        }
    }

    fn append(&self, kind: KnowledgeEventKind) -> Result<(), KnowledgeError> {
        let event = KnowledgeEvent {
            schema_version: KNOWLEDGE_EVENT_SCHEMA_VERSION,
            event_id: Uuid::new_v4().to_string(),
            created_at_ms: now_ms(),
            kind,
        };
        self.append_event(&event)
    }

    fn append_event(&self, event: &KnowledgeEvent) -> Result<(), KnowledgeError> {
        let Some(path) = self.events_path.as_ref() else {
            return Ok(());
        };
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| KnowledgeError::Storage("knowledge event lock poisoned".into()))?;
        let parent = path
            .parent()
            .ok_or_else(|| KnowledgeError::Storage("knowledge event path has no parent".into()))?;
        fs::create_dir_all(parent).map_err(|error| {
            KnowledgeError::Storage(format!("create knowledge event directory: {error}"))
        })?;
        let line = serde_json::to_string(event).map_err(|error| {
            KnowledgeError::Storage(format!("serialize knowledge event: {error}"))
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| {
                KnowledgeError::Storage(format!("open knowledge event log: {error}"))
            })?;
        file.write_all(line.as_bytes())
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_data())
            .map_err(|error| KnowledgeError::Storage(format!("append knowledge event: {error}")))
    }

    fn rebuild_projection(&self) -> Result<(), KnowledgeError> {
        let Some(path) = self.events_path.as_ref() else {
            return Ok(());
        };
        if !path.exists() {
            return self.backfill_from_projection();
        }
        let content = fs::read_to_string(path).map_err(|error| {
            KnowledgeError::Storage(format!("read knowledge event log: {error}"))
        })?;
        if content.trim().is_empty() {
            return self.backfill_from_projection();
        }
        let has_trailing_newline = content.ends_with('\n');
        for (index, raw_line) in content.split('\n').enumerate() {
            let line = raw_line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let event = match serde_json::from_str::<KnowledgeEvent>(line) {
                Ok(event) => event,
                Err(error) if !has_trailing_newline && index == content.split('\n').count() - 1 => {
                    // A process can be interrupted after writing a partial final line. Keep
                    // earlier durable facts and let the next mutation append a clean record.
                    let _ = error;
                    break;
                }
                Err(error) => {
                    return Err(KnowledgeError::Storage(format!(
                        "invalid knowledge event at line {}: {error}",
                        index + 1
                    )));
                }
            };
            if event.schema_version != KNOWLEDGE_EVENT_SCHEMA_VERSION {
                return Err(KnowledgeError::Storage(format!(
                    "unsupported knowledge event schema {}",
                    event.schema_version
                )));
            }
            self.apply_event(&event)?;
        }
        Ok(())
    }

    fn backfill_from_projection(&self) -> Result<(), KnowledgeError> {
        let facts = self
            .db
            .with_connection(|connection| {
                let enabled: Option<bool> = connection
                    .query_row(
                        "SELECT value='true' FROM settings WHERE key='knowledge.enabled'",
                        [],
                        |row| row.get(0),
                    )
                    .optional()?;
                let mut facts = Vec::new();
                if let Some(enabled) = enabled {
                    facts.push(KnowledgeEventKind::KnowledgeEnabledChanged { enabled });
                }
                let mut collections = connection.prepare(
                    "SELECT id,name,scope,scope_key,enabled,deleted FROM knowledge_collections ORDER BY created_at_ms,id",
                )?;
                let rows = collections.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)? != 0,
                        row.get::<_, i64>(5)? != 0,
                    ))
                })?;
                for row in rows {
                    let (id, name, scope, scope_key, enabled, deleted) = row?;
                    if deleted {
                        facts.push(KnowledgeEventKind::CollectionDeleted { id });
                    } else {
                        facts.push(KnowledgeEventKind::CollectionUpserted {
                            id,
                            name,
                            scope,
                            scope_key,
                            enabled,
                        });
                    }
                }
                let mut sources = connection.prepare(
                    "SELECT id,collection_id,workspace_id,relative_path,size_bytes,modified_at_ms FROM knowledge_sources ORDER BY created_at_ms,id",
                )?;
                let rows = sources.query_map([], |row| {
                    Ok(KnowledgeEventKind::SourceRegistered {
                        id: row.get(0)?,
                        collection_id: row.get(1)?,
                        workspace_id: row.get(2)?,
                        relative_path: row.get(3)?,
                        size_bytes: row.get::<_, i64>(4)? as u64,
                        modified_at_ms: row.get::<_, i64>(5)? as u64,
                    })
                })?;
                for row in rows {
                    facts.push(row?);
                }
                Ok(facts)
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        for fact in facts {
            self.append(fact)?;
        }
        Ok(())
    }

    fn apply_event(&self, event: &KnowledgeEvent) -> Result<(), KnowledgeError> {
        self.db
            .with_connection(|connection| {
                let tx = connection.transaction()?;
                match &event.kind {
                    KnowledgeEventKind::KnowledgeEnabledChanged { enabled } => {
                        tx.execute(
                            "INSERT INTO settings(key,value) VALUES('knowledge.enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                            [if *enabled { "true" } else { "false" }],
                        )?;
                    }
                    KnowledgeEventKind::CollectionUpserted {
                        id,
                        name,
                        scope,
                        scope_key,
                        enabled,
                    } => {
                        tx.execute(
                            "INSERT INTO knowledge_collections(id,name,scope,scope_key,enabled,deleted,created_at_ms,updated_at_ms)
                             VALUES(?1,?2,?3,?4,?5,0,?6,?6)
                             ON CONFLICT(id) DO UPDATE SET name=excluded.name,scope=excluded.scope,scope_key=excluded.scope_key,enabled=excluded.enabled,deleted=0,updated_at_ms=excluded.updated_at_ms",
                            params![id, name, scope, scope_key, *enabled as i64, event.created_at_ms],
                        )?;
                    }
                    KnowledgeEventKind::CollectionDeleted { id } => {
                        tx.execute(
                            "UPDATE knowledge_collections SET deleted=1,enabled=0,updated_at_ms=?2 WHERE id=?1",
                            params![id, event.created_at_ms],
                        )?;
                        purge_source_rows(&tx, "collection_id=?1", id)?;
                    }
                    KnowledgeEventKind::SourceRegistered {
                        id,
                        collection_id,
                        workspace_id,
                        relative_path,
                        size_bytes,
                        modified_at_ms,
                    } => {
                        let path_id: Option<String> = tx
                            .query_row(
                                "SELECT id FROM knowledge_sources WHERE collection_id=?1 AND workspace_id=?2 AND relative_path=?3",
                                params![collection_id, workspace_id, relative_path],
                                |row| row.get(0),
                            )
                            .optional()?;
                        if let Some(existing_id) = path_id {
                            tx.execute(
                                "UPDATE knowledge_sources SET size_bytes=?2,modified_at_ms=?3,updated_at_ms=?4 WHERE id=?1",
                                params![existing_id, *size_bytes as i64, *modified_at_ms as i64, event.created_at_ms],
                            )?;
                        } else {
                            tx.execute(
                                "INSERT INTO knowledge_sources(id,collection_id,workspace_id,relative_path,size_bytes,modified_at_ms,content_hash,state,updated_at_ms,created_at_ms)
                                 VALUES(?1,?2,?3,?4,?5,?6,NULL,'queued',?7,?7)
                                 ON CONFLICT(id) DO UPDATE SET collection_id=excluded.collection_id,workspace_id=excluded.workspace_id,relative_path=excluded.relative_path,size_bytes=excluded.size_bytes,modified_at_ms=excluded.modified_at_ms,updated_at_ms=excluded.updated_at_ms",
                                params![id, collection_id, workspace_id, relative_path, *size_bytes as i64, *modified_at_ms as i64, event.created_at_ms],
                            )?;
                        }
                    }
                    KnowledgeEventKind::SourceDeleted { id } => {
                        purge_source_rows(&tx, "id=?1", id)?;
                    }
                }
                tx.commit()?;
                Ok(())
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))
    }
}

fn purge_source_rows(
    tx: &rusqlite::Transaction<'_>,
    predicate: &str,
    value: &str,
) -> Result<(), rusqlite::Error> {
    let query = format!("SELECT id FROM knowledge_sources WHERE {predicate}");
    let source_ids = query_strings(tx, &query, value)?;
    for source_id in source_ids {
        tx.execute(
            "DELETE FROM knowledge_chunk_embeddings WHERE revision_id IN (SELECT id FROM knowledge_revisions WHERE source_id=?1)",
            [&source_id],
        )?;
        tx.execute(
            "DELETE FROM knowledge_chunks_fts WHERE revision_id IN (SELECT id FROM knowledge_revisions WHERE source_id=?1)",
            [&source_id],
        )?;
        tx.execute(
            "DELETE FROM knowledge_chunks WHERE revision_id IN (SELECT id FROM knowledge_revisions WHERE source_id=?1)",
            [&source_id],
        )?;
        tx.execute(
            "DELETE FROM knowledge_revisions WHERE source_id=?1",
            [&source_id],
        )?;
        tx.execute(
            "DELETE FROM knowledge_index_jobs WHERE source_id=?1",
            [&source_id],
        )?;
        tx.execute("DELETE FROM knowledge_sources WHERE id=?1", [&source_id])?;
    }
    Ok(())
}

impl QueryEmbeddingCache {
    fn get(&mut self, key: &str, now: Instant) -> Option<Vec<f32>> {
        self.prune_expired(now);
        let (vector, expires_at) = self.entries.get(key)?;
        if *expires_at <= now {
            self.remove(key);
            return None;
        }
        let vector = vector.clone();
        self.touch(key);
        Some(vector)
    }

    fn insert(&mut self, key: String, vector: Vec<f32>, now: Instant) {
        self.prune_expired(now);
        self.remove(&key);
        self.entries
            .insert(key.clone(), (vector, now + QUERY_EMBEDDING_CACHE_TTL));
        self.lru.push_back(key);
        while self.entries.len() > QUERY_EMBEDDING_CACHE_CAPACITY {
            if let Some(expired_key) = self.lru.pop_front() {
                self.entries.remove(&expired_key);
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.lru.clear();
    }

    fn prune_expired(&mut self, now: Instant) {
        let expired = self
            .entries
            .iter()
            .filter_map(|(key, (_, expires_at))| (*expires_at <= now).then_some(key.clone()))
            .collect::<Vec<_>>();
        for key in expired {
            self.remove(&key);
        }
    }

    fn touch(&mut self, key: &str) {
        if let Some(position) = self.lru.iter().position(|candidate| candidate == key) {
            self.lru.remove(position);
        }
        self.lru.push_back(key.to_owned());
    }

    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
        if let Some(position) = self.lru.iter().position(|candidate| candidate == key) {
            self.lru.remove(position);
        }
    }
}

struct ActiveJobGuard {
    jobs: Arc<Mutex<HashMap<String, CancellationToken>>>,
    job_id: String,
}

impl Drop for ActiveJobGuard {
    fn drop(&mut self) {
        self.jobs.lock().unwrap().remove(&self.job_id);
    }
}

impl KnowledgeService {
    pub fn new(db: ProjectionDb, credentials: Arc<dyn CredentialStore>) -> Self {
        Self::new_with_embedding_endpoint(db, credentials, Arc::from(EMBEDDING_ENDPOINT))
    }

    fn new_with_embedding_endpoint(
        db: ProjectionDb,
        credentials: Arc<dyn CredentialStore>,
        embedding_endpoint: Arc<str>,
    ) -> Self {
        let repository = KnowledgeRepository::new(db.clone());
        let _ = repository.rebuild_projection();
        let service = Self {
            db,
            repository,
            credentials,
            citations: Arc::new(Mutex::new(HashMap::new())),
            active_jobs: Arc::new(Mutex::new(HashMap::new())),
            source_locks: Arc::new(Mutex::new(HashMap::new())),
            enqueue_lock: Arc::new(Mutex::new(())),
            worker_notify: Arc::new(Notify::new()),
            worker_started: Arc::new(AtomicBool::new(false)),
            embedding_endpoint,
            query_embedding_cache: Arc::new(Mutex::new(QueryEmbeddingCache::default())),
            observers: KnowledgeObservers::default(),
            metrics: Arc::new(Mutex::new(KnowledgeIndexMetrics::default())),
            observed_jobs: Arc::new(Mutex::new(HashSet::new())),
        };
        service.recover_interrupted_jobs();
        service.ensure_worker();
        service
    }

    #[cfg(test)]
    fn new_for_test(
        db: ProjectionDb,
        credentials: Arc<dyn CredentialStore>,
        endpoint: impl Into<Arc<str>>,
    ) -> Self {
        Self::new_with_embedding_endpoint(db, credentials, endpoint.into())
    }

    /// 附加宿主侧的进度出口。可以在 worker 启动之后调用：出口由所有克隆共享。
    pub fn attach_progress_sink(&self, sink: Arc<dyn KnowledgeProgressSink>) {
        if let Ok(mut guard) = self.observers.progress.lock() {
            *guard = Some(sink);
        }
    }

    /// 附加结构化日志。日志只记录不透明 ID、计数与耗时，不记录正文、绝对路径或凭据。
    pub fn attach_logger(&self, logger: StructuredLogger) {
        if let Ok(mut guard) = self.observers.logger.lock() {
            *guard = Some(logger);
        }
    }

    pub fn metrics_snapshot(&self) -> KnowledgeIndexMetrics {
        self.metrics
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn record_metrics(&self, update: impl FnOnce(&mut KnowledgeIndexMetrics)) {
        if let Ok(mut guard) = self.metrics.lock() {
            update(&mut guard);
            guard.refresh_average();
        }
    }

    fn log_event(&self, level: &str, event: &str, fields: serde_json::Value) {
        let logger = self
            .observers
            .logger
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        if let Some(logger) = logger {
            let _ = logger.log(level, event, fields);
        }
    }

    fn progress_percent(job: &KnowledgeIndexJob) -> u8 {
        let total = job.total_chunks.max(job.total_bytes as usize);
        if total == 0 {
            return 0;
        }
        let processed = job.processed_chunks.max(job.processed_bytes as usize);
        if processed >= total {
            100
        } else {
            ((processed.saturating_mul(100)) / total).min(100) as u8
        }
    }

    fn publish_progress(&self, job: &KnowledgeIndexJob, context: &IndexJobContext) {
        let sink = match self.observers.progress.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => None,
        };
        let Some(sink) = sink else {
            return;
        };
        let timestamp_ms = now_ms();
        sink.publish(KnowledgeIndexProgress {
            schema_version: KNOWLEDGE_PROGRESS_SCHEMA_VERSION,
            job_id: job.job_id.clone(),
            source_id: job.source_id.clone(),
            collection_id: context.collection_id.clone(),
            state: job.state.clone(),
            stage: job.stage.clone(),
            processed_bytes: job.processed_bytes,
            total_bytes: job.total_bytes,
            processed_chunks: job.processed_chunks,
            total_chunks: job.total_chunks,
            percent: Self::progress_percent(job),
            embedding_requests: job.embedding_requests,
            retry_count: job.retry_count,
            last_http_status: job.last_http_status,
            error_code: job.error_code.clone(),
            elapsed_ms: timestamp_ms.saturating_sub(context.started_at_ms),
            timestamp_ms,
        });
    }

    fn job_context(&self, source_id: &str) -> IndexJobContext {
        let collection_id = self
            .db
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT collection_id FROM knowledge_sources WHERE id=?1",
                        [source_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
            })
            .ok()
            .flatten();
        IndexJobContext {
            collection_id,
            started_at_ms: now_ms(),
        }
    }

    /// 终态收口：只对已完成、复用、失败和取消发布最终进度、累计指标并写日志。
    /// 取消可能同时由用户命令和 worker 收口触发，因此同一 job 只累计一次。
    fn observe_finished_job(&self, job: &KnowledgeIndexJob, context: &IndexJobContext) {
        let state = job.state.as_str();
        if !matches!(state, "completed" | "reused" | "failed" | "cancelled") {
            return;
        }
        let first_time = match self.observed_jobs.lock() {
            Ok(mut seen) => {
                if seen.len() >= MAX_OBSERVED_JOBS {
                    seen.clear();
                }
                seen.insert(job.job_id.clone())
            }
            Err(_) => true,
        };
        if !first_time {
            self.publish_progress(job, context);
            return;
        }
        let duration_ms = job
            .completed_at_ms
            .unwrap_or_else(now_ms)
            .saturating_sub(context.started_at_ms);
        let error_code = job.error_code.clone();
        let chunk_count = job.chunk_count;
        let vector_count = job.vector_count;
        let embedding_requests = job.embedding_requests;
        self.record_metrics(|metrics| {
            match state {
                "completed" => metrics.jobs_completed += 1,
                "reused" => metrics.jobs_reused += 1,
                "failed" => metrics.jobs_failed += 1,
                "cancelled" => metrics.jobs_cancelled += 1,
                _ => {}
            }
            metrics.chunks_indexed = metrics.chunks_indexed.saturating_add(chunk_count);
            metrics.vectors_indexed = metrics.vectors_indexed.saturating_add(vector_count);
            metrics.embedding_requests = metrics
                .embedding_requests
                .saturating_add(embedding_requests);
            metrics.total_index_duration_ms =
                metrics.total_index_duration_ms.saturating_add(duration_ms);
            metrics.last_completed_at_ms = Some(now_ms());
            if let Some(code) = error_code.clone() {
                metrics.last_error_code = Some(code);
            }
        });
        self.publish_progress(job, context);
        let (level, event) = match state {
            "failed" => ("warn", "knowledge_index_job_failed"),
            "cancelled" => ("warn", "knowledge_index_job_cancelled"),
            _ => ("info", "knowledge_index_job_completed"),
        };
        self.log_event(
            level,
            event,
            json!({
                "jobId": job.job_id,
                "sourceId": job.source_id,
                "state": state,
                "embeddingMode": job.embedding_mode,
                "chunks": chunk_count,
                "vectors": vector_count,
                "embeddingRequests": embedding_requests,
                "retryCount": job.retry_count,
                "durationMs": duration_ms,
                "errorCode": error_code,
            }),
        );
    }

    fn recover_interrupted_jobs(&self) {
        let _ = self.db.with_connection(|connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "UPDATE knowledge_index_jobs SET state='queued',stage='queued',completed_at_ms=NULL WHERE state='running'",
                [],
            )?;
            tx.execute(
                "UPDATE knowledge_sources SET state='queued',updated_at_ms=?1 WHERE id IN (SELECT source_id FROM knowledge_index_jobs WHERE state='queued')",
                [now_ms()],
            )?;
            let missing_jobs = {
                let mut statement = tx.prepare(
                    "SELECT s.id,s.size_bytes FROM knowledge_sources s
                     JOIN knowledge_collections c ON c.id=s.collection_id AND c.deleted=0
                     WHERE s.state='queued' AND NOT EXISTS(
                       SELECT 1 FROM knowledge_index_jobs j WHERE j.source_id=s.id AND j.state IN ('queued','running'))
                     ORDER BY s.updated_at_ms,s.id",
                )?;
                statement
                    .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            for (source_id, size_bytes) in missing_jobs {
                let now = now_ms();
                tx.execute(
                    "INSERT INTO knowledge_index_jobs(id,source_id,stage,embedding_mode,state,processed_bytes,total_bytes,created_at_ms)
                     VALUES(?1,?2,'queued','lexical_only','queued',0,?3,?4)",
                    params![Uuid::new_v4().to_string(), source_id, size_bytes, now],
                )?;
            }
            tx.commit()?;
            Ok(())
        });
        self.worker_notify.notify_one();
    }

    fn ensure_worker(&self) {
        if tokio::runtime::Handle::try_current().is_err()
            || self
                .worker_started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            service.run_worker().await;
        });
    }

    async fn run_worker(self) {
        loop {
            match self.claim_next_job() {
                Ok(Some((job_id, source_id, workspace))) => {
                    self.run_claimed_job(&job_id, &source_id, &workspace).await;
                }
                Ok(None) => self.worker_notify.notified().await,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            }
        }
    }

    fn claim_next_job(&self) -> Result<Option<(String, String, PathBuf)>, KnowledgeError> {
        self.db
            .with_connection(|connection| {
                let tx = connection.transaction()?;
                let queued = tx
                    .query_row(
                        "SELECT j.id,j.source_id,s.workspace_id FROM knowledge_index_jobs j JOIN knowledge_sources s ON s.id=j.source_id WHERE j.state='queued' ORDER BY j.created_at_ms,j.id LIMIT 1",
                        [],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                PathBuf::from(row.get::<_, String>(2)?),
                            ))
                        },
                    )
                    .optional()?;
                let Some((job_id, source_id, workspace)) = queued else {
                    tx.commit()?;
                    return Ok(None);
                };
                let claimed = tx.execute(
                    "UPDATE knowledge_index_jobs SET state='running',stage='parse',started_at_ms=COALESCE(started_at_ms,?2),completed_at_ms=NULL WHERE id=?1 AND state='queued'",
                    params![job_id, now_ms()],
                )?;
                tx.commit()?;
                Ok((claimed == 1).then_some((job_id, source_id, workspace)))
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))
    }

    async fn run_claimed_job(&self, job_id: &str, source_id: &str, workspace: &Path) {
        let cancellation = CancellationToken::new();
        self.active_jobs
            .lock()
            .unwrap()
            .insert(job_id.to_owned(), cancellation.clone());
        let _job_guard = ActiveJobGuard {
            jobs: self.active_jobs.clone(),
            job_id: job_id.to_owned(),
        };
        let still_running = self.get_job(job_id).is_ok_and(|job| job.state == "running");
        if !still_running {
            return;
        }
        let context = self.job_context(source_id);
        if let Ok(job) = self.get_job(job_id) {
            self.publish_progress(&job, &context);
        }
        match self
            .index_source_inner(workspace, source_id, job_id, &context, cancellation)
            .await
        {
            Ok(job) => self.observe_finished_job(&job, &context),
            Err(error) if error.code() == "KC_CANCELLED" => {
                let _ = self.finish_cancelled_without_revision(job_id, source_id);
                if let Ok(job) = self.get_job(job_id) {
                    self.observe_finished_job(&job, &context);
                }
            }
            Err(error) => {
                let _ = self.finish_failed_job(job_id, source_id, error.code());
                if let Ok(job) = self.get_job(job_id) {
                    self.observe_finished_job(&job, &context);
                }
            }
        }
    }

    fn finish_cancelled_without_revision(
        &self,
        job_id: &str,
        source_id: &str,
    ) -> Result<(), KnowledgeError> {
        self.db
            .with_connection(|connection| {
                let tx = connection.transaction()?;
                let changed = tx.execute(
                    "UPDATE knowledge_index_jobs SET state='cancelled',stage='cancelled',error_code='KC_CANCELLED',completed_at_ms=?2 WHERE id=?1 AND state IN ('queued','running')",
                    params![job_id, now_ms()],
                )?;
                if changed == 1 {
                    tx.execute(
                        "UPDATE knowledge_sources SET state=CASE WHEN active_revision_id IS NULL THEN 'cancelled' ELSE 'indexed' END,last_error_code='KC_CANCELLED',updated_at_ms=?2 WHERE id=?1",
                        params![source_id, now_ms()],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))
    }

    fn finish_failed_job(
        &self,
        job_id: &str,
        source_id: &str,
        error_code: &str,
    ) -> Result<(), KnowledgeError> {
        self.db
            .with_connection(|connection| {
                let tx = connection.transaction()?;
                let changed = tx.execute(
                    "UPDATE knowledge_index_jobs SET state='failed',stage='complete',error_code=?2,completed_at_ms=?3 WHERE id=?1 AND state='running'",
                    params![job_id, error_code, now_ms()],
                )?;
                if changed == 1 {
                    tx.execute(
                        "UPDATE knowledge_sources SET state=CASE WHEN active_revision_id IS NULL THEN 'failed' ELSE 'indexed' END,last_error_code=?2,updated_at_ms=?3 WHERE id=?1",
                        params![source_id, error_code, now_ms()],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))
    }

    pub fn settings(&self) -> Result<KnowledgeSettings, KnowledgeError> {
        let embedding = self.embedding_settings()?;
        Ok(KnowledgeSettings {
            enabled: self.bool_setting("knowledge.enabled", false)?,
            auto_search: self.bool_setting("knowledge.auto_search", false)?,
            max_results: self.usize_setting(
                "knowledge.max_results",
                MAX_RESULTS,
                1,
                MAX_RESULTS,
            )?,
            max_chunk_tokens: self.usize_setting("knowledge.max_chunk_tokens", 1500, 128, 8192)?,
            knowledge_budget_percent: self.usize_setting("knowledge.budget_percent", 8, 1, 50)?,
            semantic_enabled: embedding.semantic_enabled,
            embedding_provider: embedding.provider,
            embedding_model: embedding.model,
            embedding_dimension: embedding.vector_dimension,
            embedding_configured: embedding.embedding_configured,
            embedding_status: embedding.embedding_status,
        })
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<KnowledgeSettings, KnowledgeError> {
        self.set_bool_setting("knowledge.enabled", enabled)?;
        self.repository
            .append(KnowledgeEventKind::KnowledgeEnabledChanged { enabled })?;
        self.settings()
    }

    pub fn list_collections(
        &self,
        workspace: &Path,
    ) -> Result<Vec<KnowledgeCollection>, KnowledgeError> {
        let scope_key = scope_key(workspace)?;
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.id,c.name,c.scope,c.scope_key,c.enabled,c.updated_at_ms,
                        COUNT(DISTINCT s.id),COALESCE(SUM((SELECT COUNT(*) FROM knowledge_chunks k WHERE k.revision_id=s.active_revision_id)),0)
                 FROM knowledge_collections c LEFT JOIN knowledge_sources s ON s.collection_id=c.id
                 WHERE c.deleted=0 AND c.scope_key=?1 GROUP BY c.id ORDER BY c.updated_at_ms DESC")?;
            let rows = statement.query_map([scope_key], |row| Ok(KnowledgeCollection {
                id: row.get(0)?, name: row.get(1)?, scope: row.get(2)?, scope_key: row.get(3)?,
                enabled: row.get::<_, i64>(4)? != 0, updated_at_ms: row.get(5)?,
                source_count: row.get::<_, i64>(6)? as usize, indexed_chunk_count: row.get::<_, i64>(7)? as usize,
            }))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }).map_err(|error| KnowledgeError::Storage(error.to_string()))
    }

    pub fn upsert_collection(
        &self,
        workspace: &Path,
        request: UpsertCollectionRequest,
    ) -> Result<KnowledgeCollection, KnowledgeError> {
        let name = request.name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(KnowledgeError::coded(
                "KC_INVALID_ARGUMENT",
                "collection name must contain 1-80 characters",
            ));
        }
        let scope_key = scope_key(workspace)?;
        let now = now_ms();
        let id = if let Some(id) = request.id.filter(|value| !value.trim().is_empty()) {
            let changed = self.db.with_connection(|connection| {
                connection.execute(
                    "UPDATE knowledge_collections SET name=?2,enabled=?3,updated_at_ms=?4 WHERE id=?1 AND scope_key=?5 AND deleted=0",
                    params![id, name, request.enabled as i64, now, scope_key],
                )
            }).map_err(|error| KnowledgeError::Storage(error.to_string()))?;
            if changed != 1 {
                return Err(KnowledgeError::coded(
                    "KC_NOT_FOUND",
                    "collection not found",
                ));
            }
            id
        } else {
            let id = self.db.with_connection(|connection| {
                let tx = connection.transaction()?;
                let deleted_id: Option<String> = tx.query_row(
                    "SELECT id FROM knowledge_collections WHERE scope_key=?1 AND name=?2 AND deleted=1",
                    params![scope_key, name],
                    |row| row.get(0),
                ).optional()?;
                let id = deleted_id.unwrap_or_else(|| Uuid::new_v4().to_string());
                tx.execute(
                    "INSERT INTO knowledge_collections(id,name,scope,scope_key,enabled,deleted,created_at_ms,updated_at_ms)
                     VALUES(?1,?2,'workspace',?3,?4,0,?5,?5)
                     ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled,deleted=0,updated_at_ms=excluded.updated_at_ms",
                    params![id, name, scope_key, request.enabled as i64, now],
                )?;
                tx.commit()?;
                Ok(id)
            }).map_err(|error| KnowledgeError::Storage(error.to_string()))?;
            id
        };
        let collection = self
            .list_collections(workspace)?
            .into_iter()
            .find(|value| value.id == id)
            .ok_or_else(|| {
                KnowledgeError::coded("KC_NOT_FOUND", "collection not found after save")
            })?;
        self.repository
            .append(KnowledgeEventKind::CollectionUpserted {
                id: collection.id.clone(),
                name: collection.name.clone(),
                scope: collection.scope.clone(),
                scope_key: collection.scope_key.clone(),
                enabled: collection.enabled,
            })?;
        Ok(collection)
    }

    pub async fn delete_collection(
        &self,
        workspace: &Path,
        collection_id: &str,
        confirmation: &str,
    ) -> Result<serde_json::Value, KnowledgeError> {
        if confirmation != collection_id {
            return Err(KnowledgeError::coded(
                "KC_INVALID_ARGUMENT",
                "confirmationToken must match collectionId",
            ));
        }
        let scope_key = scope_key(workspace)?;
        let (source_ids, job_ids) = {
            let _enqueue_guard = self.enqueue_lock.lock().unwrap();
            self.db.with_connection(|connection| {
                let tx = connection.transaction()?;
                let changed = tx.execute(
                    "UPDATE knowledge_collections SET deleted=1,enabled=0,updated_at_ms=?3 WHERE id=?1 AND scope_key=?2 AND deleted=0",
                    params![collection_id, scope_key, now_ms()],
                )?;
                if changed != 1 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                let source_ids = query_strings(
                    &tx,
                    "SELECT id FROM knowledge_sources WHERE collection_id=?1 ORDER BY id",
                    collection_id,
                )?;
                let job_ids = query_strings(
                    &tx,
                    "SELECT id FROM knowledge_index_jobs WHERE source_id IN (SELECT id FROM knowledge_sources WHERE collection_id=?1) AND state IN ('queued','running') ORDER BY id",
                    collection_id,
                )?;
                tx.execute(
                    "UPDATE knowledge_index_jobs SET state='cancelled',stage='cancelled',error_code='KC_CANCELLED',completed_at_ms=?2 WHERE source_id IN (SELECT id FROM knowledge_sources WHERE collection_id=?1) AND state IN ('queued','running')",
                    params![collection_id, now_ms()],
                )?;
                tx.execute(
                    "UPDATE knowledge_sources SET state='deleting',active_revision_id=NULL,updated_at_ms=?2 WHERE collection_id=?1",
                    params![collection_id, now_ms()],
                )?;
                tx.execute(
                    "UPDATE knowledge_revisions SET active=0 WHERE source_id IN (SELECT id FROM knowledge_sources WHERE collection_id=?1)",
                    [collection_id],
                )?;
                tx.commit()?;
                Ok((source_ids, job_ids))
            }).map_err(|error| if matches!(error, crate::persistence::ProjectionError::Database(rusqlite::Error::QueryReturnedNoRows)) { KnowledgeError::coded("KC_NOT_FOUND", "collection not found") } else { KnowledgeError::Storage(error.to_string()) })?
        };
        self.cancel_job_tokens(&job_ids);
        let guards = self.lock_sources(&source_ids).await;
        let purged = self.purge_sources(&source_ids)?;
        drop(guards);
        self.remove_deleted_runtime_state(&source_ids, &purged.chunk_ids);
        self.repository
            .append(KnowledgeEventKind::CollectionDeleted {
                id: collection_id.to_owned(),
            })?;
        Ok(json!({
            "deletedCollectionId": collection_id,
            "deletedSourceCount": source_ids.len(),
            "cancelledJobCount": job_ids.len(),
            "purgedChunkCount": purged.chunk_count,
            "purgedVectorCount": purged.vector_count,
        }))
    }

    pub async fn delete_source(
        &self,
        workspace: &Path,
        source_id: &str,
        confirmation: &str,
    ) -> Result<serde_json::Value, KnowledgeError> {
        if confirmation != source_id {
            return Err(KnowledgeError::coded(
                "KC_INVALID_ARGUMENT",
                "confirmationToken must match sourceId",
            ));
        }
        let scope_key = scope_key(workspace)?;
        let job_ids = {
            let _enqueue_guard = self.enqueue_lock.lock().unwrap();
            self.db.with_connection(|connection| {
                let tx = connection.transaction()?;
                let exists: Option<i64> = tx.query_row(
                    "SELECT 1 FROM knowledge_sources s JOIN knowledge_collections c ON c.id=s.collection_id WHERE s.id=?1 AND s.workspace_id=?2 AND c.deleted=0 AND s.state!='deleting'",
                    params![source_id, scope_key],
                    |row| row.get(0),
                ).optional()?;
                if exists.is_none() {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                let job_ids = query_strings(
                    &tx,
                    "SELECT id FROM knowledge_index_jobs WHERE source_id=?1 AND state IN ('queued','running') ORDER BY id",
                    source_id,
                )?;
                tx.execute(
                    "UPDATE knowledge_index_jobs SET state='cancelled',stage='cancelled',error_code='KC_CANCELLED',completed_at_ms=?2 WHERE source_id=?1 AND state IN ('queued','running')",
                    params![source_id, now_ms()],
                )?;
                tx.execute(
                    "UPDATE knowledge_sources SET state='deleting',active_revision_id=NULL,updated_at_ms=?2 WHERE id=?1",
                    params![source_id, now_ms()],
                )?;
                tx.execute(
                    "UPDATE knowledge_revisions SET active=0 WHERE source_id=?1",
                    [source_id],
                )?;
                tx.commit()?;
                Ok(job_ids)
            }).map_err(|error| if matches!(error, crate::persistence::ProjectionError::Database(rusqlite::Error::QueryReturnedNoRows)) { KnowledgeError::coded("KC_NOT_FOUND", "source not found") } else { KnowledgeError::Storage(error.to_string()) })?
        };
        self.cancel_job_tokens(&job_ids);
        let source_ids = vec![source_id.to_owned()];
        let guards = self.lock_sources(&source_ids).await;
        let purged = self.purge_sources(&source_ids)?;
        drop(guards);
        self.remove_deleted_runtime_state(&source_ids, &purged.chunk_ids);
        self.repository.append(KnowledgeEventKind::SourceDeleted {
            id: source_id.to_owned(),
        })?;
        Ok(json!({
            "deletedSourceId": source_id,
            "cancelledJobCount": job_ids.len(),
            "purgedChunkCount": purged.chunk_count,
            "purgedVectorCount": purged.vector_count,
        }))
    }

    fn cancel_job_tokens(&self, job_ids: &[String]) {
        let active_jobs = self.active_jobs.lock().unwrap();
        for job_id in job_ids {
            if let Some(cancellation) = active_jobs.get(job_id) {
                cancellation.cancel();
            }
        }
    }

    async fn lock_sources(&self, source_ids: &[String]) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
        let locks = {
            let mut source_locks = self.source_locks.lock().unwrap();
            source_ids
                .iter()
                .map(|source_id| {
                    source_locks
                        .entry(source_id.clone())
                        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                        .clone()
                })
                .collect::<Vec<_>>()
        };
        let mut guards = Vec::with_capacity(locks.len());
        for lock in locks {
            guards.push(lock.lock_owned().await);
        }
        guards
    }

    fn purge_sources(&self, source_ids: &[String]) -> Result<PurgedKnowledgeRows, KnowledgeError> {
        self.db
            .with_connection(|connection| {
                let tx = connection.transaction()?;
                let mut purged = PurgedKnowledgeRows::default();
                for source_id in source_ids {
                    let chunk_ids = query_strings(
                        &tx,
                        "SELECT id FROM knowledge_chunks WHERE revision_id IN (SELECT id FROM knowledge_revisions WHERE source_id=?1)",
                        source_id,
                    )?;
                    purged.chunk_count += chunk_ids.len();
                    purged.chunk_ids.extend(chunk_ids);
                    purged.vector_count += tx.query_row(
                        "SELECT COUNT(*) FROM knowledge_chunk_embeddings WHERE revision_id IN (SELECT id FROM knowledge_revisions WHERE source_id=?1)",
                        [source_id],
                        |row| row.get::<_, i64>(0),
                    )? as usize;
                    tx.execute(
                        "DELETE FROM knowledge_chunk_embeddings WHERE revision_id IN (SELECT id FROM knowledge_revisions WHERE source_id=?1)",
                        [source_id],
                    )?;
                    tx.execute(
                        "DELETE FROM knowledge_chunks_fts WHERE revision_id IN (SELECT id FROM knowledge_revisions WHERE source_id=?1)",
                        [source_id],
                    )?;
                    tx.execute(
                        "DELETE FROM knowledge_chunks WHERE revision_id IN (SELECT id FROM knowledge_revisions WHERE source_id=?1)",
                        [source_id],
                    )?;
                    tx.execute(
                        "DELETE FROM knowledge_revisions WHERE source_id=?1",
                        [source_id],
                    )?;
                    tx.execute("DELETE FROM knowledge_sources WHERE id=?1", [source_id])?;
                }
                tx.commit()?;
                Ok(purged)
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))
    }

    fn remove_deleted_runtime_state(&self, source_ids: &[String], chunk_ids: &HashSet<String>) {
        let source_ids = source_ids.iter().collect::<HashSet<_>>();
        self.source_locks
            .lock()
            .unwrap()
            .retain(|source_id, _| !source_ids.contains(source_id));
        self.citations
            .lock()
            .unwrap()
            .retain(|_, citation| !chunk_ids.contains(&citation.chunk_id));
    }

    pub async fn add_source(
        &self,
        workspace: &Path,
        request: AddSourceRequest,
    ) -> Result<KnowledgeSource, KnowledgeError> {
        let root = canonical_workspace(workspace)?;
        let path = resolve_source_path(&root, &request.workspace_relative_path)?;
        let metadata = std::fs::metadata(&path).map_err(|error| {
            KnowledgeError::coded("KC_PATH_OUTSIDE_WORKSPACE", error.to_string())
        })?;
        if metadata.len() > MAX_SOURCE_BYTES {
            return Err(KnowledgeError::coded(
                "KC_SOURCE_TOO_LARGE",
                "source exceeds bounded size",
            ));
        }
        let relative = path
            .strip_prefix(&root)
            .map_err(|_| {
                KnowledgeError::coded(
                    "KC_PATH_OUTSIDE_WORKSPACE",
                    "source path is outside workspace",
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let workspace_id = scope_key(&root)?;
        self.db.with_connection(|connection| connection.query_row("SELECT 1 FROM knowledge_collections WHERE id=?1 AND scope_key=?2 AND deleted=0", params![request.collection_id, workspace_id], |_| Ok(()))).map_err(|error| match error { crate::persistence::ProjectionError::Database(rusqlite::Error::QueryReturnedNoRows) => KnowledgeError::coded("KC_NOT_FOUND", "collection not found"), other => KnowledgeError::Storage(other.to_string()) })?;
        let source_id = Uuid::new_v4().to_string();
        self.db.with_connection(|connection| {
            connection.execute(
                "INSERT INTO knowledge_sources(id,collection_id,workspace_id,relative_path,size_bytes,modified_at_ms,content_hash,state,updated_at_ms,created_at_ms)
                 VALUES(?1,?2,?3,?4,?5,?6,NULL,'queued',?6,?6)
                 ON CONFLICT(collection_id,workspace_id,relative_path) DO UPDATE SET size_bytes=excluded.size_bytes,modified_at_ms=excluded.modified_at_ms,state='queued',updated_at_ms=excluded.updated_at_ms",
                params![source_id, request.collection_id, workspace_id, relative, metadata.len(), modified_ms(&metadata)])?;
            Ok(())
        }).map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        let actual_id = self.db.with_connection(|connection| connection.query_row("SELECT id FROM knowledge_sources WHERE collection_id=?1 AND workspace_id=?2 AND relative_path=?3", params![request.collection_id,workspace_id,relative], |row| row.get::<_,String>(0))).map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        let job = self.enqueue_source(&root, &actual_id)?;
        let mut value = self.source_view(&actual_id)?;
        value.initial_job_id = Some(job.job_id);
        self.repository
            .append(KnowledgeEventKind::SourceRegistered {
                id: actual_id,
                collection_id: request.collection_id,
                workspace_id,
                relative_path: relative,
                size_bytes: metadata.len(),
                modified_at_ms: modified_ms(&metadata),
            })?;
        Ok(value)
    }

    pub fn list_sources(
        &self,
        workspace: &Path,
        collection_id: &str,
    ) -> Result<Vec<KnowledgeSource>, KnowledgeError> {
        let scope_key = scope_key(workspace)?;
        self.db.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id FROM knowledge_sources WHERE collection_id=?1 AND workspace_id=?2 ORDER BY relative_path")?;
            let ids = statement.query_map(params![collection_id,scope_key], |row| row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            Ok(ids)
        }).map_err(|error| KnowledgeError::Storage(error.to_string()))?.into_iter().map(|id| self.source_view(&id)).collect()
    }

    pub async fn refresh_source(
        &self,
        workspace: &Path,
        source_id: &str,
    ) -> Result<KnowledgeIndexJob, KnowledgeError> {
        let root = canonical_workspace(workspace)?;
        self.enqueue_source(&root, source_id)
    }

    fn enqueue_source(
        &self,
        root: &Path,
        source_id: &str,
    ) -> Result<KnowledgeIndexJob, KnowledgeError> {
        let _enqueue_guard = self.enqueue_lock.lock().unwrap();
        let scope = scope_key(root)?;
        let existing_job = self
            .db
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT j.id FROM knowledge_index_jobs j JOIN knowledge_sources s ON s.id=j.source_id JOIN knowledge_collections c ON c.id=s.collection_id WHERE j.source_id=?1 AND s.workspace_id=?2 AND s.state!='deleting' AND c.deleted=0 AND j.state IN ('queued','running') ORDER BY j.created_at_ms LIMIT 1",
                        params![source_id, scope],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        if let Some(job_id) = existing_job {
            self.ensure_worker();
            self.worker_notify.notify_one();
            return self.get_job(&job_id);
        }
        let source_exists = self
            .db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT 1 FROM knowledge_sources s JOIN knowledge_collections c ON c.id=s.collection_id WHERE s.id=?1 AND s.workspace_id=?2 AND s.state!='deleting' AND c.deleted=0",
                    params![source_id, scope],
                    |_| Ok(()),
                )
            })
            .map_err(|error| match error {
                crate::persistence::ProjectionError::Database(
                    rusqlite::Error::QueryReturnedNoRows,
                ) => KnowledgeError::coded("KC_NOT_FOUND", "source not found"),
                other => KnowledgeError::Storage(other.to_string()),
            });
        source_exists?;
        let job_id = Uuid::new_v4().to_string();
        let now = now_ms();
        self.db
            .with_connection(|connection| {
                let tx = connection.transaction()?;
                tx.execute(
                    "INSERT INTO knowledge_index_jobs(id,source_id,stage,embedding_mode,state,created_at_ms) VALUES(?1,?2,'queued','lexical_only','queued',?3)",
                    params![job_id, source_id, now],
                )?;
                tx.execute(
                    "UPDATE knowledge_sources SET state='queued',last_error_code=NULL,updated_at_ms=?2 WHERE id=?1",
                    params![source_id, now],
                )?;
                tx.commit()?;
                Ok(())
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        self.ensure_worker();
        self.worker_notify.notify_one();
        let job = self.get_job(&job_id)?;
        let context = self.job_context(&job.source_id);
        self.record_metrics(|metrics| metrics.jobs_queued += 1);
        self.publish_progress(&job, &context);
        Ok(job)
    }

    pub fn get_job(&self, job_id: &str) -> Result<KnowledgeIndexJob, KnowledgeError> {
        self.db.with_connection(|connection| {
            connection.query_row("SELECT id,source_id,state,stage,embedding_mode,processed_bytes,total_bytes,processed_chunks,total_chunks,embedding_requests,retry_count,last_http_status,chunk_count,vector_count,error_code,created_at_ms,completed_at_ms FROM knowledge_index_jobs WHERE id=?1", [job_id], map_job)
        }).map_err(|error| match error { crate::persistence::ProjectionError::Database(rusqlite::Error::QueryReturnedNoRows) => KnowledgeError::coded("KC_NOT_FOUND", "index job not found"), other => KnowledgeError::Storage(other.to_string()) })
    }

    pub fn cancel_job(&self, job_id: &str) -> Result<KnowledgeIndexJob, KnowledgeError> {
        if let Some(cancellation) = self.active_jobs.lock().unwrap().get(job_id).cloned() {
            cancellation.cancel();
        }
        self.db.with_connection(|connection| {
            let tx = connection.transaction()?;
            let source_id: Option<String> = tx
                .query_row(
                    "SELECT source_id FROM knowledge_index_jobs WHERE id=?1",
                    [job_id],
                    |row| row.get(0),
                )
                .optional()?;
            let changed = tx.execute(
                "UPDATE knowledge_index_jobs SET state='cancelled',stage='cancelled',error_code='KC_CANCELLED',completed_at_ms=?2 WHERE id=?1 AND state IN ('queued','running')",
                params![job_id, now_ms()],
            )?;
            if changed == 1 {
                if let Some(source_id) = source_id {
                    tx.execute(
                        "UPDATE knowledge_sources SET state=CASE WHEN active_revision_id IS NULL THEN 'cancelled' ELSE 'indexed' END,last_error_code='KC_CANCELLED',updated_at_ms=?2 WHERE id=?1",
                        params![source_id, now_ms()],
                    )?;
                }
            }
            tx.commit()?;
            Ok(())
        }).map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        let job = self.get_job(job_id)?;
        let context = self.job_context(&job.source_id);
        self.observe_finished_job(&job, &context);
        Ok(job)
    }

    pub fn embedding_settings(&self) -> Result<EmbeddingSettings, KnowledgeError> {
        let semantic_enabled = self.bool_setting("embedding.semantic_enabled", false)?;
        let batch_size = self.usize_setting("embedding.batch_size", 16, 1, 32)?;
        let timeout_ms = self.u64_setting("embedding.timeout_ms", 30_000, 1_000, 120_000)?;
        let max_scan =
            self.usize_setting("embedding.max_vector_scan_chunks", 10_000, 100, 100_000)?;
        let configured = self
            .credentials
            .get_api_key(EMBEDDING_CREDENTIAL)
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?
            .is_some();
        Ok(EmbeddingSettings {
            provider: EMBEDDING_PROVIDER.into(),
            endpoint: EMBEDDING_ENDPOINT.into(),
            model: EMBEDDING_MODEL.into(),
            semantic_enabled,
            encoding_format: EMBEDDING_ENCODING.into(),
            batch_size,
            timeout_ms,
            max_vector_scan_chunks: max_scan,
            model_max_input_tokens: 8192,
            vector_dimension: self.usize_setting("embedding.vector_dimension", 0, 0, 4096)?,
            embedding_configured: configured,
            embedding_status: if semantic_enabled
                && configured
                && self.usize_setting("embedding.vector_dimension", 0, 0, 4096)? > 0
            {
                "semantic_ready"
            } else {
                "lexical_only"
            }
            .into(),
        })
    }

    pub fn set_embedding_settings(
        &self,
        request: SetEmbeddingSettingsRequest,
    ) -> Result<EmbeddingSettings, KnowledgeError> {
        if !(1..=32).contains(&request.batch_size)
            || !(1_000..=120_000).contains(&request.timeout_ms)
            || !(100..=100_000).contains(&request.max_vector_scan_chunks)
        {
            return Err(KnowledgeError::coded(
                "KC_INVALID_ARGUMENT",
                "embedding limits are outside bounds",
            ));
        }
        self.set_bool_setting("embedding.semantic_enabled", request.semantic_enabled)?;
        self.set_usize_setting("embedding.batch_size", request.batch_size)?;
        self.set_u64_setting("embedding.timeout_ms", request.timeout_ms)?;
        self.set_usize_setting(
            "embedding.max_vector_scan_chunks",
            request.max_vector_scan_chunks,
        )?;
        self.query_embedding_cache.lock().unwrap().clear();
        self.embedding_settings()
    }

    pub fn set_embedding_key(&self, api_key: &str) -> Result<serde_json::Value, KnowledgeError> {
        let value = api_key.trim();
        if value.is_empty() || value.len() > 512 {
            return Err(KnowledgeError::coded(
                "KC_INVALID_ARGUMENT",
                "apiKey must contain 1-512 characters",
            ));
        }
        self.credentials
            .set_api_key(EMBEDDING_CREDENTIAL, value)
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        self.query_embedding_cache.lock().unwrap().clear();
        Ok(json!({"embeddingConfigured":true,"provider":EMBEDDING_PROVIDER}))
    }

    pub fn delete_embedding_key(&self) -> Result<serde_json::Value, KnowledgeError> {
        self.credentials
            .delete_api_key(EMBEDDING_CREDENTIAL)
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        self.query_embedding_cache.lock().unwrap().clear();
        Ok(
            json!({"embeddingConfigured":false,"embeddingStatus":"lexical_only","provider":EMBEDDING_PROVIDER}),
        )
    }

    pub async fn test_embedding_connection(
        &self,
    ) -> Result<EmbeddingConnectionTest, KnowledgeError> {
        let key = self
            .credentials
            .get_api_key(EMBEDDING_CREDENTIAL)
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?
            .ok_or_else(|| {
                KnowledgeError::coded(
                    "KC_EMBEDDING_NOT_CONFIGURED",
                    "embedding credential is not configured",
                )
            })?;
        let timeout_ms = self.u64_setting("embedding.timeout_ms", 30_000, 1_000, 120_000)?;
        let started = std::time::Instant::now();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .map_err(|_| {
                KnowledgeError::coded("KC_EMBEDDING_UNAVAILABLE", "embedding client unavailable")
            })?;
        let (response, _retry_count) = send_embedding_request(
            &client,
            &self.embedding_endpoint,
            &key,
            json!({
                "model": EMBEDDING_MODEL,
                "input": "k-coder embedding connection test",
                "encoding_format": EMBEDDING_ENCODING
            }),
            &CancellationToken::new(),
        )
        .await?;
        let status = response.status().as_u16();
        let trace_id = response
            .headers()
            .get("x-siliconcloud-trace-id")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.chars().take(128).collect::<String>());
        if status != 200 {
            let code = match status {
                401 | 403 => "KC_EMBEDDING_AUTH_FAILED",
                404 => "KC_EMBEDDING_MODEL_NOT_FOUND",
                429 => "KC_EMBEDDING_RATE_LIMITED",
                503 | 504 => "KC_EMBEDDING_UNAVAILABLE",
                _ => "KC_EMBEDDING_RESPONSE_INVALID",
            };
            return Err(KnowledgeError::coded(
                code,
                format!("embedding request returned HTTP {status}"),
            ));
        }
        let payload: serde_json::Value = response.json().await.map_err(|_| {
            KnowledgeError::coded(
                "KC_EMBEDDING_RESPONSE_INVALID",
                "embedding response is not valid JSON",
            )
        })?;
        if payload.get("object").and_then(|value| value.as_str()) != Some("list") {
            return Err(KnowledgeError::coded(
                "KC_EMBEDDING_RESPONSE_INVALID",
                "embedding response object is invalid",
            ));
        }
        let model = payload
            .get("model")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let data = payload
            .get("data")
            .and_then(|value| value.as_array())
            .ok_or_else(|| {
                KnowledgeError::coded(
                    "KC_EMBEDDING_RESPONSE_INVALID",
                    "embedding response data is invalid",
                )
            })?;
        if data.len() != 1 {
            return Err(KnowledgeError::coded(
                "KC_EMBEDDING_RESPONSE_INVALID",
                "embedding response item count is invalid",
            ));
        }
        let vector = data
            .first()
            .and_then(|value| value.get("embedding"))
            .and_then(|value| value.as_array())
            .ok_or_else(|| {
                KnowledgeError::coded(
                    "KC_EMBEDDING_RESPONSE_INVALID",
                    "embedding vector is invalid",
                )
            })?;
        if data[0].get("index").and_then(|value| value.as_u64()) != Some(0) {
            return Err(KnowledgeError::coded(
                "KC_EMBEDDING_RESPONSE_INVALID",
                "embedding response index is invalid",
            ));
        }
        if model != EMBEDDING_MODEL
            || vector.is_empty()
            || vector.len() > 4096
            || vector
                .iter()
                .any(|value| !value.as_f64().is_some_and(f64::is_finite))
        {
            return Err(KnowledgeError::coded(
                "KC_EMBEDDING_RESPONSE_INVALID",
                "embedding response failed model or vector validation",
            ));
        }
        self.set_usize_setting("embedding.vector_dimension", vector.len())?;
        Ok(EmbeddingConnectionTest {
            connected: true,
            latency_ms: started.elapsed().as_millis() as u64,
            http_status: Some(status),
            model: model.to_string(),
            vector_dimension: vector.len(),
            usage: payload.get("usage").cloned(),
            trace_id,
            error_code: None,
        })
    }

    pub async fn search(
        &self,
        workspace: &Path,
        thread_id: &str,
        turn_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<KnowledgeSearchResponse, KnowledgeError> {
        let started = Instant::now();
        let query = query.trim();
        if query.is_empty() {
            return Err(KnowledgeError::coded(
                "KC_QUERY_EMPTY",
                "knowledge query is empty",
            ));
        }
        let limit = limit.clamp(1, MAX_RESULTS);
        if !self.bool_setting("knowledge.enabled", false)? {
            return Ok(KnowledgeSearchResponse {
                success: true,
                results: vec![],
                metadata: json!({"retrievalMode":"disabled"}),
            });
        }
        let scope_key = scope_key(workspace)?;
        let terms = fts_query(query);
        if terms.is_empty() {
            return Err(KnowledgeError::coded(
                "KC_QUERY_EMPTY",
                "knowledge query has no searchable terms",
            ));
        }
        let lexical_limit = limit.saturating_mul(8).clamp(limit, 50);
        let rows = self.db.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT k.id,k.title,k.text,s.relative_path,r.id,k.start_line,k.end_line
                 FROM knowledge_chunks_fts f JOIN knowledge_chunks k ON k.id=f.chunk_id
                 JOIN knowledge_revisions r ON r.id=k.revision_id AND r.active=1
                 JOIN knowledge_sources s ON s.id=r.source_id
                 JOIN knowledge_collections c ON c.id=s.collection_id AND c.enabled=1 AND c.deleted=0
                 WHERE c.scope_key=?1 AND knowledge_chunks_fts MATCH ?2 ORDER BY bm25(knowledge_chunks_fts) LIMIT ?3")?;
            let mapped = statement.query_map(params![scope_key,terms,lexical_limit as i64], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,i64>(5)?,row.get::<_,i64>(6)?)))?.collect::<Result<Vec<_>,_>>()?;
            Ok(mapped)
        }).map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        let mut semantic_ranks = HashMap::<String, usize>::new();
        let mut retrieval_mode = "lexical_only";
        let mut fallback_code = if self.embedding_settings()?.semantic_enabled {
            Some("KC_EMBEDDING_UNAVAILABLE")
        } else {
            Some("KC_EMBEDDING_NOT_CONFIGURED")
        };
        let embedding = self.embedding_settings()?;
        if embedding.semantic_enabled && embedding.embedding_configured {
            match self.embed_query(query).await {
                Ok(query_vector) => {
                    let scan_limit = embedding.max_vector_scan_chunks;
                    let vector_rows = self.db.with_connection(|connection| {
                    let mut statement = connection.prepare(
                        "SELECT k.id,e.vector,e.dimension FROM knowledge_chunk_embeddings e
                         JOIN knowledge_chunks k ON k.id=e.chunk_id
                         JOIN knowledge_revisions r ON r.id=k.revision_id AND r.active=1 AND r.embedding_status='semantic_ready'
                         JOIN knowledge_sources s ON s.id=r.source_id
                         JOIN knowledge_collections c ON c.id=s.collection_id AND c.enabled=1 AND c.deleted=0
                         WHERE c.scope_key=?1 AND e.provider=?2 AND e.model=?3 ORDER BY k.id LIMIT ?4")?;
                    statement.query_map(params![scope_key, EMBEDDING_PROVIDER, EMBEDDING_MODEL, scan_limit as i64], |row| Ok((row.get::<_,String>(0)?,row.get::<_,Vec<u8>>(1)?,row.get::<_,i64>(2)? as usize)))?.collect::<Result<Vec<_>,_>>()
                }).map_err(|error| KnowledgeError::Storage(error.to_string()))?;
                    let mut ranked = vector_rows
                        .into_iter()
                        .filter_map(|(id, bytes, dimension)| {
                            if dimension == 0 || bytes.len() != dimension.saturating_mul(4) {
                                return None;
                            }
                            let vector = bytes
                                .chunks_exact(4)
                                .map(|chunk| {
                                    f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
                                })
                                .collect::<Vec<_>>();
                            Some((id, cosine(&query_vector, &vector)))
                        })
                        .filter(|(_, score)| score.is_finite())
                        .collect::<Vec<_>>();
                    ranked.sort_by(|left, right| {
                        right
                            .1
                            .partial_cmp(&left.1)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    for (rank, (id, _score)) in
                        ranked.into_iter().take(limit.saturating_mul(2)).enumerate()
                    {
                        semantic_ranks.insert(id, rank + 1);
                    }
                    if !semantic_ranks.is_empty() {
                        retrieval_mode = "hybrid";
                        fallback_code = None;
                    } else {
                        fallback_code = Some("KC_EMBEDDING_RESPONSE_INVALID");
                    }
                }
                Err(error) => {
                    fallback_code = Some(error.code());
                }
            }
        }
        let mut candidates = rows
            .into_iter()
            .enumerate()
            .map(|(rank, row)| (row, rank + 1))
            .collect::<Vec<_>>();
        let lexical_ids = candidates
            .iter()
            .map(|(row, _)| row.0.clone())
            .collect::<std::collections::HashSet<_>>();
        if !semantic_ranks.is_empty() {
            let missing = semantic_ranks
                .keys()
                .filter(|id| !lexical_ids.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                let semantic_rows = self.db.with_connection(|connection| {
                    let mut statement = connection.prepare("SELECT k.id,k.title,k.text,s.relative_path,r.id,k.start_line,k.end_line FROM knowledge_chunks k JOIN knowledge_revisions r ON r.id=k.revision_id AND r.active=1 AND r.embedding_status='semantic_ready' JOIN knowledge_sources s ON s.id=r.source_id JOIN knowledge_collections c ON c.id=s.collection_id AND c.enabled=1 AND c.deleted=0 WHERE c.scope_key=?1 AND k.id IN (SELECT chunk_id FROM knowledge_chunk_embeddings WHERE provider=?2 AND model=?3) ORDER BY k.id LIMIT ?4")?;
                    statement.query_map(params![scope_key,EMBEDDING_PROVIDER,EMBEDDING_MODEL, embedding.max_vector_scan_chunks as i64], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,i64>(5)?,row.get::<_,i64>(6)?)))?.collect::<Result<Vec<_>,_>>()
                }).map_err(|error| KnowledgeError::Storage(error.to_string()))?;
                for row in semantic_rows {
                    if !lexical_ids.contains(&row.0) && semantic_ranks.contains_key(&row.0) {
                        candidates.push((row, 0));
                    }
                }
            }
            candidates.sort_by(|left, right| {
                let left_score = rrf_score(left.1, semantic_ranks.get(&left.0.0).copied());
                let right_score = rrf_score(right.1, semantic_ranks.get(&right.0.0).copied());
                right_score
                    .partial_cmp(&left_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        let mut results = Vec::new();
        let mut citations = self
            .citations
            .lock()
            .map_err(|_| KnowledgeError::Storage("citation lock poisoned".into()))?;
        let candidate_count = candidates.len();
        for (rank, ((chunk_id, title, text, path, revision, start, end), lexical_rank)) in
            candidates.into_iter().take(limit).enumerate()
        {
            if citations.len() >= MAX_CITATIONS {
                citations.clear();
            }
            let citation_id = Uuid::new_v4().to_string();
            let locator = format!("L{}-{}", start, end);
            let citation = KnowledgeCitation {
                citation_id: citation_id.clone(),
                path: path.clone(),
                locator: locator.clone(),
                text: text.clone(),
                revision: revision.clone(),
                is_current_revision: true,
            };
            citations.insert(
                citation_id.clone(),
                CitationRecord {
                    thread_id: thread_id.into(),
                    turn_id: turn_id.into(),
                    chunk_id: chunk_id.clone(),
                    citation,
                },
            );
            results.push(KnowledgeSearchResult {
                citation_id,
                title,
                path,
                locator,
                preview: preview(&text),
                revision,
                score: if !semantic_ranks.is_empty() {
                    rrf_score(lexical_rank, semantic_ranks.get(&chunk_id).copied())
                } else {
                    1.0 / (rank as f64 + 1.0)
                },
                lexical_rank,
                semantic_rank: semantic_ranks.get(&chunk_id).copied(),
            });
            let _ = chunk_id;
        }
        let returned = results.len();
        self.log_event(
            "info",
            "knowledge_search_completed",
            json!({
                "queryHash": hash_text(query).chars().take(12).collect::<String>(),
                "retrievalMode": retrieval_mode,
                "candidates": candidate_count,
                "returned": returned,
                "truncated": returned >= limit,
                "fallbackCode": fallback_code,
                "durationMs": started.elapsed().as_millis() as u64,
            }),
        );
        Ok(KnowledgeSearchResponse {
            success: true,
            results,
            metadata: json!({"retrievalMode":retrieval_mode,"embeddingModel":EMBEDDING_MODEL,"fallbackCode":fallback_code}),
        })
    }

    async fn embed_query(&self, query: &str) -> Result<Vec<f32>, KnowledgeError> {
        let key = self
            .credentials
            .get_api_key(EMBEDDING_CREDENTIAL)
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?
            .ok_or_else(|| {
                KnowledgeError::coded(
                    "KC_EMBEDDING_NOT_CONFIGURED",
                    "embedding credential is not configured",
                )
            })?;
        let cache_key = query_embedding_cache_key(query);
        if let Some(vector) = self
            .query_embedding_cache
            .lock()
            .unwrap()
            .get(&cache_key, Instant::now())
        {
            return Ok(vector);
        }
        let timeout_ms = self.u64_setting("embedding.timeout_ms", 30_000, 1_000, 120_000)?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .map_err(|_| {
                KnowledgeError::coded("KC_EMBEDDING_UNAVAILABLE", "embedding client unavailable")
            })?;
        let (response, _) = send_embedding_request(
            &client,
            &self.embedding_endpoint,
            &key,
            json!({"model":EMBEDDING_MODEL,"input":query,"encoding_format":EMBEDDING_ENCODING}),
            &CancellationToken::new(),
        )
        .await?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(KnowledgeError::coded(
                match status {
                    401 | 403 => "KC_EMBEDDING_AUTH_FAILED",
                    404 => "KC_EMBEDDING_MODEL_NOT_FOUND",
                    429 => "KC_EMBEDDING_RATE_LIMITED",
                    503 | 504 => "KC_EMBEDDING_UNAVAILABLE",
                    _ => "KC_EMBEDDING_RESPONSE_INVALID",
                },
                format!("embedding request returned HTTP {status}"),
            ));
        }
        let payload: serde_json::Value = response.json().await.map_err(|_| {
            KnowledgeError::coded(
                "KC_EMBEDDING_RESPONSE_INVALID",
                "embedding response is not valid JSON",
            )
        })?;
        let vector =
            validate_embedding_payload(&payload, 1).map(|mut vectors| vectors.remove(0))?;
        self.query_embedding_cache.lock().unwrap().insert(
            cache_key,
            vector.clone(),
            Instant::now(),
        );
        Ok(vector)
    }

    pub fn read_citation(
        &self,
        thread_id: &str,
        turn_id: &str,
        citation_id: &str,
        before: usize,
        after: usize,
    ) -> Result<KnowledgeCitation, KnowledgeError> {
        if before > 1 || after > 1 {
            return Err(KnowledgeError::coded(
                "KC_INVALID_ARGUMENT",
                "citation context window is limited to one",
            ));
        }
        let citations = self
            .citations
            .lock()
            .map_err(|_| KnowledgeError::Storage("citation lock poisoned".into()))?;
        let record = citations.get(citation_id).ok_or_else(|| {
            KnowledgeError::coded("KC_CITATION_FORBIDDEN", "citation is unknown or expired")
        })?;
        if record.thread_id != thread_id || record.turn_id != turn_id {
            self.log_event(
                "warn",
                "knowledge_citation_rejected",
                json!({ "reason": "turn_mismatch" }),
            );
            return Err(KnowledgeError::coded(
                "KC_CITATION_FORBIDDEN",
                "citation is not bound to this turn",
            ));
        }
        let chunk_id = record.chunk_id.clone();
        let base = record.citation.clone();
        drop(citations);
        let (revision_id, ordinal): (String, i64) = self
            .db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT k.revision_id,k.ordinal FROM knowledge_chunks k
                     JOIN knowledge_revisions r ON r.id=k.revision_id AND r.active=1
                     JOIN knowledge_sources s ON s.id=r.source_id AND s.active_revision_id=r.id AND s.state!='deleting'
                     JOIN knowledge_collections c ON c.id=s.collection_id AND c.enabled=1 AND c.deleted=0
                     WHERE k.id=?1 AND r.id=?2",
                    params![chunk_id, &base.revision],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .map_err(|error| {
                if matches!(
                    error,
                    crate::persistence::ProjectionError::Database(
                        rusqlite::Error::QueryReturnedNoRows
                    )
                ) {
                    KnowledgeError::coded(
                        "KC_CITATION_STALE",
                        "citation source is no longer active",
                    )
                } else {
                    KnowledgeError::Storage(error.to_string())
                }
            })?;
        if before == 0 && after == 0 {
            return Ok(base);
        }
        let context = self
            .db
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT text,start_line,end_line FROM knowledge_chunks
                     WHERE revision_id=?1 AND ordinal BETWEEN ?2 AND ?3 ORDER BY ordinal",
                )?;
                statement
                    .query_map(
                        params![
                            revision_id,
                            ordinal.saturating_sub(before as i64),
                            ordinal.saturating_add(after as i64)
                        ],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        },
                    )?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        let mut combined = String::new();
        let mut start_line = None;
        let mut end_line = None;
        for (text, start, end) in context {
            if combined.len() + text.len() + usize::from(!combined.is_empty()) * 2 > 16 * 1024 {
                break;
            }
            if !combined.is_empty() {
                combined.push_str("\n\n");
            }
            combined.push_str(&text);
            start_line = Some(start_line.map_or(start, |value: i64| value.min(start)));
            end_line = Some(end_line.map_or(end, |value: i64| value.max(end)));
        }
        let mut result = base;
        if !combined.is_empty() {
            result.text = combined;
            result.locator = format!("L{}-{}", start_line.unwrap_or(0), end_line.unwrap_or(0));
        }
        Ok(result)
    }

    async fn index_source_inner(
        &self,
        root: &Path,
        source_id: &str,
        job_id: &str,
        context: &IndexJobContext,
        cancellation: CancellationToken,
    ) -> Result<KnowledgeIndexJob, KnowledgeError> {
        let source_lock = {
            let mut locks = self.source_locks.lock().unwrap();
            locks
                .entry(source_id.to_owned())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        let _source_guard = source_lock.lock().await;
        if cancellation.is_cancelled() {
            return Err(KnowledgeError::coded("KC_CANCELLED", "index job cancelled"));
        }
        let source = self.db.with_connection(|connection| connection.query_row("SELECT collection_id,workspace_id,relative_path,content_hash FROM knowledge_sources WHERE id=?1", [source_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,Option<String>>(3)?)))).map_err(|error| KnowledgeError::coded("KC_NOT_FOUND", format!("source not found: {error}")))?;
        if source.1 != scope_key(root)? {
            return Err(KnowledgeError::coded(
                "KC_PATH_OUTSIDE_WORKSPACE",
                "source is not bound to this workspace",
            ));
        }
        let path = resolve_source_path(root, &source.2)?;
        let text = read_text(&path)?;
        let hash = hash_text(&text);
        let chunks = chunk_text(
            &text,
            self.usize_setting("knowledge.max_chunk_tokens", 1500, 128, 8192)?,
        );
        let now = now_ms();
        let embedding = self.embedding_settings()?;
        let lexical = embedding.semantic_enabled && embedding.embedding_configured;
        let (selected_revision_id, needs_embedding, had_old_active, candidate_is_old_active) = self.db.with_connection(|connection| {
            let tx = connection.transaction()?;
            let updated = tx.execute("UPDATE knowledge_index_jobs SET requested_revision_hash=?2,stage='parse',embedding_mode=?3,processed_bytes=0,total_bytes=?4,processed_chunks=0,total_chunks=?5,chunk_count=0,vector_count=0,error_code=NULL WHERE id=?1 AND state='running'", params![job_id,hash,if lexical {"semantic_pending"} else {"lexical_only"},text.len() as u64,chunks.len() as i64])?;
            if updated != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            let existing: Option<(String,String,String,Option<String>,i64,String,i64)> = tx.query_row("SELECT id,embedding_status,embedding_provider,embedding_model,embedding_dimension,embedding_encoding_format,active FROM knowledge_revisions WHERE source_id=?1 AND revision_hash=?2 AND parser_version=?3 AND chunker_version=?4", params![source_id,hash,PARSER_VERSION,CHUNKER_VERSION], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?))).optional()?;
            let old_active: Option<String> = tx.query_row("SELECT id FROM knowledge_revisions WHERE source_id=?1 AND active=1 ORDER BY created_at_ms DESC LIMIT 1", [source_id], |row| row.get(0)).optional()?;
            let revision_id = if let Some((id, ..)) = existing.as_ref() { id.clone() } else {
                let id = Uuid::new_v4().to_string();
                tx.execute("INSERT INTO knowledge_revisions(id,source_id,revision_hash,parser_version,chunker_version,embedding_provider,embedding_model,embedding_status,active,created_at_ms) VALUES(?1,?2,?3,?4,?5,'none',NULL,?6,0,?7)", params![id,source_id,hash,PARSER_VERSION,CHUNKER_VERSION,if lexical {"embedding_pending"} else {"lexical_only"},now])?;
                for (ordinal, chunk) in chunks.iter().enumerate() {
                    let chunk_id = Uuid::new_v4().to_string();
                    tx.execute("INSERT INTO knowledge_chunks(id,revision_id,ordinal,title,text,terms,token_estimate,start_line,end_line) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![chunk_id,id,ordinal as i64,chunk.title,chunk.text,chunk.terms,chunk.token_estimate as i64,chunk.start_line as i64,chunk.end_line as i64])?;
                    tx.execute("INSERT INTO knowledge_chunks_fts(chunk_id,revision_id,title,text,terms) VALUES(?1,?2,?3,?4,?5)", params![chunk_id,id,chunk.title,chunk.text,chunk.terms])?;
                }
                id
            };
            let candidate_is_old_active = old_active.as_deref() == Some(revision_id.as_str());
            let existing_ready = existing.as_ref().is_some_and(|(_, status, provider, model, dimension, encoding, _)| {
                lexical && status == "semantic_ready" && provider == EMBEDDING_PROVIDER && model.as_deref() == Some(EMBEDDING_MODEL) && *dimension > 0 && encoding == EMBEDDING_ENCODING
            });
            let vector_count: i64 = tx.query_row("SELECT COUNT(*) FROM knowledge_chunk_embeddings WHERE revision_id=?1 AND provider=?2 AND model=?3 AND encoding_format=?4", params![revision_id,EMBEDDING_PROVIDER,EMBEDDING_MODEL,EMBEDDING_ENCODING], |row| row.get(0))?;
            let needs_embedding = lexical && !(existing_ready && vector_count == chunks.len() as i64);
            let reused = if lexical {
                !needs_embedding
            } else {
                existing
                    .as_ref()
                    .is_some_and(|(_, status, ..)| status == "lexical_only")
            };
            if !lexical {
                tx.execute("UPDATE knowledge_revisions SET active=0 WHERE source_id=?1", [source_id])?;
                tx.execute("UPDATE knowledge_revisions SET active=1,embedding_provider='none',embedding_model=NULL,embedding_dimension=0,embedding_encoding_format='float',embedding_status='lexical_only' WHERE id=?1", [revision_id.as_str()])?;
                tx.execute("UPDATE knowledge_sources SET content_hash=?2,active_revision_id=?3,state='indexed',embedding_status='lexical_only',active_embedding_model=NULL,active_embedding_dimension=0,active_embedding_encoding_format='float',updated_at_ms=?4,last_error_code=NULL WHERE id=?1", params![source_id,hash,revision_id,now])?;
            } else if !needs_embedding {
                let ready_dimension = existing.as_ref().map(|value| value.4).unwrap_or(0);
                tx.execute("UPDATE knowledge_revisions SET active=0 WHERE source_id=?1", [source_id])?;
                tx.execute("UPDATE knowledge_revisions SET active=1,embedding_provider=?2,embedding_model=?3,embedding_dimension=?4,embedding_encoding_format=?5,embedding_status='semantic_ready' WHERE id=?1", params![revision_id,EMBEDDING_PROVIDER,EMBEDDING_MODEL,ready_dimension,EMBEDDING_ENCODING])?;
                tx.execute("UPDATE knowledge_sources SET content_hash=?2,active_revision_id=?3,state='indexed',embedding_status='semantic_ready',active_embedding_model=?4,active_embedding_dimension=?5,active_embedding_encoding_format=?6,updated_at_ms=?7,last_error_code=NULL WHERE id=?1", params![source_id,hash,revision_id,EMBEDDING_MODEL,ready_dimension,EMBEDDING_ENCODING,now])?;
            } else if candidate_is_old_active {
                tx.execute("UPDATE knowledge_sources SET state='indexing',updated_at_ms=?2 WHERE id=?1", params![source_id,now])?;
            } else {
                tx.execute("UPDATE knowledge_revisions SET active=0,embedding_status='embedding_pending' WHERE id=?1", [revision_id.as_str()])?;
                tx.execute("UPDATE knowledge_sources SET state='indexing',updated_at_ms=?2 WHERE id=?1", params![source_id,now])?;
            }
            if lexical && needs_embedding {
                tx.execute("UPDATE knowledge_index_jobs SET stage='embedding',state='running',processed_bytes=total_bytes WHERE id=?1", [job_id])?;
            } else {
                tx.execute(
                    "UPDATE knowledge_index_jobs SET stage='complete',state=?2,embedding_mode=?3,processed_bytes=total_bytes,processed_chunks=total_chunks,chunk_count=total_chunks,vector_count=CASE WHEN ?3='semantic_ready' THEN (SELECT COUNT(*) FROM knowledge_chunk_embeddings WHERE revision_id=?4) ELSE 0 END,completed_at_ms=?5 WHERE id=?1 AND state='running'",
                    params![job_id, if reused {"reused"} else {"completed"}, if lexical {"semantic_ready"} else {"lexical_only"}, revision_id, now_ms()],
                )?;
            }
            tx.commit()?;
            Ok((revision_id, needs_embedding, old_active.is_some(), candidate_is_old_active))
        }).map_err(|error| {
            if cancellation.is_cancelled() {
                KnowledgeError::coded("KC_CANCELLED", "index job cancelled")
            } else {
                KnowledgeError::Storage(error.to_string())
            }
        })?;
        if let Ok(job) = self.get_job(job_id) {
            self.publish_progress(&job, context);
        }
        if needs_embedding && cancellation.is_cancelled() {
            return self.finish_cancelled(
                &job_id,
                source_id,
                &selected_revision_id,
                had_old_active,
                candidate_is_old_active,
            );
        }
        if lexical && needs_embedding {
            match self
                .embed_revision(
                    &selected_revision_id,
                    source_id,
                    &job_id,
                    context,
                    cancellation.clone(),
                )
                .await
            {
                Ok(dimension) => {
                    let committed = self.db.with_connection(|connection| {
                        let tx = connection.transaction()?;
                        let running: String = tx.query_row(
                            "SELECT state FROM knowledge_index_jobs WHERE id=?1",
                            [&job_id],
                            |row| row.get(0),
                        )?;
                        if running != "running" {
                            return Ok(false);
                        }
                        tx.execute("UPDATE knowledge_revisions SET active=0 WHERE source_id=?1", [source_id])?;
                        tx.execute("UPDATE knowledge_revisions SET active=1,embedding_provider=?2,embedding_model=?3,embedding_dimension=?4,embedding_encoding_format=?5,embedding_status='semantic_ready' WHERE id=?1", params![&selected_revision_id,EMBEDDING_PROVIDER,EMBEDDING_MODEL,dimension as i64,EMBEDDING_ENCODING])?;
                        tx.execute("UPDATE knowledge_sources SET content_hash=?2,active_revision_id=?3,state='indexed',embedding_status='semantic_ready',active_embedding_model=?4,active_embedding_dimension=?5,active_embedding_encoding_format=?6,last_error_code=NULL,updated_at_ms=?7 WHERE id=?1", params![source_id,hash,&selected_revision_id,EMBEDDING_MODEL,dimension as i64,EMBEDDING_ENCODING,now_ms()])?;
                        tx.execute("INSERT INTO settings(key,value) VALUES('embedding.vector_dimension',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [dimension.to_string()])?;
                        tx.execute("UPDATE knowledge_index_jobs SET stage='complete',state='completed',embedding_mode='semantic_ready',chunk_count=total_chunks,vector_count=(SELECT COUNT(*) FROM knowledge_chunk_embeddings WHERE revision_id=?2),completed_at_ms=?3 WHERE id=?1", params![job_id,&selected_revision_id,now_ms()])?;
                        tx.commit()?;
                        Ok(true)
                    }).map_err(|error| KnowledgeError::Storage(error.to_string()))?;
                    if !committed {
                        return self.finish_cancelled(
                            &job_id,
                            source_id,
                            &selected_revision_id,
                            had_old_active,
                            candidate_is_old_active,
                        );
                    }
                }
                Err(error) if error.code() == "KC_CANCELLED" => {
                    return self.finish_cancelled(
                        &job_id,
                        source_id,
                        &selected_revision_id,
                        had_old_active,
                        candidate_is_old_active,
                    );
                }
                Err(error) => {
                    let handled = self.db.with_connection(|connection| {
                        let tx = connection.transaction()?;
                        let running: String = tx.query_row(
                            "SELECT state FROM knowledge_index_jobs WHERE id=?1",
                            [job_id],
                            |row| row.get(0),
                        )?;
                        if running != "running" {
                            return Ok(false);
                        }
                        if candidate_is_old_active {
                            let active_status: String = tx.query_row("SELECT embedding_status FROM knowledge_revisions WHERE id=?1", [&selected_revision_id], |row| row.get(0))?;
                            if active_status == "semantic_ready" {
                                tx.execute("UPDATE knowledge_sources SET state='indexed',embedding_status='semantic_ready',last_error_code=?2,updated_at_ms=?3 WHERE id=?1", params![source_id,error.code(),now_ms()])?;
                                tx.execute("UPDATE knowledge_index_jobs SET stage='complete',state='completed',embedding_mode='semantic_ready',error_code=?2,chunk_count=total_chunks,vector_count=(SELECT COUNT(*) FROM knowledge_chunk_embeddings WHERE revision_id=?3),completed_at_ms=?4 WHERE id=?1", params![job_id,error.code(),&selected_revision_id,now_ms()])?;
                            } else {
                                tx.execute("DELETE FROM knowledge_chunk_embeddings WHERE revision_id=?1", [&selected_revision_id])?;
                                tx.execute("UPDATE knowledge_revisions SET embedding_provider='none',embedding_model=NULL,embedding_dimension=0,embedding_encoding_format='float',embedding_status='lexical_only' WHERE id=?1", [&selected_revision_id])?;
                                tx.execute("UPDATE knowledge_sources SET state='indexed',embedding_status='lexical_only',active_embedding_model=NULL,active_embedding_dimension=0,active_embedding_encoding_format='float',last_error_code=?2,updated_at_ms=?3 WHERE id=?1", params![source_id,error.code(),now_ms()])?;
                                tx.execute("UPDATE knowledge_index_jobs SET stage='complete',state='completed',embedding_mode='lexical_only',error_code=?2,chunk_count=total_chunks,vector_count=0,completed_at_ms=?3 WHERE id=?1", params![job_id,error.code(),now_ms()])?;
                            }
                        } else if had_old_active {
                            tx.execute("DELETE FROM knowledge_chunk_embeddings WHERE revision_id=?1", [&selected_revision_id])?;
                            tx.execute("UPDATE knowledge_revisions SET embedding_status='embedding_failed' WHERE id=?1", [&selected_revision_id])?;
                            tx.execute("UPDATE knowledge_sources SET state='indexed',embedding_status=COALESCE((SELECT embedding_status FROM knowledge_revisions WHERE id=knowledge_sources.active_revision_id),'lexical_only'),last_error_code=?2,updated_at_ms=?3 WHERE id=?1", params![source_id,error.code(),now_ms()])?;
                            tx.execute("UPDATE knowledge_index_jobs SET stage='complete',state='completed',embedding_mode='lexical_only',error_code=?2,chunk_count=total_chunks,vector_count=0,completed_at_ms=?3 WHERE id=?1", params![job_id,error.code(),now_ms()])?;
                        } else {
                            tx.execute("UPDATE knowledge_revisions SET active=1,embedding_provider='none',embedding_model=NULL,embedding_dimension=0,embedding_encoding_format='float',embedding_status='lexical_only' WHERE id=?1", [&selected_revision_id])?;
                            tx.execute("UPDATE knowledge_sources SET content_hash=?2,active_revision_id=?3,state='indexed',embedding_status='lexical_only',active_embedding_model=NULL,active_embedding_dimension=0,active_embedding_encoding_format='float',last_error_code=?4,updated_at_ms=?5 WHERE id=?1", params![source_id,hash,&selected_revision_id,error.code(),now_ms()])?;
                            tx.execute("UPDATE knowledge_index_jobs SET stage='complete',state='completed',embedding_mode='lexical_only',error_code=?2,chunk_count=total_chunks,vector_count=0,completed_at_ms=?3 WHERE id=?1", params![job_id,error.code(),now_ms()])?;
                        }
                        tx.commit()?;
                        Ok(true)
                    }).map_err(|db_error| KnowledgeError::Storage(db_error.to_string()))?;
                    if !handled {
                        return self.finish_cancelled(
                            job_id,
                            source_id,
                            &selected_revision_id,
                            had_old_active,
                            candidate_is_old_active,
                        );
                    }
                }
            }
        }
        self.get_job(&job_id)
    }

    fn finish_cancelled(
        &self,
        job_id: &str,
        source_id: &str,
        revision_id: &str,
        had_old_active: bool,
        candidate_is_old_active: bool,
    ) -> Result<KnowledgeIndexJob, KnowledgeError> {
        self.db
            .with_connection(|connection| {
                let tx = connection.transaction()?;
                let job_state: String = tx.query_row(
                    "SELECT state FROM knowledge_index_jobs WHERE id=?1",
                    [job_id],
                    |row| row.get(0),
                )?;
                if matches!(job_state.as_str(), "completed" | "reused" | "failed") {
                    tx.commit()?;
                    return Ok(());
                }
                if candidate_is_old_active {
                    let active_status: String = tx.query_row(
                        "SELECT embedding_status FROM knowledge_revisions WHERE id=?1",
                        [revision_id],
                        |row| row.get(0),
                    )?;
                    if active_status == "semantic_ready" {
                        tx.execute(
                            "UPDATE knowledge_sources SET state='indexed',embedding_status='semantic_ready',last_error_code='KC_CANCELLED',updated_at_ms=?2 WHERE id=?1",
                            params![source_id, now_ms()],
                        )?;
                    } else {
                        tx.execute(
                            "DELETE FROM knowledge_chunk_embeddings WHERE revision_id=?1",
                            [revision_id],
                        )?;
                        tx.execute(
                            "UPDATE knowledge_revisions SET embedding_provider='none',embedding_model=NULL,embedding_dimension=0,embedding_encoding_format='float',embedding_status='lexical_only' WHERE id=?1",
                            [revision_id],
                        )?;
                        tx.execute(
                            "UPDATE knowledge_sources SET state='indexed',embedding_status='lexical_only',active_embedding_model=NULL,active_embedding_dimension=0,active_embedding_encoding_format='float',last_error_code='KC_CANCELLED',updated_at_ms=?2 WHERE id=?1",
                            params![source_id, now_ms()],
                        )?;
                    }
                } else if had_old_active {
                    tx.execute(
                        "DELETE FROM knowledge_chunk_embeddings WHERE revision_id=?1",
                        [revision_id],
                    )?;
                    tx.execute(
                        "UPDATE knowledge_revisions SET active=0,embedding_status='embedding_pending' WHERE id=?1",
                        [revision_id],
                    )?;
                    tx.execute(
                        "UPDATE knowledge_sources SET state='indexed',last_error_code='KC_CANCELLED',updated_at_ms=?2 WHERE id=?1",
                        params![source_id, now_ms()],
                    )?;
                } else {
                    tx.execute(
                        "UPDATE knowledge_revisions SET active=0,embedding_status='embedding_pending' WHERE id=?1",
                        [revision_id],
                    )?;
                    tx.execute(
                    "UPDATE knowledge_sources SET state='cancelled',active_revision_id=NULL,embedding_status='lexical_only',active_embedding_model=NULL,active_embedding_dimension=0,active_embedding_encoding_format='float',last_error_code='KC_CANCELLED',updated_at_ms=?2 WHERE id=?1",
                        params![source_id, now_ms()],
                    )?;
                }
                tx.execute(
                    "UPDATE knowledge_index_jobs SET stage='cancelled',state='cancelled',embedding_mode=CASE WHEN (SELECT embedding_status FROM knowledge_sources WHERE id=?3)='semantic_ready' THEN 'semantic_ready' WHEN ?2 THEN 'lexical_only' ELSE embedding_mode END,vector_count=CASE WHEN (SELECT embedding_status FROM knowledge_sources WHERE id=?3)='semantic_ready' THEN (SELECT COUNT(*) FROM knowledge_chunk_embeddings WHERE revision_id=(SELECT active_revision_id FROM knowledge_sources WHERE id=?3)) ELSE 0 END,error_code='KC_CANCELLED',completed_at_ms=?4 WHERE id=?1 AND state IN ('queued','running','cancelled')",
                    params![job_id, candidate_is_old_active || !had_old_active, source_id, now_ms()],
                )?;
                tx.commit()?;
                Ok(())
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        self.get_job(job_id)
    }

    async fn embed_revision(
        &self,
        revision_id: &str,
        source_id: &str,
        job_id: &str,
        context: &IndexJobContext,
        cancellation: CancellationToken,
    ) -> Result<usize, KnowledgeError> {
        let key = self
            .credentials
            .get_api_key(EMBEDDING_CREDENTIAL)
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?
            .ok_or_else(|| {
                KnowledgeError::coded(
                    "KC_EMBEDDING_NOT_CONFIGURED",
                    "embedding credential is not configured",
                )
            })?;
        let batch_size = self.usize_setting("embedding.batch_size", 16, 1, 32)?;
        let timeout_ms = self.u64_setting("embedding.timeout_ms", 30_000, 1_000, 120_000)?;
        let chunks = self
            .db
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT id,text FROM knowledge_chunks WHERE revision_id=?1 ORDER BY ordinal",
                )?;
                statement
                    .query_map([revision_id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .map_err(|_| {
                KnowledgeError::coded("KC_EMBEDDING_UNAVAILABLE", "embedding client unavailable")
            })?;
        let mut dimension = None;
        let mut staged_vectors = Vec::with_capacity(chunks.len());
        let mut last_progress = Instant::now();
        for batch in chunks.chunks(batch_size) {
            if cancellation.is_cancelled() {
                return Err(KnowledgeError::coded("KC_CANCELLED", "index job cancelled"));
            }
            let batch_started = Instant::now();
            let (response, retry_count) = send_embedding_request(
                &client,
                &self.embedding_endpoint,
                &key,
                json!({
                    "model": EMBEDDING_MODEL,
                    "input": batch.iter().map(|(_, text)| text).collect::<Vec<_>>(),
                    "encoding_format": EMBEDDING_ENCODING
                }),
                &cancellation,
            )
            .await?;
            let status = response.status().as_u16();
            self.db
                .with_connection(|connection| {
                    connection.execute(
                        "UPDATE knowledge_index_jobs SET embedding_requests=embedding_requests+1,retry_count=retry_count+?2,last_http_status=?3 WHERE id=?1",
                        params![job_id, retry_count as i64, status as i64],
                    )?;
                    Ok(())
                })
                .map_err(|error| KnowledgeError::Storage(error.to_string()))?;
            self.record_metrics(|metrics| {
                metrics.embedding_requests = metrics.embedding_requests.saturating_add(1);
                metrics.embedding_retries = metrics.embedding_retries.saturating_add(retry_count);
            });
            self.log_event(
                if status == 200 { "info" } else { "warn" },
                "knowledge_embedding_batch",
                json!({
                    "jobId": job_id,
                    "batchSize": batch.len(),
                    "httpStatus": status,
                    "retries": retry_count,
                    "durationMs": batch_started.elapsed().as_millis() as u64,
                }),
            );
            if status != 200 {
                return Err(KnowledgeError::coded(
                    match status {
                        401 | 403 => "KC_EMBEDDING_AUTH_FAILED",
                        404 => "KC_EMBEDDING_MODEL_NOT_FOUND",
                        429 => "KC_EMBEDDING_RATE_LIMITED",
                        503 | 504 => "KC_EMBEDDING_UNAVAILABLE",
                        _ => "KC_EMBEDDING_RESPONSE_INVALID",
                    },
                    format!("embedding request returned HTTP {status}"),
                ));
            }
            let payload: serde_json::Value = tokio::select! {
                _ = cancellation.cancelled() => {
                    return Err(KnowledgeError::coded("KC_CANCELLED", "index job cancelled"));
                }
                result = response.json() => result.map_err(|_| {
                    KnowledgeError::coded(
                        "KC_EMBEDDING_RESPONSE_INVALID",
                        "embedding response is not valid JSON",
                    )
                })?,
            };
            let vectors = validate_embedding_payload(&payload, batch.len())?;
            for (position, values) in vectors.into_iter().enumerate() {
                if let Some(expected) = dimension {
                    if expected != values.len() {
                        return Err(KnowledgeError::coded(
                            "KC_EMBEDDING_DIMENSION_MISMATCH",
                            "embedding vector dimensions differ across batches",
                        ));
                    }
                } else {
                    dimension = Some(values.len());
                }
                let bytes = values
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect::<Vec<_>>();
                let vector_hash = hash_bytes(&bytes);
                if cancellation.is_cancelled() {
                    return Err(KnowledgeError::coded("KC_CANCELLED", "index job cancelled"));
                }
                staged_vectors.push((batch[position].0.clone(), values.len(), bytes, vector_hash));
            }
            if cancellation.is_cancelled() {
                return Err(KnowledgeError::coded("KC_CANCELLED", "index job cancelled"));
            }
            self.db.with_connection(|connection| { connection.execute("UPDATE knowledge_index_jobs SET processed_chunks=MIN(total_chunks,processed_chunks+?2),vector_count=MIN(total_chunks,vector_count+?2) WHERE id=?1", params![job_id,batch.len() as i64])?; Ok(()) }).map_err(|error| KnowledgeError::Storage(error.to_string()))?;
            if last_progress.elapsed() >= PROGRESS_THROTTLE {
                if let Ok(job) = self.get_job(job_id) {
                    self.publish_progress(&job, context);
                }
                last_progress = Instant::now();
            }
        }
        if cancellation.is_cancelled() {
            return Err(KnowledgeError::coded("KC_CANCELLED", "index job cancelled"));
        }
        self.db
            .with_connection(|connection| {
                let tx = connection.transaction()?;
                for (chunk_id, vector_dimension, bytes, vector_hash) in &staged_vectors {
                    tx.execute(
                        "INSERT OR REPLACE INTO knowledge_chunk_embeddings(chunk_id,revision_id,provider,model,dimension,encoding_format,vector,vector_hash,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                        params![chunk_id, revision_id, EMBEDDING_PROVIDER, EMBEDDING_MODEL, *vector_dimension as i64, EMBEDDING_ENCODING, bytes, vector_hash, now_ms()],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?;
        if let Ok(job) = self.get_job(job_id) {
            self.publish_progress(&job, context);
        }
        let _ = source_id;
        dimension.ok_or_else(|| {
            KnowledgeError::coded(
                "KC_EMBEDDING_RESPONSE_INVALID",
                "embedding response contained no vectors",
            )
        })
    }

    fn source_view(&self, source_id: &str) -> Result<KnowledgeSource, KnowledgeError> {
        self.db.with_connection(|connection| connection.query_row("SELECT s.id,s.relative_path,s.size_bytes,s.content_hash,s.active_revision_id,s.active_embedding_model,s.active_embedding_dimension,s.active_embedding_encoding_format,s.embedding_status,s.state,s.last_error_code,(SELECT COUNT(*) FROM knowledge_chunks k WHERE k.revision_id=s.active_revision_id),(SELECT MAX(completed_at_ms) FROM knowledge_index_jobs j WHERE j.source_id=s.id AND j.state IN ('completed','reused')),(SELECT id FROM knowledge_index_jobs j WHERE j.source_id=s.id AND j.state IN ('queued','running') ORDER BY created_at_ms LIMIT 1) FROM knowledge_sources s WHERE s.id=?1", [source_id], |row| Ok(KnowledgeSource { source_id: row.get(0)?, relative_path: row.get(1)?, size_bytes: row.get::<_,i64>(2)? as u64, content_hash_prefix: row.get::<_,Option<String>>(3)?.map(|value| value.chars().take(12).collect()), active_revision_id: row.get(4)?, active_embedding_model: row.get(5)?, active_embedding_dimension: row.get::<_,i64>(6)? as usize, active_embedding_encoding_format: row.get(7)?, embedding_status: row.get(8)?, state: row.get(9)?, last_error_code: row.get(10)?, chunk_count: row.get::<_,i64>(11)? as usize, last_indexed_at_ms: row.get(12)?, initial_job_id: row.get(13)? }))).map_err(|error| if matches!(error, crate::persistence::ProjectionError::Database(rusqlite::Error::QueryReturnedNoRows)) { KnowledgeError::coded("KC_NOT_FOUND", "source not found") } else { KnowledgeError::Storage(error.to_string()) })
    }

    fn bool_setting(&self, key: &str, default: bool) -> Result<bool, KnowledgeError> {
        Ok(self
            .db
            .setting(key)
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?
            .and_then(|value| value.parse().ok())
            .unwrap_or(default))
    }
    fn usize_setting(
        &self,
        key: &str,
        default: usize,
        min: usize,
        max: usize,
    ) -> Result<usize, KnowledgeError> {
        Ok(self
            .db
            .setting(key)
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
            .clamp(min, max))
    }
    fn u64_setting(
        &self,
        key: &str,
        default: u64,
        min: u64,
        max: u64,
    ) -> Result<u64, KnowledgeError> {
        Ok(self
            .db
            .setting(key)
            .map_err(|error| KnowledgeError::Storage(error.to_string()))?
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
            .clamp(min, max))
    }
    fn set_bool_setting(&self, key: &str, value: bool) -> Result<(), KnowledgeError> {
        self.db
            .set_setting(key, if value { "true" } else { "false" })
            .map_err(|error| KnowledgeError::Storage(error.to_string()))
    }
    fn set_usize_setting(&self, key: &str, value: usize) -> Result<(), KnowledgeError> {
        self.db
            .set_setting(key, &value.to_string())
            .map_err(|error| KnowledgeError::Storage(error.to_string()))
    }
    fn set_u64_setting(&self, key: &str, value: u64) -> Result<(), KnowledgeError> {
        self.db
            .set_setting(key, &value.to_string())
            .map_err(|error| KnowledgeError::Storage(error.to_string()))
    }
}

pub struct KnowledgeSearchTool {
    service: KnowledgeService,
}
pub struct KnowledgeCitationTool {
    service: KnowledgeService,
}

impl KnowledgeSearchTool {
    pub fn new(service: KnowledgeService) -> Self {
        Self { service }
    }
}
impl KnowledgeCitationTool {
    pub fn new(service: KnowledgeService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl ToolHandler for KnowledgeSearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition { name: "search_knowledge".into(), description: "Search user-selected workspace knowledge sources. Results are bounded and include citation ids; do not use this tool as arbitrary file access.".into(), input_schema: json!({"type":"object","properties":{"query":{"type":"string","minLength":1,"maxLength":2000},"limit":{"type":"integer","minimum":1,"maximum":6}},"required":["query"],"additionalProperties":false}) }
    }
    async fn execute(
        &self,
        context: &ToolContext,
        arguments: serde_json::Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let query = arguments
            .get("query")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("query must be a string".into()))?;
        let limit = arguments
            .get("limit")
            .and_then(|value| value.as_u64())
            .unwrap_or(MAX_RESULTS as u64) as usize;
        let response = self
            .service
            .search(
                &context.workspace_root,
                &context.thread_id,
                &context.turn_id,
                query,
                limit,
            )
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&response)
                .map_err(|error| ToolError::Execution(error.to_string()))?,
            metadata: json!({"retrievalMode":response.metadata["retrievalMode"],"resultCount":response.results.len()}),
        })
    }
}

#[async_trait]
impl ToolHandler for KnowledgeCitationTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition { name: "read_knowledge_citation".into(), description: "Read a citation previously issued by search_knowledge in the current turn. Citation ids cannot be used to read arbitrary files.".into(), input_schema: json!({"type":"object","properties":{"citationId":{"type":"string","minLength":1},"before":{"type":"integer","minimum":0,"maximum":1},"after":{"type":"integer","minimum":0,"maximum":1}},"required":["citationId"],"additionalProperties":false}) }
    }
    async fn execute(
        &self,
        context: &ToolContext,
        arguments: serde_json::Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let citation_id = arguments
            .get("citationId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("citationId must be a string".into()))?;
        let before = arguments
            .get("before")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize;
        let after = arguments
            .get("after")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize;
        let citation = self
            .service
            .read_citation(
                &context.thread_id,
                &context.turn_id,
                citation_id,
                before,
                after,
            )
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&citation)
                .map_err(|error| ToolError::Execution(error.to_string()))?,
            metadata: json!({"citationId":citation_id,"revision":citation.revision}),
        })
    }
}

#[derive(Debug)]
struct Chunk {
    title: String,
    text: String,
    terms: String,
    token_estimate: usize,
    start_line: usize,
    end_line: usize,
}

fn chunk_text(text: &str, max_tokens: usize) -> Vec<Chunk> {
    let max_chars = max_tokens.saturating_mul(4).clamp(512, MAX_CHUNK_BYTES);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut start_line = 1usize;
    let mut line = 1usize;
    for source_line in text.lines() {
        if !current.is_empty()
            && (current.len() + source_line.len() + 1 > max_chars || source_line.trim().is_empty())
        {
            let end_line = line.saturating_sub(1);
            chunks.push(make_chunk(&current, start_line, end_line));
            current.clear();
            start_line = line;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(source_line);
        line += 1;
    }
    if !current.trim().is_empty() {
        chunks.push(make_chunk(&current, start_line, line.saturating_sub(1)));
    }
    if chunks.is_empty() {
        chunks.push(make_chunk(text, 1, 1));
    }
    chunks
}

fn make_chunk(text: &str, start_line: usize, end_line: usize) -> Chunk {
    Chunk {
        title: text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Untitled")
            .trim()
            .chars()
            .take(120)
            .collect(),
        text: text.to_string(),
        terms: terms(text),
        token_estimate: text.chars().count().div_ceil(4),
        start_line,
        end_line,
    }
}

fn terms(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut values = Vec::new();
    values.extend(
        text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-' && ch != '.')
            .filter(|value| value.len() >= 2)
            .map(str::to_lowercase),
    );
    for window in chars.windows(2) {
        if window.iter().all(|ch| !ch.is_ascii_whitespace()) {
            values.push(window.iter().collect());
        }
    }
    values.join(" ")
}

fn fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn validate_embedding_payload(
    payload: &serde_json::Value,
    expected_count: usize,
) -> Result<Vec<Vec<f32>>, KnowledgeError> {
    if payload.get("object").and_then(|value| value.as_str()) != Some("list") {
        return Err(KnowledgeError::coded(
            "KC_EMBEDDING_RESPONSE_INVALID",
            "embedding response object is invalid",
        ));
    }
    if payload.get("model").and_then(|value| value.as_str()) != Some(EMBEDDING_MODEL) {
        return Err(KnowledgeError::coded(
            "KC_EMBEDDING_MODEL_NOT_FOUND",
            "embedding response model does not match fixed profile",
        ));
    }
    let data = payload
        .get("data")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            KnowledgeError::coded(
                "KC_EMBEDDING_RESPONSE_INVALID",
                "embedding response data is invalid",
            )
        })?;
    if data.len() != expected_count {
        return Err(KnowledgeError::coded(
            "KC_EMBEDDING_RESPONSE_INVALID",
            "embedding response item count is invalid",
        ));
    }
    let mut dimension = None;
    data.iter()
        .enumerate()
        .map(|(position, item)| {
            if item.get("object").and_then(|value| value.as_str()) != Some("embedding")
                || item.get("index").and_then(|value| value.as_u64()) != Some(position as u64)
            {
                return Err(KnowledgeError::coded(
                    "KC_EMBEDDING_RESPONSE_INVALID",
                    "embedding response indexes are not contiguous",
                ));
            }
            let vector = item
                .get("embedding")
                .and_then(|value| value.as_array())
                .ok_or_else(|| {
                    KnowledgeError::coded(
                        "KC_EMBEDDING_RESPONSE_INVALID",
                        "embedding vector is invalid",
                    )
                })?;
            if vector.is_empty()
                || vector.len() > 4096
                || vector
                    .iter()
                    .any(|value| !value.as_f64().is_some_and(f64::is_finite))
            {
                return Err(KnowledgeError::coded(
                    "KC_EMBEDDING_RESPONSE_INVALID",
                    "embedding vector failed bounded finite validation",
                ));
            }
            if dimension.is_some_and(|expected| expected != vector.len()) {
                return Err(KnowledgeError::coded(
                    "KC_EMBEDDING_DIMENSION_MISMATCH",
                    "embedding vector dimensions differ within a batch",
                ));
            }
            dimension.get_or_insert(vector.len());
            let mut values = vector
                .iter()
                .map(|value| value.as_f64().unwrap() as f32)
                .collect::<Vec<_>>();
            normalize_vector(&mut values)?;
            Ok(values)
        })
        .collect()
}

fn normalize_vector(values: &mut [f32]) -> Result<(), KnowledgeError> {
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        return Err(KnowledgeError::coded(
            "KC_EMBEDDING_RESPONSE_INVALID",
            "embedding vector contains non-finite values",
        ));
    }
    let norm = values
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        return Err(KnowledgeError::coded(
            "KC_EMBEDDING_RESPONSE_INVALID",
            "embedding vector has zero magnitude",
        ));
    }
    for value in values {
        *value = (f64::from(*value) / norm) as f32;
    }
    Ok(())
}

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() {
        return f64::NAN;
    }
    left.iter()
        .zip(right)
        .map(|(left, right)| f64::from(*left) * f64::from(*right))
        .sum()
}

fn rrf_score(lexical_rank: usize, semantic_rank: Option<usize>) -> f64 {
    const RRF_K: f64 = 60.0;
    let lexical = if lexical_rank == 0 {
        0.0
    } else {
        1.0 / (RRF_K + lexical_rank as f64)
    };
    let semantic = semantic_rank
        .map(|rank| 1.0 / (RRF_K + rank as f64))
        .unwrap_or(0.0);
    lexical + semantic
}

fn preview(text: &str) -> String {
    text.chars().take(600).collect()
}
fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn query_embedding_cache_key(query: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(EMBEDDING_PROVIDER.as_bytes());
    hasher.update([0]);
    hasher.update(EMBEDDING_MODEL.as_bytes());
    hasher.update([0]);
    hasher.update(EMBEDDING_ENCODING.as_bytes());
    hasher.update([0]);
    hasher.update(query.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn modified_ms(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}

fn sanitize_network_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "embedding request timed out".into()
    } else {
        "embedding request failed".into()
    }
}

async fn send_embedding_request(
    client: &reqwest::Client,
    endpoint: &str,
    key: &str,
    payload: serde_json::Value,
    cancellation: &CancellationToken,
) -> Result<(reqwest::Response, usize), KnowledgeError> {
    let mut retry_count = 0usize;
    loop {
        if cancellation.is_cancelled() {
            return Err(KnowledgeError::coded("KC_CANCELLED", "index job cancelled"));
        }
        let request = client.post(endpoint).bearer_auth(key).json(&payload).send();
        let result = tokio::select! {
            _ = cancellation.cancelled() => {
                return Err(KnowledgeError::coded("KC_CANCELLED", "index job cancelled"));
            }
            result = request => result,
        };
        match result {
            Ok(response)
                if matches!(response.status().as_u16(), 429 | 503 | 504) && retry_count < 2 =>
            {
                retry_count += 1;
                let delay_ms = 250u64.saturating_mul(1u64 << (retry_count - 1));
                tokio::select! {
                    _ = cancellation.cancelled() => {
                        return Err(KnowledgeError::coded("KC_CANCELLED", "index job cancelled"));
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                }
            }
            Ok(response) => return Ok((response, retry_count)),
            Err(error) if (error.is_timeout() || error.is_connect()) && retry_count < 2 => {
                retry_count += 1;
                let delay_ms = 250u64.saturating_mul(1u64 << (retry_count - 1));
                tokio::select! {
                    _ = cancellation.cancelled() => {
                        return Err(KnowledgeError::coded("KC_CANCELLED", "index job cancelled"));
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                }
            }
            Err(error) => {
                return Err(KnowledgeError::coded(
                    "KC_EMBEDDING_UNAVAILABLE",
                    sanitize_network_error(&error),
                ));
            }
        }
    }
}

fn scope_key(root: &Path) -> Result<String, KnowledgeError> {
    Ok(canonical_workspace(root)?.to_string_lossy().to_string())
}
fn canonical_workspace(root: &Path) -> Result<PathBuf, KnowledgeError> {
    let root = root
        .canonicalize()
        .map_err(|error| KnowledgeError::coded("KC_WORKSPACE_REQUIRED", error.to_string()))?;
    if !root.is_dir() {
        return Err(KnowledgeError::coded(
            "KC_WORKSPACE_REQUIRED",
            "workspace root is not a directory",
        ));
    }
    Ok(root)
}
fn resolve_source_path(root: &Path, relative: &str) -> Result<PathBuf, KnowledgeError> {
    let path = Path::new(relative);
    if relative.trim().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
        || relative.contains('*')
        || relative.contains('?')
    {
        return Err(KnowledgeError::coded(
            "KC_PATH_OUTSIDE_WORKSPACE",
            "source path must be an exact workspace-relative path",
        ));
    }
    let candidate = root.join(path);
    let mut current = root.to_path_buf();
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        current.push(name);
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            KnowledgeError::coded("KC_PATH_OUTSIDE_WORKSPACE", error.to_string())
        })?;
        if metadata.file_type().is_symlink() {
            return Err(KnowledgeError::coded(
                "KC_PATH_OUTSIDE_WORKSPACE",
                "symbolic links and junctions are not supported",
            ));
        }
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|error| KnowledgeError::coded("KC_PATH_OUTSIDE_WORKSPACE", error.to_string()))?;
    if !canonical.starts_with(root) {
        return Err(KnowledgeError::coded(
            "KC_PATH_OUTSIDE_WORKSPACE",
            "source path resolves outside workspace",
        ));
    }
    if !canonical.is_file() {
        return Err(KnowledgeError::coded(
            "KC_UNSUPPORTED_FILE",
            "source must be a regular supported text file",
        ));
    }
    let extension = canonical
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "txt"
            | "md"
            | "markdown"
            | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "json"
            | "yaml"
            | "yml"
            | "toml"
            | "py"
            | "go"
            | "java"
            | "c"
            | "cpp"
            | "h"
            | "hpp"
            | "cs"
            | "sql"
            | "sh"
            | "ps1"
            | "html"
            | "css"
            | "xml"
    ) {
        return Err(KnowledgeError::coded(
            "KC_UNSUPPORTED_FILE",
            "source file type is not supported",
        ));
    }
    Ok(canonical)
}

fn read_text(path: &Path) -> Result<String, KnowledgeError> {
    let bytes = std::fs::read(path)
        .map_err(|error| KnowledgeError::coded("KC_INDEX_FAILED", error.to_string()))?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(KnowledgeError::coded(
            "KC_SOURCE_TOO_LARGE",
            "source exceeds bounded size",
        ));
    }
    if bytes.iter().any(|byte| *byte == 0) {
        return Err(KnowledgeError::coded(
            "KC_UNSUPPORTED_FILE",
            "binary files are not supported",
        ));
    }
    if bytes.starts_with(&[0xff, 0xfe]) && (bytes.len() - 2).is_multiple_of(2) {
        return String::from_utf16(
            &bytes[2..]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| KnowledgeError::coded("KC_INDEX_FAILED", "invalid UTF-16 text"));
    }
    std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
        .map(str::to_string)
        .map_err(|_| KnowledgeError::coded("KC_INDEX_FAILED", "source is not valid UTF-8"))
}

fn map_job(row: &rusqlite::Row<'_>) -> Result<KnowledgeIndexJob, rusqlite::Error> {
    Ok(KnowledgeIndexJob {
        job_id: row.get(0)?,
        source_id: row.get(1)?,
        state: row.get(2)?,
        stage: row.get(3)?,
        embedding_mode: row.get(4)?,
        processed_bytes: row.get::<_, i64>(5)? as u64,
        total_bytes: row.get::<_, i64>(6)? as u64,
        processed_chunks: row.get::<_, i64>(7)? as usize,
        total_chunks: row.get::<_, i64>(8)? as usize,
        embedding_requests: row.get::<_, i64>(9)? as usize,
        retry_count: row.get::<_, i64>(10)? as usize,
        last_http_status: row.get::<_, Option<i64>>(11)?.map(|value| value as u16),
        chunk_count: row.get::<_, i64>(12)? as usize,
        vector_count: row.get::<_, i64>(13)? as usize,
        error_code: row.get(14)?,
        created_at_ms: row.get(15)?,
        completed_at_ms: row.get(16)?,
    })
}

fn query_strings(
    transaction: &rusqlite::Transaction<'_>,
    sql: &str,
    value: &str,
) -> Result<Vec<String>, rusqlite::Error> {
    let mut statement = transaction.prepare(sql)?;
    statement
        .query_map([value], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()
}

pub fn knowledge_tool_risks() -> HashMap<String, ToolRisk> {
    HashMap::from([
        ("search_knowledge".into(), ToolRisk::Read),
        ("read_knowledge_citation".into(), ToolRisk::Read),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::ProjectionDb;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[derive(Default)]
    struct FakeCredentialStore(std::sync::Mutex<Option<String>>);
    impl CredentialStore for FakeCredentialStore {
        fn get_api_key(
            &self,
            _provider_id: &str,
        ) -> Result<Option<String>, crate::providers::CredentialError> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn set_api_key(
            &self,
            _provider_id: &str,
            api_key: &str,
        ) -> Result<(), crate::providers::CredentialError> {
            *self.0.lock().unwrap() = Some(api_key.into());
            Ok(())
        }
        fn delete_api_key(
            &self,
            _provider_id: &str,
        ) -> Result<(), crate::providers::CredentialError> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    async fn wait_for_job(service: &KnowledgeService, job_id: &str) -> KnowledgeIndexJob {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let job = service.get_job(job_id).unwrap();
            if matches!(
                job.state.as_str(),
                "completed" | "reused" | "failed" | "cancelled"
            ) {
                return job;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "knowledge job {job_id} did not finish"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[derive(Default)]
    struct RecordingProgressSink(Mutex<Vec<KnowledgeIndexProgress>>);

    impl KnowledgeProgressSink for RecordingProgressSink {
        fn publish(&self, progress: KnowledgeIndexProgress) {
            self.0.lock().unwrap().push(progress);
        }
    }

    async fn spawn_embedding_server(
        responses: Vec<(u16, String)>,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/embeddings", listener.local_addr().unwrap());
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed_attempts = attempts.clone();
        let task = tokio::spawn(async move {
            for (status, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    let read = socket.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let Some(header_end) =
                        request.windows(4).position(|value| value == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                assert!(
                    request.contains("authorization: Bearer test-key")
                        || request.contains("Authorization: Bearer test-key")
                );
                assert!(request.contains(EMBEDDING_MODEL));
                observed_attempts.fetch_add(1, Ordering::SeqCst);
                let reason = match status {
                    200 => "OK",
                    429 => "Too Many Requests",
                    503 => "Service Unavailable",
                    504 => "Gateway Timeout",
                    _ => "Error",
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        (endpoint, attempts, task)
    }

    #[tokio::test]
    async fn indexes_and_retrieves_only_selected_workspace_text() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("guide.md"),
            "部署需要先运行 cargo test\n\n第二段",
        )
        .unwrap();
        let db = ProjectionDb::memory().unwrap();
        let service = KnowledgeService::new(db, Arc::new(FakeCredentialStore::default()));
        service.set_enabled(true).unwrap();
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Docs".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id.clone(),
                    workspace_relative_path: "guide.md".into(),
                },
            )
            .await
            .unwrap();
        wait_for_job(&service, source.initial_job_id.as_deref().unwrap()).await;
        let response = service
            .search(root.path(), "thread", "turn", "cargo test", 6)
            .await
            .unwrap();
        assert_eq!(response.results.len(), 1);
        let citation = service
            .read_citation("thread", "turn", &response.results[0].citation_id, 0, 0)
            .unwrap();
        assert!(citation.text.contains("cargo test"));
    }

    #[test]
    fn normalizes_vectors_and_rrf_keeps_both_rank_signals() {
        let mut vector = vec![3.0_f32, 4.0];
        normalize_vector(&mut vector).unwrap();
        assert!((f64::from(vector[0]) - 0.6).abs() < 1e-6);
        assert!((f64::from(vector[1]) - 0.8).abs() < 1e-6);
        assert!((cosine(&vector, &vector) - 1.0).abs() < 1e-6);
        assert!(cosine(&vector, &[1.0]).is_nan());
        assert!(rrf_score(1, Some(1)) > rrf_score(1, None));
        assert!(rrf_score(0, Some(1)) > rrf_score(0, None));
        assert!(rrf_score(1, Some(1)) > rrf_score(1, None));
        assert_eq!(rrf_score(0, None), 0.0);
    }

    #[test]
    fn query_embedding_cache_has_deterministic_lru_ttl_and_clear_behavior() {
        let now = Instant::now();
        let mut cache = QueryEmbeddingCache::default();
        for index in 0..QUERY_EMBEDDING_CACHE_CAPACITY {
            cache.insert(format!("q-{index}"), vec![index as f32], now);
        }
        assert_eq!(cache.entries.len(), QUERY_EMBEDDING_CACHE_CAPACITY);
        assert_eq!(cache.get("q-0", now).as_deref(), Some([0.0_f32].as_slice()));

        cache.insert("q-overflow".into(), vec![999.0], now);
        assert!(cache.get("q-1", now).is_none());
        assert_eq!(cache.get("q-0", now).as_deref(), Some([0.0_f32].as_slice()));
        assert_eq!(
            cache.get("q-overflow", now).as_deref(),
            Some([999.0_f32].as_slice())
        );

        cache.insert("q-expired".into(), vec![1.0], now);
        assert!(
            cache
                .get("q-expired", now + QUERY_EMBEDDING_CACHE_TTL)
                .is_none()
        );
        cache.clear();
        assert!(cache.entries.is_empty());
        assert!(cache.lru.is_empty());
    }

    #[test]
    fn knowledge_events_rebuild_collection_and_source_projection() {
        let data_root = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspace_key = scope_key(workspace.path()).unwrap();
        let db = ProjectionDb::open(data_root.path()).unwrap();
        let repository = KnowledgeRepository::new(db.clone());
        repository
            .append(KnowledgeEventKind::KnowledgeEnabledChanged { enabled: true })
            .unwrap();
        repository
            .append(KnowledgeEventKind::CollectionUpserted {
                id: "collection-recovery".into(),
                name: "Recovered docs".into(),
                scope: "workspace".into(),
                scope_key: workspace_key.clone(),
                enabled: true,
            })
            .unwrap();
        repository
            .append(KnowledgeEventKind::SourceRegistered {
                id: "source-recovery".into(),
                collection_id: "collection-recovery".into(),
                workspace_id: workspace_key.clone(),
                relative_path: "docs/guide.md".into(),
                size_bytes: 42,
                modified_at_ms: 7,
            })
            .unwrap();
        let events_path = data_root.path().join("knowledge").join("events.jsonl");
        let events = std::fs::read_to_string(&events_path).unwrap();
        assert!(events.contains("knowledge_source_registered"));

        db.with_connection(|connection| {
            connection.execute_batch(
                "DELETE FROM knowledge_chunk_embeddings;
                 DELETE FROM knowledge_chunks_fts;
                 DELETE FROM knowledge_chunks;
                 DELETE FROM knowledge_revisions;
                 DELETE FROM knowledge_index_jobs;
                 DELETE FROM knowledge_sources;
                 DELETE FROM knowledge_collections;
                 DELETE FROM settings WHERE key LIKE 'knowledge.%';",
            )?;
            Ok(())
        })
        .unwrap();
        repository.rebuild_projection().unwrap();

        let service = KnowledgeService::new(db, Arc::new(FakeCredentialStore::default()));
        let collections = service.list_collections(workspace.path()).unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].id, "collection-recovery");
        assert!(service.settings().unwrap().enabled);
        let sources = service
            .list_sources(workspace.path(), "collection-recovery")
            .unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].relative_path, "docs/guide.md");
        assert_eq!(sources[0].state, "queued");
    }

    #[test]
    fn knowledge_events_ignore_a_truncated_final_line() {
        let data_root = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspace_key = scope_key(workspace.path()).unwrap();
        let db = ProjectionDb::open(data_root.path()).unwrap();
        let repository = KnowledgeRepository::new(db.clone());
        repository
            .append(KnowledgeEventKind::CollectionUpserted {
                id: "collection-truncated".into(),
                name: "Truncated".into(),
                scope: "workspace".into(),
                scope_key: workspace_key,
                enabled: true,
            })
            .unwrap();
        let events_path = data_root.path().join("knowledge").join("events.jsonl");
        let mut file = OpenOptions::new().append(true).open(&events_path).unwrap();
        file.write_all(b"{\"schemaVersion\":1,\"eventId\":\"partial\"")
            .unwrap();
        file.flush().unwrap();
        repository.rebuild_projection().unwrap();
        let count = db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM knowledge_collections WHERE id='collection-truncated'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn embedding_payload_validation_rejects_profile_index_dimension_and_zero_vectors() {
        let valid = json!({
            "object": "list",
            "model": EMBEDDING_MODEL,
            "data": [
                {"object":"embedding","index":0,"embedding":[3.0,4.0]},
                {"object":"embedding","index":1,"embedding":[4.0,3.0]}
            ]
        });
        assert_eq!(validate_embedding_payload(&valid, 2).unwrap().len(), 2);

        let cases = [
            (
                json!({"object":"invalid","model":EMBEDDING_MODEL,"data":[]}),
                0,
                "KC_EMBEDDING_RESPONSE_INVALID",
            ),
            (
                json!({"object":"list","model":"other/model","data":[]}),
                0,
                "KC_EMBEDDING_MODEL_NOT_FOUND",
            ),
            (
                json!({"object":"list","model":EMBEDDING_MODEL,"data":[{"object":"embedding","index":1,"embedding":[1.0]}]}),
                1,
                "KC_EMBEDDING_RESPONSE_INVALID",
            ),
            (
                json!({"object":"list","model":EMBEDDING_MODEL,"data":[{"object":"embedding","index":0,"embedding":[1.0]},{"object":"embedding","index":1,"embedding":[1.0,2.0]}]}),
                2,
                "KC_EMBEDDING_DIMENSION_MISMATCH",
            ),
            (
                json!({"object":"list","model":EMBEDDING_MODEL,"data":[{"object":"embedding","index":0,"embedding":[0.0,0.0]}]}),
                1,
                "KC_EMBEDDING_RESPONSE_INVALID",
            ),
        ];
        for (payload, count, expected_code) in cases {
            let error = validate_embedding_payload(&payload, count).unwrap_err();
            assert_eq!(error.code(), expected_code);
        }
    }

    #[tokio::test]
    async fn invalid_embedding_json_is_rejected_without_retry() {
        let (endpoint, attempts, server) =
            spawn_embedding_server(vec![(200, "not-json".into())]).await;
        let credentials = Arc::new(FakeCredentialStore(std::sync::Mutex::new(Some(
            "test-key".into(),
        ))));
        let service =
            KnowledgeService::new_for_test(ProjectionDb::memory().unwrap(), credentials, endpoint);
        let error = service.embed_query("invalid json").await.unwrap_err();
        assert_eq!(error.code(), "KC_EMBEDDING_RESPONSE_INVALID");
        server.await.unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn citation_context_reads_adjacent_chunks_and_rejects_other_turns() {
        let root = tempfile::tempdir().unwrap();
        let paragraph = |label: &str| format!("{label} {}", "context ".repeat(24));
        std::fs::write(
            root.path().join("context.md"),
            format!(
                "{}\n\n{}\n\n{}",
                paragraph("BEFORE_MARKER"),
                paragraph("MIDDLE_MARKER"),
                paragraph("AFTER_MARKER")
            ),
        )
        .unwrap();
        let db = ProjectionDb::memory().unwrap();
        let service = KnowledgeService::new(db, Arc::new(FakeCredentialStore::default()));
        service.set_enabled(true).unwrap();
        service
            .db
            .set_setting("knowledge.max_chunk_tokens", "128")
            .unwrap();
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Context".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id.clone(),
                    workspace_relative_path: "context.md".into(),
                },
            )
            .await
            .unwrap();
        wait_for_job(&service, source.initial_job_id.as_deref().unwrap()).await;
        let response = service
            .search(root.path(), "thread", "turn", "MIDDLE_MARKER", 6)
            .await
            .unwrap();
        assert_eq!(response.results.len(), 1);
        let citation_id = &response.results[0].citation_id;
        let expanded = service
            .read_citation("thread", "turn", citation_id, 1, 1)
            .unwrap();
        assert!(expanded.text.contains("BEFORE_MARKER"));
        assert!(expanded.text.contains("MIDDLE_MARKER"));
        assert!(expanded.text.contains("AFTER_MARKER"));
        assert!(matches!(
            service.read_citation("other-thread", "turn", citation_id, 0, 0),
            Err(KnowledgeError::Coded {
                code: "KC_CITATION_FORBIDDEN",
                ..
            })
        ));
        service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: Some(collection.id),
                    name: "Context".into(),
                    scope: None,
                    enabled: false,
                },
            )
            .unwrap();
        assert!(matches!(
            service.read_citation("thread", "turn", citation_id, 0, 0),
            Err(KnowledgeError::Coded {
                code: "KC_CITATION_STALE",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn empty_queries_are_rejected_before_disabled_or_scope_checks() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::new(
            ProjectionDb::memory().unwrap(),
            Arc::new(FakeCredentialStore::default()),
        );
        assert!(matches!(
            service
                .search(root.path(), "thread", "turn", "   ", 6)
                .await,
            Err(KnowledgeError::Coded {
                code: "KC_QUERY_EMPTY",
                ..
            })
        ));
    }

    #[test]
    fn rejects_path_escape_and_unsupported_extensions() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            resolve_source_path(root.path(), "../secret.md"),
            Err(KnowledgeError::Coded {
                code: "KC_PATH_OUTSIDE_WORKSPACE",
                ..
            })
        ));
        std::fs::write(root.path().join("image.bin"), [1, 2, 3]).unwrap();
        assert!(matches!(
            resolve_source_path(root.path(), "image.bin"),
            Err(KnowledgeError::Coded { .. })
        ));
    }

    #[tokio::test]
    async fn cancelled_embedding_request_stops_before_network() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = send_embedding_request(
            &reqwest::Client::new(),
            EMBEDDING_ENDPOINT,
            "test-key",
            json!({"model": EMBEDDING_MODEL, "input": "test"}),
            &cancellation,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "KC_CANCELLED");
    }

    #[tokio::test]
    async fn embedding_http_retries_429_and_504_before_success() {
        let (endpoint, attempts, server) = spawn_embedding_server(vec![
            (429, "{}".into()),
            (504, "{}".into()),
            (200, "{}".into()),
        ])
        .await;
        let cancellation = CancellationToken::new();
        let (response, retry_count) = send_embedding_request(
            &reqwest::Client::new(),
            &endpoint,
            "test-key",
            json!({"model": EMBEDDING_MODEL, "input": "retry"}),
            &cancellation,
        )
        .await
        .unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(retry_count, 2);
        server.await.unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn embedding_503_fallback_records_retries_and_preserves_old_active_revision() {
        let responses = (0..6)
            .map(|_| (503, r#"{"error":"unavailable"}"#.to_string()))
            .collect();
        let (endpoint, attempts, server) = spawn_embedding_server(responses).await;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("fallback.md"), "first revision").unwrap();
        let credentials = Arc::new(FakeCredentialStore(std::sync::Mutex::new(Some(
            "test-key".into(),
        ))));
        let service =
            KnowledgeService::new_for_test(ProjectionDb::memory().unwrap(), credentials, endpoint);
        service
            .set_embedding_settings(SetEmbeddingSettingsRequest {
                semantic_enabled: true,
                batch_size: 16,
                timeout_ms: 1_000,
                max_vector_scan_chunks: 10_000,
            })
            .unwrap();
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Fallback".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id,
                    workspace_relative_path: "fallback.md".into(),
                },
            )
            .await
            .unwrap();
        let first_job = wait_for_job(&service, source.initial_job_id.as_deref().unwrap()).await;
        assert_eq!(first_job.state, "completed");
        assert_eq!(first_job.embedding_mode, "lexical_only");
        assert_eq!(first_job.embedding_requests, 1);
        assert_eq!(first_job.retry_count, 2);
        assert_eq!(first_job.last_http_status, Some(503));
        assert_eq!(
            first_job.error_code.as_deref(),
            Some("KC_EMBEDDING_UNAVAILABLE")
        );
        let first_source = service.source_view(&source.source_id).unwrap();
        let first_revision = first_source.active_revision_id.clone().unwrap();
        let first_hash = first_source.content_hash_prefix.clone();

        std::fs::write(root.path().join("fallback.md"), "second revision").unwrap();
        let refresh = service
            .refresh_source(root.path(), &source.source_id)
            .await
            .unwrap();
        let second_job = wait_for_job(&service, &refresh.job_id).await;
        assert_eq!(
            second_job.error_code.as_deref(),
            Some("KC_EMBEDDING_UNAVAILABLE")
        );
        assert_eq!(second_job.retry_count, 2);
        let restored = service.source_view(&source.source_id).unwrap();
        assert_eq!(
            restored.active_revision_id.as_deref(),
            Some(first_revision.as_str())
        );
        assert_eq!(restored.content_hash_prefix, first_hash);
        assert_eq!(restored.embedding_status, "lexical_only");
        let failed_candidates = service
            .db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM knowledge_revisions WHERE source_id=?1 AND active=0 AND embedding_status='embedding_failed'",
                    [&source.source_id],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap();
        assert_eq!(failed_candidates, 1);
        server.await.unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 6);
    }

    #[tokio::test]
    async fn cancelling_active_revisions_preserves_lexical_and_semantic_indexes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("guide.md"), "lexical fallback").unwrap();
        let db = ProjectionDb::memory().unwrap();
        let service = KnowledgeService::new(db, Arc::new(FakeCredentialStore::default()));
        service.set_enabled(true).unwrap();
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Docs".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id,
                    workspace_relative_path: "guide.md".into(),
                },
            )
            .await
            .unwrap();
        wait_for_job(&service, source.initial_job_id.as_deref().unwrap()).await;
        let source = service.source_view(&source.source_id).unwrap();
        let revision_id = source.active_revision_id.clone().unwrap();
        let job_id = Uuid::new_v4().to_string();
        service
            .db
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO knowledge_index_jobs(id,source_id,state,total_chunks,total_bytes,created_at_ms) VALUES(?1,?2,'running',1,1,?3)",
                    params![job_id, source.source_id, now_ms()],
                )?;
                Ok(())
            })
            .unwrap();
        let job = service
            .finish_cancelled(&job_id, &source.source_id, &revision_id, true, true)
            .unwrap();
        assert_eq!(job.state, "cancelled");
        let restored = service.source_view(&source.source_id).unwrap();
        assert_eq!(restored.state, "indexed");
        assert_eq!(restored.embedding_status, "lexical_only");
        assert_eq!(
            restored.active_revision_id.as_deref(),
            Some(revision_id.as_str())
        );
        let response = service
            .search(root.path(), "thread", "turn", "lexical fallback", 6)
            .await
            .unwrap();
        assert_eq!(response.results.len(), 1);

        let semantic_job_id = Uuid::new_v4().to_string();
        service
            .db
            .with_connection(|connection| {
                let chunk_id: String = connection.query_row(
                    "SELECT id FROM knowledge_chunks WHERE revision_id=?1 LIMIT 1",
                    [&revision_id],
                    |row| row.get(0),
                )?;
                let vector = 1.0_f32.to_le_bytes().to_vec();
                connection.execute(
                    "UPDATE knowledge_revisions SET embedding_provider=?2,embedding_model=?3,embedding_dimension=1,embedding_status='semantic_ready' WHERE id=?1",
                    params![revision_id, EMBEDDING_PROVIDER, EMBEDDING_MODEL],
                )?;
                connection.execute(
                    "UPDATE knowledge_sources SET embedding_status='semantic_ready',active_embedding_model=?2,active_embedding_dimension=1 WHERE id=?1",
                    params![source.source_id, EMBEDDING_MODEL],
                )?;
                connection.execute(
                    "INSERT INTO knowledge_chunk_embeddings(chunk_id,revision_id,provider,model,dimension,encoding_format,vector,vector_hash,created_at_ms) VALUES(?1,?2,?3,?4,1,?5,?6,?7,?8)",
                    params![chunk_id, revision_id, EMBEDDING_PROVIDER, EMBEDDING_MODEL, EMBEDDING_ENCODING, vector, hash_bytes(&1.0_f32.to_le_bytes()), now_ms()],
                )?;
                connection.execute(
                    "INSERT INTO knowledge_index_jobs(id,source_id,state,total_chunks,total_bytes,created_at_ms) VALUES(?1,?2,'running',1,1,?3)",
                    params![semantic_job_id, source.source_id, now_ms()],
                )?;
                Ok(())
            })
            .unwrap();
        let semantic_job = service
            .finish_cancelled(
                &semantic_job_id,
                &source.source_id,
                &revision_id,
                true,
                true,
            )
            .unwrap();
        assert_eq!(semantic_job.embedding_mode, "semantic_ready");
        assert_eq!(semantic_job.vector_count, 1);
        let restored = service.source_view(&source.source_id).unwrap();
        assert_eq!(restored.embedding_status, "semantic_ready");
        assert_eq!(
            restored.active_embedding_model.as_deref(),
            Some(EMBEDDING_MODEL)
        );
        let persisted_vectors = service
            .db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM knowledge_chunk_embeddings WHERE revision_id=?1",
                    [&revision_id],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap();
        assert_eq!(persisted_vectors, 1);
    }

    #[tokio::test]
    async fn repeated_refresh_reuses_revision_without_duplicate_chunks() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("reuse.md"), "stable knowledge").unwrap();
        let service = KnowledgeService::new(
            ProjectionDb::memory().unwrap(),
            Arc::new(FakeCredentialStore::default()),
        );
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Reuse".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id,
                    workspace_relative_path: "reuse.md".into(),
                },
            )
            .await
            .unwrap();
        wait_for_job(&service, source.initial_job_id.as_deref().unwrap()).await;
        let indexed = service.source_view(&source.source_id).unwrap();
        let active_revision = indexed.active_revision_id.clone().unwrap();

        let refresh = service
            .refresh_source(root.path(), &source.source_id)
            .await
            .unwrap();
        let refresh = wait_for_job(&service, &refresh.job_id).await;
        assert_eq!(refresh.state, "reused");
        let refreshed = service.source_view(&source.source_id).unwrap();
        assert_eq!(
            refreshed.active_revision_id.as_deref(),
            Some(active_revision.as_str())
        );
        let (revision_count, chunk_count) = service
            .db
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT COUNT(*) FROM knowledge_revisions WHERE source_id=?1",
                        [&source.source_id],
                        |row| row.get::<_, i64>(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM knowledge_chunks WHERE revision_id=?1",
                        [&active_revision],
                        |row| row.get::<_, i64>(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(revision_count, 1);
        assert_eq!(chunk_count, 1);

        let late_cancel = service
            .finish_cancelled(
                &refresh.job_id,
                &source.source_id,
                &active_revision,
                true,
                true,
            )
            .unwrap();
        assert_eq!(late_cancel.state, "reused");
        assert_eq!(
            service
                .source_view(&source.source_id)
                .unwrap()
                .last_error_code,
            None
        );
    }

    #[tokio::test]
    async fn duplicate_refreshes_share_one_job_and_cancellation_keeps_active_revision() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("cancel.md"), "active knowledge").unwrap();
        let service = KnowledgeService::new(
            ProjectionDb::memory().unwrap(),
            Arc::new(FakeCredentialStore::default()),
        );
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Cancel".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id,
                    workspace_relative_path: "cancel.md".into(),
                },
            )
            .await
            .unwrap();
        wait_for_job(&service, source.initial_job_id.as_deref().unwrap()).await;
        let active_revision = service
            .source_view(&source.source_id)
            .unwrap()
            .active_revision_id
            .unwrap();
        let source_lock = {
            let mut locks = service.source_locks.lock().unwrap();
            locks
                .entry(source.source_id.clone())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        let source_guard = source_lock.lock().await;
        let first = service
            .refresh_source(root.path(), &source.source_id)
            .await
            .unwrap();
        tokio::task::yield_now().await;
        let second = service
            .refresh_source(root.path(), &source.source_id)
            .await
            .unwrap();
        assert_eq!(first.job_id, second.job_id);
        let cancelled = service.cancel_job(&first.job_id).unwrap();
        assert_eq!(cancelled.state, "cancelled");
        service
            .finish_failed_job(&first.job_id, &source.source_id, "KC_TEST_FAILURE")
            .unwrap();
        drop(source_guard);
        let source = service.source_view(&source.source_id).unwrap();
        assert_eq!(
            source.active_revision_id.as_deref(),
            Some(active_revision.as_str())
        );
        assert_eq!(source.embedding_status, "lexical_only");
        assert_eq!(source.last_error_code.as_deref(), Some("KC_CANCELLED"));
    }

    #[test]
    fn cancelling_queued_first_index_leaves_source_retryable() {
        let root = tempfile::tempdir().unwrap();
        let db = ProjectionDb::memory().unwrap();
        let service = KnowledgeService::new(db, Arc::new(FakeCredentialStore::default()));
        let collection_id = Uuid::new_v4().to_string();
        let source_id = Uuid::new_v4().to_string();
        let job_id = Uuid::new_v4().to_string();
        let workspace = scope_key(root.path()).unwrap();
        service
            .db
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO knowledge_collections(id,name,scope,scope_key,enabled,deleted,created_at_ms,updated_at_ms) VALUES(?1,'Cancel queued','workspace',?2,1,0,?3,?3)",
                    params![collection_id, workspace, now_ms()],
                )?;
                connection.execute(
                    "INSERT INTO knowledge_sources(id,collection_id,workspace_id,relative_path,size_bytes,modified_at_ms,state,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,'queued.md',1,?4,'queued',?4,?4)",
                    params![source_id, collection_id, workspace, now_ms()],
                )?;
                connection.execute(
                    "INSERT INTO knowledge_index_jobs(id,source_id,stage,embedding_mode,state,created_at_ms) VALUES(?1,?2,'queued','lexical_only','queued',?3)",
                    params![job_id, source_id, now_ms()],
                )?;
                Ok(())
            })
            .unwrap();
        let job = service.cancel_job(&job_id).unwrap();
        assert_eq!(job.state, "cancelled");
        let source = service.source_view(&source_id).unwrap();
        assert_eq!(source.state, "cancelled");
        assert_eq!(source.initial_job_id, None);
    }

    #[tokio::test]
    async fn startup_recovers_running_jobs_and_finishes_them_in_background() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("recover.md"), "recovered knowledge").unwrap();
        let db = ProjectionDb::memory().unwrap();
        let collection_id = Uuid::new_v4().to_string();
        let source_id = Uuid::new_v4().to_string();
        let job_id = Uuid::new_v4().to_string();
        let workspace = scope_key(root.path()).unwrap();
        db.with_connection(|connection| {
            connection.execute(
                "INSERT INTO knowledge_collections(id,name,scope,scope_key,enabled,deleted,created_at_ms,updated_at_ms) VALUES(?1,'Recovery','workspace',?2,1,0,?3,?3)",
                params![collection_id, workspace, now_ms()],
            )?;
            connection.execute(
                "INSERT INTO knowledge_sources(id,collection_id,workspace_id,relative_path,size_bytes,modified_at_ms,state,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,'recover.md',19,?4,'indexing',?4,?4)",
                params![source_id, collection_id, workspace, now_ms()],
            )?;
            connection.execute(
                "INSERT INTO knowledge_index_jobs(id,source_id,stage,embedding_mode,state,created_at_ms,started_at_ms) VALUES(?1,?2,'parse','lexical_only','running',?3,?3)",
                params![job_id, source_id, now_ms()],
            )?;
            Ok(())
        })
        .unwrap();

        let service = KnowledgeService::new(db, Arc::new(FakeCredentialStore::default()));
        let recovered = wait_for_job(&service, &job_id).await;
        assert_eq!(recovered.state, "completed");
        let source = service.source_view(&source_id).unwrap();
        assert_eq!(source.state, "indexed");
        assert!(source.active_revision_id.is_some());
        assert_eq!(source.chunk_count, 1);
    }

    #[tokio::test]
    async fn deleting_source_purges_derived_indexes_and_keeps_workspace_file() {
        let root = tempfile::tempdir().unwrap();
        let source_path = root.path().join("delete-source.md");
        std::fs::write(&source_path, "deletion marker\n\nsecond paragraph").unwrap();
        let service = KnowledgeService::new(
            ProjectionDb::memory().unwrap(),
            Arc::new(FakeCredentialStore::default()),
        );
        service.set_enabled(true).unwrap();
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Delete source".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id.clone(),
                    workspace_relative_path: "delete-source.md".into(),
                },
            )
            .await
            .unwrap();
        wait_for_job(&service, source.initial_job_id.as_deref().unwrap()).await;
        let response = service
            .search(root.path(), "thread", "turn", "deletion marker", 6)
            .await
            .unwrap();
        let citation_id = response.results[0].citation_id.clone();
        let (revision_id, chunk_id) = service
            .db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT k.revision_id,k.id FROM knowledge_chunks k JOIN knowledge_revisions r ON r.id=k.revision_id WHERE r.source_id=?1 LIMIT 1",
                    [&source.source_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
            })
            .unwrap();
        service
            .db
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO knowledge_chunk_embeddings(chunk_id,revision_id,provider,model,dimension,encoding_format,vector,vector_hash,created_at_ms) VALUES(?1,?2,?3,?4,1,?5,?6,?7,?8)",
                    params![chunk_id, revision_id, EMBEDDING_PROVIDER, EMBEDDING_MODEL, EMBEDDING_ENCODING, 1.0_f32.to_le_bytes().to_vec(), hash_bytes(&1.0_f32.to_le_bytes()), now_ms()],
                )?;
                Ok(())
            })
            .unwrap();

        let wrong_confirmation = service
            .delete_source(root.path(), &source.source_id, "wrong")
            .await
            .unwrap_err();
        assert_eq!(wrong_confirmation.code(), "KC_INVALID_ARGUMENT");
        let deleted = service
            .delete_source(root.path(), &source.source_id, &source.source_id)
            .await
            .unwrap();
        assert_eq!(deleted["purgedVectorCount"], 1);
        assert!(deleted["purgedChunkCount"].as_u64().unwrap() >= 1);
        assert!(source_path.exists());
        assert!(
            service
                .list_sources(root.path(), &collection.id)
                .unwrap()
                .is_empty()
        );
        let counts = service
            .db
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT COUNT(*) FROM knowledge_sources WHERE id=?1",
                        [&source.source_id],
                        |row| row.get::<_, i64>(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM knowledge_revisions WHERE source_id=?1",
                        [&source.source_id],
                        |row| row.get::<_, i64>(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM knowledge_chunks_fts WHERE chunk_id=?1",
                        [&chunk_id],
                        |row| row.get::<_, i64>(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM knowledge_chunk_embeddings WHERE chunk_id=?1",
                        [&chunk_id],
                        |row| row.get::<_, i64>(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(counts, (0, 0, 0, 0));
        assert!(
            service
                .search(root.path(), "thread", "after-delete", "deletion marker", 6)
                .await
                .unwrap()
                .results
                .is_empty()
        );
        assert_eq!(
            service
                .read_citation("thread", "turn", &citation_id, 0, 0)
                .unwrap_err()
                .code(),
            "KC_CITATION_FORBIDDEN"
        );
    }

    #[tokio::test]
    async fn deleting_collection_cancels_active_refresh_before_purging_sources() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("delete-collection.md"),
            "collection marker",
        )
        .unwrap();
        let service = KnowledgeService::new(
            ProjectionDb::memory().unwrap(),
            Arc::new(FakeCredentialStore::default()),
        );
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Delete collection".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id.clone(),
                    workspace_relative_path: "delete-collection.md".into(),
                },
            )
            .await
            .unwrap();
        wait_for_job(&service, source.initial_job_id.as_deref().unwrap()).await;
        let source_lock = {
            let mut locks = service.source_locks.lock().unwrap();
            locks
                .entry(source.source_id.clone())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        let source_guard = source_lock.lock().await;
        let refresh = service
            .refresh_source(root.path(), &source.source_id)
            .await
            .unwrap();
        let delete_service = service.clone();
        let delete_root = root.path().to_path_buf();
        let collection_id = collection.id.clone();
        let delete = tokio::spawn(async move {
            delete_service
                .delete_collection(&delete_root, &collection_id, &collection_id)
                .await
        });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if service.get_job(&refresh.job_id).unwrap().state == "cancelled" {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!delete.is_finished());
        drop(source_guard);
        let deleted = delete.await.unwrap().unwrap();
        assert_eq!(deleted["deletedSourceCount"], 1);
        assert_eq!(deleted["cancelledJobCount"], 1);
        assert!(service.list_collections(root.path()).unwrap().is_empty());
        assert!(matches!(
            service.refresh_source(root.path(), &source.source_id).await,
            Err(KnowledgeError::Coded {
                code: "KC_NOT_FOUND",
                ..
            })
        ));
        let recreated = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Delete collection".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        assert_eq!(recreated.id, collection.id);
    }

    #[tokio::test]
    async fn index_progress_events_cover_queued_running_and_terminal_states() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("guide.md"), "第一段内容\n\n第二段内容").unwrap();
        let db = ProjectionDb::memory().unwrap();
        let service = KnowledgeService::new(db, Arc::new(FakeCredentialStore::default()));
        service.set_enabled(true).unwrap();
        let sink = Arc::new(RecordingProgressSink::default());
        service.attach_progress_sink(sink.clone());
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Docs".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id.clone(),
                    workspace_relative_path: "guide.md".into(),
                },
            )
            .await
            .unwrap();
        let job_id = source.initial_job_id.clone().unwrap();
        let finished = wait_for_job(&service, &job_id).await;

        let events = sink.0.lock().unwrap().clone();
        let states = events
            .iter()
            .map(|event| event.state.clone())
            .collect::<Vec<_>>();
        assert!(
            states.iter().any(|state| state == "queued"),
            "缺少 queued 进度：{states:?}"
        );
        assert!(
            states.iter().any(|state| state == "running"),
            "缺少 running 进度：{states:?}"
        );
        let last = events.last().expect("至少一条进度事件");
        assert_eq!(last.job_id, job_id);
        assert_eq!(last.collection_id.as_deref(), Some(collection.id.as_str()));
        assert_eq!(last.state, finished.state);
        assert_eq!(last.percent, 100);
        assert_eq!(last.total_chunks, finished.total_chunks);
        let mut previous = 0u8;
        for event in &events {
            if event.total_bytes == 0 && event.total_chunks == 0 {
                continue;
            }
            assert!(event.percent >= previous, "进度回退：{event:?}");
            previous = event.percent;
        }

        let metrics = service.metrics_snapshot();
        assert_eq!(metrics.jobs_queued, 1);
        assert_eq!(metrics.jobs_completed + metrics.jobs_reused, 1);
        assert!(metrics.chunks_indexed >= finished.chunk_count.max(1));
        assert!(metrics.last_completed_at_ms.is_some());
        assert!(metrics.average_index_duration_ms <= metrics.total_index_duration_ms);
    }

    #[tokio::test]
    async fn finished_job_metrics_are_not_counted_twice() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("guide.md"), "去重验收").unwrap();
        let db = ProjectionDb::memory().unwrap();
        let service = KnowledgeService::new(db, Arc::new(FakeCredentialStore::default()));
        service.set_enabled(true).unwrap();
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Docs".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id,
                    workspace_relative_path: "guide.md".into(),
                },
            )
            .await
            .unwrap();
        let job_id = source.initial_job_id.clone().unwrap();
        wait_for_job(&service, &job_id).await;
        let before = service.metrics_snapshot();
        service.cancel_job(&job_id).unwrap();
        let after = service.metrics_snapshot();
        assert_eq!(before.finished_jobs(), after.finished_jobs());
        assert_eq!(before.chunks_indexed, after.chunks_indexed);
        assert_eq!(before.embedding_requests, after.embedding_requests);
    }

    #[tokio::test]
    async fn embedding_failure_reports_error_code_and_logs_without_content() {
        let (endpoint, _attempts, server) = spawn_embedding_server(vec![(500, "{}".into())]).await;
        let root = tempfile::tempdir().unwrap();
        let logs = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("secret.md"), "不得外泄的正文").unwrap();
        let credentials = Arc::new(FakeCredentialStore(Mutex::new(Some("test-key".into()))));
        let service =
            KnowledgeService::new_for_test(ProjectionDb::memory().unwrap(), credentials, endpoint);
        service.attach_logger(StructuredLogger::new(logs.path()).unwrap());
        let sink = Arc::new(RecordingProgressSink::default());
        service.attach_progress_sink(sink.clone());
        service.set_enabled(true).unwrap();
        service
            .set_embedding_settings(SetEmbeddingSettingsRequest {
                semantic_enabled: true,
                batch_size: 1,
                timeout_ms: 5_000,
                max_vector_scan_chunks: 100,
            })
            .unwrap();
        let collection = service
            .upsert_collection(
                root.path(),
                UpsertCollectionRequest {
                    id: None,
                    name: "Docs".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        let source = service
            .add_source(
                root.path(),
                AddSourceRequest {
                    collection_id: collection.id,
                    workspace_relative_path: "secret.md".into(),
                },
            )
            .await
            .unwrap();
        let job_id = source.initial_job_id.clone().unwrap();
        let finished = wait_for_job(&service, &job_id).await;
        assert_eq!(
            finished.error_code.as_deref(),
            Some("KC_EMBEDDING_RESPONSE_INVALID")
        );

        let events = sink.0.lock().unwrap().clone();
        assert_eq!(
            events.last().and_then(|event| event.error_code.as_deref()),
            Some("KC_EMBEDDING_RESPONSE_INVALID")
        );
        let metrics = service.metrics_snapshot();
        assert!(metrics.embedding_requests >= 1);
        assert_eq!(
            metrics.last_error_code.as_deref(),
            Some("KC_EMBEDDING_RESPONSE_INVALID")
        );

        let records = StructuredLogger::new(logs.path())
            .unwrap()
            .read_logs(crate::logging::LogQuery {
                limit: Some(50),
                level: None,
                event: None,
                after_timestamp_ms: None,
            })
            .unwrap();
        let serialized = serde_json::to_string(&records.records).unwrap();
        assert!(
            serialized.contains("knowledge_index_job_completed"),
            "缺少索引终态日志：{serialized}"
        );
        assert!(
            serialized.contains("knowledge_embedding_batch"),
            "缺少 embedding 批次日志：{serialized}"
        );
        assert!(!serialized.contains("不得外泄的正文"));
        assert!(!serialized.contains("secret.md"));
        assert!(!serialized.contains("test-key"));
        server.await.unwrap();
    }
}
