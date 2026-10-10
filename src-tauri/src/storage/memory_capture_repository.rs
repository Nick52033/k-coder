use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memory::{MemoryScope, MemoryScopeKind, Sensitivity, detect_sensitivity};
use crate::persistence::{ProjectionDb, ProjectionError};
use crate::storage::event_validation::{validate_bounded, validate_id};
use crate::storage::now_ms;

pub const MEMORY_CAPTURE_SCHEMA_VERSION: u32 = 1;
pub const MAX_CAPTURE_SUMMARY_CHARS: usize = 900;
pub const MAX_CAPTURE_QUEUED_JOBS: usize = 256;
pub const MAX_DREAM_CAPTURE_SUMMARIES: usize = 8;
const MAX_CAPTURE_EVENT_BYTES: usize = 32 * 1024;
static CAPTURE_WRITER: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl CaptureJobStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureCounts {
    pub candidates: u32,
    pub accepted: u32,
    pub pending: u32,
    pub suppressed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryCaptureJob {
    pub turn_id: String,
    pub thread_id: String,
    pub scope: MemoryScope,
    pub summary: String,
    pub started_at_ms: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub status: CaptureJobStatus,
    pub attempts: u32,
    pub counts: CaptureCounts,
    pub reason: Option<String>,
    pub dream_processed: bool,
}

impl MemoryCaptureJob {
    fn validate(&self) -> Result<(), ProjectionError> {
        validate_id(&self.turn_id, "turnId")?;
        validate_id(&self.thread_id, "threadId")?;
        self.scope
            .validate()
            .map_err(|_| invalid("invalid capture scope"))?;
        if self.scope.kind == MemoryScopeKind::User {
            return Err(invalid("automatic capture cannot use global user scope"));
        }
        validate_bounded(&self.summary, "summary", 1, MAX_CAPTURE_SUMMARY_CHARS)?;
        if detect_sensitivity(&self.summary) != Sensitivity::Normal {
            return Err(invalid("capture summary contains sensitive material"));
        }
        if let Some(reason) = &self.reason {
            validate_safe_reason(reason)?;
        }
        if self.counts.candidates > 8
            || self.counts.accepted.saturating_add(self.counts.pending) > self.counts.candidates
            || self.counts.suppressed > 8
        {
            return Err(invalid("invalid capture candidate counts"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CaptureMetadata {
    last_capture_at_ms: Option<u64>,
    last_extraction_at_ms: Option<u64>,
    last_counts: CaptureCounts,
    last_reason: Option<String>,
    provider_unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDiagnostics {
    pub schema_version: u32,
    pub queued_jobs: u64,
    pub running_jobs: u64,
    pub completed_jobs: u64,
    pub failed_jobs: u64,
    pub skipped_jobs: u64,
    pub dream_pending_summaries: u64,
    pub last_capture_at_ms: Option<u64>,
    pub last_extraction_at_ms: Option<u64>,
    pub last_candidate_count: u32,
    pub last_accepted_count: u32,
    pub last_pending_count: u32,
    pub last_suppressed_count: u32,
    pub last_reason: Option<String>,
    pub provider_unavailable_reason: Option<String>,
    pub recovery_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CaptureEvent {
    schema_version: u32,
    event_id: String,
    at_ms: u64,
    kind: CaptureEventKind,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum CaptureEventKind {
    Captured(MemoryCaptureJob),
    JobChanged(MemoryCaptureJob),
    DreamProcessed(Vec<String>),
    Diagnostic(CaptureMetadata),
}

impl CaptureEvent {
    fn validate(&self) -> Result<(), ProjectionError> {
        if self.schema_version != MEMORY_CAPTURE_SCHEMA_VERSION {
            return Err(invalid("unsupported memory capture event schema"));
        }
        validate_id(&self.event_id, "eventId")?;
        match &self.kind {
            CaptureEventKind::Captured(job) | CaptureEventKind::JobChanged(job) => job.validate(),
            CaptureEventKind::DreamProcessed(ids) => {
                if ids.is_empty() || ids.len() > MAX_DREAM_CAPTURE_SUMMARIES {
                    return Err(invalid("invalid Dream summary batch size"));
                }
                for id in ids {
                    validate_id(id, "turnId")?;
                }
                Ok(())
            }
            CaptureEventKind::Diagnostic(metadata) => {
                for reason in [&metadata.last_reason, &metadata.provider_unavailable_reason]
                    .into_iter()
                    .flatten()
                {
                    validate_safe_reason(reason)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone)]
pub struct MemoryCaptureRepository {
    db: ProjectionDb,
    events_path: Option<PathBuf>,
}

impl MemoryCaptureRepository {
    pub fn new(db: ProjectionDb) -> Self {
        let events_path = db
            .data_root()
            .map(|root| root.join("memory/capture-events.jsonl"));
        Self { db, events_path }
    }

    pub fn initialize(&self) -> Result<(), ProjectionError> {
        let _guard = CAPTURE_WRITER
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        self.db.with_connection(|connection| {
            connection.execute_batch(
                "CREATE TABLE IF NOT EXISTS memory_capture_jobs (
                    turn_id TEXT PRIMARY KEY, scope TEXT NOT NULL, status TEXT NOT NULL,
                    dream_processed INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
                    payload TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS memory_capture_queue
                    ON memory_capture_jobs(status, created_at_ms, turn_id);
                 CREATE TABLE IF NOT EXISTS memory_capture_metadata (
                    singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload TEXT NOT NULL
                 );",
            )
        })?;
        if let Some(path) = &self.events_path {
            self.validate_log_path()?;
            if path.exists() {
                // Validate the entire durable prefix before replacing any projection rows.
                let valid_bytes = visit_events(path, |_| Ok(()))?;
                let length = fs::metadata(path).map_err(io_error)?.len();
                if valid_bytes < length {
                    OpenOptions::new()
                        .write(true)
                        .open(path)
                        .and_then(|file| file.set_len(valid_bytes).and_then(|_| file.sync_data()))
                        .map_err(io_error)?;
                }
                self.db.with_connection(|connection| {
                    let transaction = connection.transaction()?;
                    transaction.execute("DELETE FROM memory_capture_jobs", [])?;
                    transaction.execute("DELETE FROM memory_capture_metadata", [])?;
                    visit_events(path, |event| {
                        apply_event(&transaction, event).map_err(ProjectionError::from)
                    })
                    .map_err(sql_error)?;
                    transaction.commit()
                })?;
            }
        }
        for mut job in self.jobs_with_status(CaptureJobStatus::Running)? {
            job.status = CaptureJobStatus::Queued;
            job.updated_at_ms = now_ms();
            job.reason = Some("interrupted_requeued".into());
            self.append(CaptureEventKind::JobChanged(job))?;
        }
        let queued = self.jobs_with_status(CaptureJobStatus::Queued)?;
        let excess = queued.len().saturating_sub(MAX_CAPTURE_QUEUED_JOBS);
        for mut job in queued.into_iter().take(excess) {
            job.status = CaptureJobStatus::Skipped;
            job.reason = Some("queue_capacity".into());
            job.updated_at_ms = now_ms();
            self.append(CaptureEventKind::JobChanged(job))?;
        }
        Ok(())
    }

    pub fn enqueue(&self, job: MemoryCaptureJob) -> Result<bool, ProjectionError> {
        job.validate()?;
        if job.status != CaptureJobStatus::Queued || job.dream_processed || job.attempts != 0 {
            return Err(invalid("new capture jobs must be unprocessed and queued"));
        }
        let _guard = CAPTURE_WRITER
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        if self.get(&job.turn_id)?.is_some() {
            return Ok(false);
        }
        let queued = self.jobs_with_status(CaptureJobStatus::Queued)?;
        let evict = queued
            .len()
            .saturating_add(1)
            .saturating_sub(MAX_CAPTURE_QUEUED_JOBS);
        for mut old in queued.into_iter().take(evict) {
            old.status = CaptureJobStatus::Skipped;
            old.reason = Some("queue_capacity".into());
            old.updated_at_ms = now_ms();
            self.append(CaptureEventKind::JobChanged(old))?;
        }
        self.append(CaptureEventKind::Captured(job))?;
        Ok(true)
    }

    pub fn get(&self, turn_id: &str) -> Result<Option<MemoryCaptureJob>, ProjectionError> {
        self.db.with_connection(|connection| {
            let row = connection
                .query_row(
                    "SELECT payload,dream_processed FROM memory_capture_jobs WHERE turn_id=?1",
                    [turn_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
                )
                .optional()?;
            row.map(|(raw, processed)| {
                let mut job: MemoryCaptureJob = serde_json::from_str(&raw).map_err(sql_error)?;
                job.dream_processed |= processed;
                job.validate().map_err(sql_error)?;
                Ok(job)
            })
            .transpose()
        })
    }

    fn jobs_with_status(
        &self,
        status: CaptureJobStatus,
    ) -> Result<Vec<MemoryCaptureJob>, ProjectionError> {
        let ids = self.db.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_id FROM memory_capture_jobs WHERE status=?1
                 ORDER BY created_at_ms,turn_id",
            )?;
            statement
                .query_map([status.as_str()], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        ids.into_iter()
            .map(|id| self.get(&id)?.ok_or_else(|| invalid("missing capture job")))
            .collect()
    }

    pub fn next_queued(&self) -> Result<Option<MemoryCaptureJob>, ProjectionError> {
        let id = self.db.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT turn_id FROM memory_capture_jobs WHERE status='queued'
                     ORDER BY created_at_ms,turn_id LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })?;
        match id {
            Some(id) => self.get(&id),
            None => Ok(None),
        }
    }

    pub fn claim(&self, turn_id: &str) -> Result<Option<MemoryCaptureJob>, ProjectionError> {
        let _guard = CAPTURE_WRITER
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let Some(mut job) = self.get(turn_id)? else {
            return Ok(None);
        };
        if job.status != CaptureJobStatus::Queued {
            return Ok(None);
        }
        job.status = CaptureJobStatus::Running;
        job.attempts = job.attempts.saturating_add(1);
        job.updated_at_ms = now_ms();
        job.reason = None;
        self.append(CaptureEventKind::JobChanged(job.clone()))?;
        Ok(Some(job))
    }

    pub fn finish(
        &self,
        turn_id: &str,
        status: CaptureJobStatus,
        counts: CaptureCounts,
        reason: Option<&str>,
    ) -> Result<(), ProjectionError> {
        if status == CaptureJobStatus::Running {
            return Err(invalid("finish cannot start a capture job"));
        }
        let _guard = CAPTURE_WRITER
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let mut job = self
            .get(turn_id)?
            .ok_or_else(|| invalid("missing capture job"))?;
        if job.status != CaptureJobStatus::Running
            && !(job.status == CaptureJobStatus::Queued && status == CaptureJobStatus::Skipped)
        {
            return Err(invalid("capture job is not running"));
        }
        if status == CaptureJobStatus::Queued {
            let queued = self.jobs_with_status(CaptureJobStatus::Queued)?;
            let evict = queued
                .len()
                .saturating_add(1)
                .saturating_sub(MAX_CAPTURE_QUEUED_JOBS);
            for mut old in queued.into_iter().take(evict) {
                old.status = CaptureJobStatus::Skipped;
                old.reason = Some("queue_capacity".into());
                old.updated_at_ms = now_ms();
                self.append(CaptureEventKind::JobChanged(old))?;
            }
        }
        job.status = status;
        job.counts = counts;
        job.reason = reason.map(safe_reason);
        job.updated_at_ms = now_ms();
        self.append(CaptureEventKind::JobChanged(job))
    }

    pub fn summary_for_dream(
        &self,
    ) -> Result<Option<(MemoryScope, Vec<String>, Vec<String>)>, ProjectionError> {
        let ids = self.db.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_id FROM memory_capture_jobs
                 WHERE status='completed' AND dream_processed=0 AND scope=(
                    SELECT scope FROM memory_capture_jobs
                    WHERE status='completed' AND dream_processed=0
                    ORDER BY created_at_ms,turn_id LIMIT 1
                 ) ORDER BY created_at_ms,turn_id LIMIT ?1",
            )?;
            statement
                .query_map([MAX_DREAM_CAPTURE_SUMMARIES as u32], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()
        })?;
        let mut scope = None;
        let mut summaries = Vec::with_capacity(ids.len());
        for id in &ids {
            let job = self
                .get(id)?
                .ok_or_else(|| invalid("missing Dream summary"))?;
            scope.get_or_insert(job.scope);
            summaries.push(job.summary);
        }
        Ok(scope.map(|scope| (scope, summaries, ids)))
    }

    pub fn mark_dream_processed(&self, ids: &[String]) -> Result<(), ProjectionError> {
        if ids.is_empty() {
            return Ok(());
        }
        if ids.len() > MAX_DREAM_CAPTURE_SUMMARIES {
            return Err(invalid("too many Dream summary ids"));
        }
        let _guard = CAPTURE_WRITER
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let mut scope = None;
        let mut fresh = Vec::new();
        for id in ids {
            let job = self
                .get(id)?
                .ok_or_else(|| invalid("missing Dream summary"))?;
            if job.status != CaptureJobStatus::Completed
                || scope.as_ref().is_some_and(|scope| scope != &job.scope)
            {
                return Err(invalid(
                    "Dream summaries must be completed and share one scope",
                ));
            }
            scope.get_or_insert(job.scope);
            if !job.dream_processed && !fresh.contains(id) {
                fresh.push(id.clone());
            }
        }
        if fresh.is_empty() {
            return Ok(());
        }
        self.append(CaptureEventKind::DreamProcessed(fresh))
    }

    fn metadata(&self) -> Result<CaptureMetadata, ProjectionError> {
        self.db.with_connection(|connection| {
            let raw = connection
                .query_row(
                    "SELECT payload FROM memory_capture_metadata WHERE singleton=1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            raw.map(|raw| serde_json::from_str::<CaptureMetadata>(&raw).map_err(sql_error))
                .transpose()
                .map(|metadata| metadata.unwrap_or_default())
        })
    }

    pub fn note_reason(&self, reason: &str) -> Result<(), ProjectionError> {
        let _guard = CAPTURE_WRITER
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let mut metadata = self.metadata()?;
        let reason = Some(safe_reason(reason));
        if metadata.last_reason == reason {
            return Ok(());
        }
        metadata.last_reason = reason;
        self.append(CaptureEventKind::Diagnostic(metadata))
    }

    pub fn set_provider_unavailable_reason(
        &self,
        reason: Option<&str>,
    ) -> Result<(), ProjectionError> {
        let _guard = CAPTURE_WRITER
            .lock()
            .map_err(|_| ProjectionError::Poisoned)?;
        let mut metadata = self.metadata()?;
        let reason = reason.map(safe_reason);
        if metadata.provider_unavailable_reason == reason {
            return Ok(());
        }
        metadata.provider_unavailable_reason = reason;
        self.append(CaptureEventKind::Diagnostic(metadata))
    }

    pub fn diagnostics(&self) -> Result<MemoryDiagnostics, ProjectionError> {
        let metadata = self.metadata()?;
        let mut result = MemoryDiagnostics {
            schema_version: MEMORY_CAPTURE_SCHEMA_VERSION,
            last_capture_at_ms: metadata.last_capture_at_ms,
            last_extraction_at_ms: metadata.last_extraction_at_ms,
            last_candidate_count: metadata.last_counts.candidates,
            last_accepted_count: metadata.last_counts.accepted,
            last_pending_count: metadata.last_counts.pending,
            last_suppressed_count: metadata.last_counts.suppressed,
            last_reason: metadata.last_reason,
            provider_unavailable_reason: metadata.provider_unavailable_reason,
            ..MemoryDiagnostics::default()
        };
        self.db.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT status,COUNT(*) FROM memory_capture_jobs GROUP BY status")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
            })?;
            for row in rows {
                let (status, count) = row?;
                match status.as_str() {
                    "queued" => result.queued_jobs = count,
                    "running" => result.running_jobs = count,
                    "completed" => result.completed_jobs = count,
                    "failed" => result.failed_jobs = count,
                    "skipped" => result.skipped_jobs = count,
                    _ => return Err(sql_error("invalid capture status")),
                }
            }
            result.dream_pending_summaries = connection.query_row(
                "SELECT COUNT(*) FROM memory_capture_jobs
                 WHERE status='completed' AND dream_processed=0",
                [],
                |row| row.get(0),
            )?;
            Ok(())
        })?;
        Ok(result)
    }

    fn validate_log_path(&self) -> Result<(), ProjectionError> {
        let Some(path) = &self.events_path else {
            return Ok(());
        };
        let root = self
            .db
            .data_root()
            .ok_or_else(|| invalid("missing capture data root"))?;
        let root = root.canonicalize().map_err(io_error)?;
        let parent = path
            .parent()
            .ok_or_else(|| invalid("invalid capture log path"))?;
        if !parent.exists() {
            fs::create_dir(parent).map_err(io_error)?;
        }
        if !parent.canonicalize().map_err(io_error)?.starts_with(&root)
            || (path.exists() && !path.canonicalize().map_err(io_error)?.starts_with(&root))
        {
            return Err(invalid("capture log escapes runtime data root"));
        }
        Ok(())
    }

    fn append(&self, kind: CaptureEventKind) -> Result<(), ProjectionError> {
        let event = CaptureEvent {
            schema_version: MEMORY_CAPTURE_SCHEMA_VERSION,
            event_id: Uuid::new_v4().to_string(),
            at_ms: now_ms(),
            kind,
        };
        event.validate()?;
        let mut raw = serde_json::to_vec(&event).map_err(|_| invalid("serialize capture event"))?;
        raw.push(b'\n');
        if raw.len() > MAX_CAPTURE_EVENT_BYTES {
            return Err(invalid("capture event exceeds byte limit"));
        }
        if let Some(path) = &self.events_path {
            self.validate_log_path()?;
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| {
                    file.write_all(&raw)?;
                    file.flush()?;
                    file.sync_data()
                })
                .map_err(io_error)?;
        }
        self.db.with_connection(|connection| {
            let transaction = connection.transaction()?;
            apply_event(&transaction, &event)?;
            transaction.commit()
        })
    }
}

fn apply_event(connection: &Connection, event: &CaptureEvent) -> Result<(), rusqlite::Error> {
    let raw = connection
        .query_row(
            "SELECT payload FROM memory_capture_metadata WHERE singleton=1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let mut metadata: CaptureMetadata = raw
        .map(|raw| serde_json::from_str(&raw).map_err(sql_error))
        .transpose()?
        .unwrap_or_default();
    match &event.kind {
        CaptureEventKind::Captured(job) => {
            let changed = connection.execute(
                "INSERT OR IGNORE INTO memory_capture_jobs
                 (turn_id,scope,status,dream_processed,created_at_ms,payload)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    job.turn_id,
                    job.scope.canonical(),
                    job.status.as_str(),
                    job.dream_processed,
                    job.created_at_ms,
                    serde_json::to_string(job).map_err(sql_error)?,
                ],
            )?;
            if changed > 0 {
                metadata.last_capture_at_ms = Some(event.at_ms);
                metadata.last_reason = Some("queued".into());
            }
        }
        CaptureEventKind::JobChanged(job) => {
            connection.execute(
                "UPDATE memory_capture_jobs SET status=?2,
                 dream_processed=MAX(dream_processed,?3),payload=?4 WHERE turn_id=?1",
                params![
                    job.turn_id,
                    job.status.as_str(),
                    job.dream_processed,
                    serde_json::to_string(job).map_err(sql_error)?,
                ],
            )?;
            if job.status != CaptureJobStatus::Running {
                metadata.last_extraction_at_ms = Some(event.at_ms);
                metadata.last_counts = job.counts.clone();
                metadata.last_reason = job.reason.clone();
            }
        }
        CaptureEventKind::DreamProcessed(ids) => {
            for id in ids {
                connection.execute(
                    "UPDATE memory_capture_jobs SET dream_processed=1 WHERE turn_id=?1",
                    [id],
                )?;
            }
        }
        CaptureEventKind::Diagnostic(value) => metadata = value.clone(),
    }
    connection.execute(
        "INSERT INTO memory_capture_metadata(singleton,payload) VALUES(1,?1)
         ON CONFLICT(singleton) DO UPDATE SET payload=excluded.payload",
        [serde_json::to_string(&metadata).map_err(sql_error)?],
    )?;
    Ok(())
}

fn visit_events(
    path: &Path,
    mut visit: impl FnMut(&CaptureEvent) -> Result<(), ProjectionError>,
) -> Result<u64, ProjectionError> {
    let file = fs::File::open(path).map_err(io_error)?;
    let mut reader = BufReader::new(file);
    let mut valid_bytes = 0_u64;
    loop {
        let mut line = Vec::new();
        let read = (&mut reader)
            .take((MAX_CAPTURE_EVENT_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(io_error)?;
        if read == 0 {
            return Ok(valid_bytes);
        }
        if line.len() > MAX_CAPTURE_EVENT_BYTES {
            return Err(invalid("capture event exceeds byte limit"));
        }
        if line.last() != Some(&b'\n') {
            return Ok(valid_bytes);
        }
        if !line.iter().all(u8::is_ascii_whitespace) {
            let event: CaptureEvent = serde_json::from_slice(&line)
                .map_err(|_| invalid("invalid capture event log; projection retained"))?;
            event.validate()?;
            visit(&event)?;
        }
        valid_bytes = valid_bytes.saturating_add(read as u64);
    }
}

fn validate_safe_reason(value: &str) -> Result<(), ProjectionError> {
    validate_bounded(value, "reason", 1, 500)?;
    if detect_sensitivity(value) != Sensitivity::Normal {
        return Err(invalid("capture diagnostic contains sensitive material"));
    }
    Ok(())
}

fn safe_reason(value: &str) -> String {
    let value = crate::execution::redact(value);
    if detect_sensitivity(&value) != Sensitivity::Normal {
        return "sensitive_detail_omitted".into();
    }
    let value = value.chars().take(500).collect::<String>();
    if value.trim().is_empty() {
        "unspecified".into()
    } else {
        value
    }
}

fn invalid(message: &str) -> ProjectionError {
    ProjectionError::InvalidData(message.into())
}

fn io_error(_: std::io::Error) -> ProjectionError {
    invalid("memory capture event log I/O failed")
}

fn sql_error(error: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(error.to_string())))
}
