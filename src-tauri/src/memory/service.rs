//! User-mediated memory service.
//!
//! This is the only entry point the IPC layer uses. It owns settings, listing, user writes,
//! candidate recording, candidate review, deletion and scoped clearing, and it delegates every
//! persistence step to `MemoryRepository` so no SQL appears here.
//!
//! Two invariants the design calls out explicitly are enforced here rather than left to callers:
//!
//! * A write appends the fact event before the projection changes, so every memory is reconstructible.
//! * Deleting never removes a row. It records a status change, which keeps the audit trail intact and
//!   lets a rebuild reproduce exactly the same state.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memory::MemoryError;
use crate::memory::candidate::{
    CANDIDATE_STATUSES, CandidateDecision, CandidateDraft, CandidateOutcome, ConflictKind,
};
use crate::memory::entity::{
    MemoryCursor, MemoryOperation, MemoryScope, MemoryScopeKind, MemorySourceType, MemoryStatus,
    MemoryType, Sensitivity,
};
use crate::memory::policy::{
    MAX_MEMORY_TTL_DAYS, detect_sensitivity, effective_expiry, memory_is_expired,
    require_confirmation, scope_confirmation_token,
};
use crate::persistence::ProjectionDb;
use crate::storage::memory_repository::{
    CANDIDATE_STATUS_PENDING, MAX_MEMORY_CONTENT_CHARS, MAX_MEMORY_PAGE_SIZE,
    MemoryCandidateRecord, MemoryEventKind, MemoryRecord, MemoryRepository, MemoryStatusChange,
    MemoryWrite, normalize_memory_key,
};
use crate::storage::now_ms;

/// Settings row key inside the shared `settings` table.
const MEMORY_SETTINGS_KEY: &str = "memory.settings";
const MEMORY_SETTINGS_SCHEMA_VERSION: u32 = 1;
/// Default page size for `list_memories`.
const DEFAULT_MEMORY_PAGE_SIZE: u32 = 100;
/// Hard stop for a scoped clear, so a corrupted projection cannot spin forever.
const MAX_MEMORY_CLEAR_COUNT: u64 = 10_000;
/// Hard stop for one offline maintenance sweep, so a corrupted projection cannot spin forever.
pub const MAX_MEMORY_MAINTENANCE_SWEEP: usize = 10_000;

/// One `(scope, normalized_key)` group collapsed by `merge_duplicate_keys`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergedKeyGroup {
    pub scope: String,
    pub normalized_key: String,
    pub kept_id: String,
    pub archived_ids: Vec<String>,
}

/// Memory settings, versioned so an unknown schema closes the read instead of being coerced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySettings {
    pub schema_version: u32,
    /// Gates automatic capture and context injection (Tasks 3 and 4). User-mediated management stays
    /// available while memory is disabled, because the design requires memories to remain viewable,
    /// reviewable and clearable at all times.
    pub enabled: bool,
    /// Design §4.2: high-confidence, non-sensitive candidates may be applied without review, but only
    /// when the user opts in. The default queues everything for review.
    pub auto_accept_high_confidence: bool,
    /// Fallback TTL for memory types without a design-mandated TTL. `0` means "never expires".
    pub default_ttl_days: u32,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            schema_version: MEMORY_SETTINGS_SCHEMA_VERSION,
            enabled: false,
            auto_accept_high_confidence: false,
            default_ttl_days: 0,
        }
    }
}

/// One page of memories plus the cursor for the next page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPage {
    pub items: Vec<MemoryRecord>,
    pub next_cursor: Option<String>,
    pub total: u64,
    pub scope: String,
    pub status: String,
}

/// Result of a user-mediated write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryUpsertOutcome {
    pub memory: MemoryRecord,
    /// True when the exact same content already existed in this scope and nothing was written.
    pub deduplicated: bool,
}

/// Result of clearing one scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryClearOutcome {
    pub scope: String,
    pub cleared_count: u64,
}

/// Domain input for `upsert_memory`, built by the command layer from the wire request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertMemoryCommand {
    pub memory_id: Option<String>,
    pub content: String,
    pub memory_type: MemoryType,
    pub scope: MemoryScope,
    pub expires_at_ms: Option<u64>,
}

#[derive(Clone)]
pub struct MemoryService {
    db: ProjectionDb,
    repository: MemoryRepository,
}

impl MemoryService {
    /// Opens the service and projects the fact log.
    ///
    /// `legacy_enabled` seeds the settings row exactly once, so an installation that already enabled
    /// the Phase 9 memory store keeps the flag the `recall_memory` tool reads instead of silently
    /// reverting to disabled.
    pub fn new(db: ProjectionDb, legacy_enabled: bool) -> Self {
        let repository = MemoryRepository::new(db.clone());
        let service = Self { db, repository };
        if service.stored_settings().ok().flatten().is_none() {
            let mut settings = MemorySettings::default();
            settings.enabled = legacy_enabled;
            let _ = service.persist_settings(&settings);
        }
        let _ = service.repository.rebuild_projection();
        service
    }

    /// Rebuilds the SQLite projection from the fact log. Exposed so recovery paths and tests can
    /// assert the projection is reproducible.
    pub fn rebuild_projection(&self) -> Result<(), MemoryError> {
        Ok(self.repository.rebuild_projection()?)
    }

    pub fn settings(&self) -> Result<MemorySettings, MemoryError> {
        Ok(self.stored_settings()?.unwrap_or_default())
    }

    pub fn set_settings(
        &self,
        enabled: bool,
        auto_accept_high_confidence: bool,
        default_ttl_days: u32,
    ) -> Result<MemorySettings, MemoryError> {
        if default_ttl_days > MAX_MEMORY_TTL_DAYS {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                format!("defaultTtlDays must not exceed {MAX_MEMORY_TTL_DAYS}"),
            ));
        }
        let settings = MemorySettings {
            schema_version: MEMORY_SETTINGS_SCHEMA_VERSION,
            enabled,
            auto_accept_high_confidence,
            default_ttl_days,
        };
        self.persist_settings(&settings)?;
        Ok(settings)
    }

    fn stored_settings(&self) -> Result<Option<MemorySettings>, MemoryError> {
        let Some(raw) = self.db.setting(MEMORY_SETTINGS_KEY)? else {
            return Ok(None);
        };
        let settings = serde_json::from_str::<MemorySettings>(&raw).map_err(|_| {
            MemoryError::coded(
                "MEM_INVALID_DATA",
                "stored memory settings are not readable",
            )
        })?;
        if settings.schema_version != MEMORY_SETTINGS_SCHEMA_VERSION {
            return Err(MemoryError::coded(
                "MEM_INVALID_DATA",
                format!(
                    "unsupported memory settings schema {}",
                    settings.schema_version
                ),
            ));
        }
        Ok(Some(settings))
    }

    fn persist_settings(&self, settings: &MemorySettings) -> Result<(), MemoryError> {
        let raw = serde_json::to_string(settings)
            .map_err(|error| MemoryError::Storage(error.to_string()))?;
        Ok(self.db.set_setting(MEMORY_SETTINGS_KEY, &raw)?)
    }

    pub fn list(
        &self,
        scope: &MemoryScope,
        status: MemoryStatus,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<MemoryPage, MemoryError> {
        scope.validate()?;
        let limit = limit
            .unwrap_or(DEFAULT_MEMORY_PAGE_SIZE)
            .clamp(1, MAX_MEMORY_PAGE_SIZE);
        let after = cursor.map(MemoryCursor::decode).transpose()?;
        let after_pair = after
            .as_ref()
            .map(|cursor| (cursor.updated_at_ms, cursor.id.clone()));
        let records = self.repository.list_page(
            scope.kind.as_str(),
            scope.id.as_deref(),
            status.as_str(),
            after_pair.as_ref().map(|(ts, id)| (*ts, id.as_str())),
            limit,
        )?;
        // A full page means there may be more, so the caller gets a cursor; a short page is the end.
        let next_cursor = (records.len() == limit as usize)
            .then(|| records.last())
            .flatten()
            .map(|record| {
                MemoryCursor {
                    updated_at_ms: record.updated_at_ms,
                    id: record.id.clone(),
                }
                .encode()
            });
        let total = self.repository.count_in_scope(
            scope.kind.as_str(),
            scope.id.as_deref(),
            status.as_str(),
        )?;
        Ok(MemoryPage {
            items: records,
            next_cursor,
            total,
            scope: scope.canonical(),
            status: status.as_str().to_owned(),
        })
    }

    /// User-mediated create or update.
    ///
    /// Deduplication reuses the row when the same content already exists in the scope; otherwise the
    /// revision increments and the original creation time is preserved. Sensitivity is host-detected
    /// and credential-shaped content is refused outright — design §8 keeps secrets in the operating
    /// system credential slot, not in memory.
    pub fn upsert(&self, command: UpsertMemoryCommand) -> Result<MemoryUpsertOutcome, MemoryError> {
        let settings = self.settings()?;
        let now = now_ms();
        command.scope.validate()?;

        let content = command.content.trim().to_owned();
        validate_content(&content)?;
        let sensitivity = detect_sensitivity(&content);
        if sensitivity == Sensitivity::SecretCandidate {
            return Err(MemoryError::coded(
                "MEM_SECRET_REJECTED",
                "memory content looks like a credential; keep it in the operating system credential slot",
            ));
        }
        let normalized_key = normalize_memory_key(&content);
        if normalized_key.is_empty() {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "content must contain at least one non-whitespace character",
            ));
        }

        let existing = self
            .repository
            .list_by_key(&normalized_key, MAX_MEMORY_PAGE_SIZE)?;
        let in_scope = existing.iter().find(|record| {
            record.scope_type == command.scope.kind.as_str() && record.scope_id == command.scope.id
        });

        let target = match command.memory_id.as_deref() {
            Some(id) => {
                let id = id.trim();
                if id.is_empty() {
                    return Err(MemoryError::coded(
                        "MEM_INVALID_ARGUMENT",
                        "memoryId must not be blank",
                    ));
                }
                let record = self.repository.get(id)?.ok_or_else(|| {
                    MemoryError::coded("MEM_NOT_FOUND", format!("memory {id} was not found"))
                })?;
                if record.scope_type != command.scope.kind.as_str()
                    || record.scope_id != command.scope.id
                {
                    return Err(MemoryError::coded(
                        "MEM_INVALID_SCOPE",
                        "memoryId belongs to a different scope",
                    ));
                }
                // Editing into a key another row already owns would break the revision uniqueness the
                // projection relies on, so it is refused instead of surfacing as a database error.
                if let Some(other) = in_scope
                    && other.id != record.id
                {
                    return Err(MemoryError::coded(
                        "MEM_INVALID_ARGUMENT",
                        "another memory in this scope already stores this content",
                    ));
                }
                Some(record)
            }
            None => in_scope.cloned(),
        };

        let (id, revision, created_at_ms) = match target {
            Some(record) => {
                if record.content.trim() == content
                    && record.status == MemoryStatus::Active.as_str()
                {
                    return Ok(MemoryUpsertOutcome {
                        memory: record,
                        deduplicated: true,
                    });
                }
                // A previously deleted memory is revived by re-adding the same content, which keeps the
                // original id so its audit history stays in one place.
                (
                    record.id,
                    record.revision.saturating_add(1),
                    record.created_at_ms,
                )
            }
            None => (Uuid::new_v4().to_string(), 1, now),
        };

        let expires_at_ms = effective_expiry(
            command.memory_type,
            command.expires_at_ms,
            now,
            settings.default_ttl_days,
        )?;
        let write = MemoryWrite {
            id: id.clone(),
            scope_type: command.scope.kind.as_str().to_owned(),
            scope_id: command.scope.id.clone(),
            memory_type: command.memory_type.as_str().to_owned(),
            normalized_key,
            content,
            source_type: MemorySourceType::User.as_str().to_owned(),
            source_ref: None,
            confidence: 1.0,
            sensitivity: sensitivity.as_str().to_owned(),
            status: MemoryStatus::Active.as_str().to_owned(),
            revision,
            expires_at_ms,
            created_at_ms,
        };
        self.repository
            .append(MemoryEventKind::MemoryUpserted(write))?;
        let memory = self.projected(&id)?;
        Ok(MemoryUpsertOutcome {
            memory,
            deduplicated: false,
        })
    }

    /// Records a candidate and either applies it or queues it for review.
    ///
    /// The target id is always resolved by the host from the deduplication key, so a candidate can
    /// never name a memory row it did not discover. A `create` that collides with different content is
    /// reclassified as an `update` and always reviewed, because the caller's belief that the memory
    /// was new is exactly the kind of conflict the design requires a human to settle.
    pub fn record_candidate(&self, draft: CandidateDraft) -> Result<CandidateOutcome, MemoryError> {
        draft.validate()?;
        let settings = self.settings()?;
        let now = now_ms();
        let content = draft.content.trim().to_owned();
        let normalized_key = normalize_memory_key(&content);
        if normalized_key.is_empty() {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "content must contain at least one non-whitespace character",
            ));
        }

        let rows = self
            .repository
            .list_by_key(&normalized_key, MAX_MEMORY_PAGE_SIZE)?;
        let in_scope = rows.iter().find(|record| {
            record.scope_type == draft.scope.kind.as_str() && record.scope_id == draft.scope.id
        });

        let (operation, target_memory_id, conflict) = match draft.operation {
            MemoryOperation::Create => match in_scope {
                Some(existing) if existing.content.trim() == content => {
                    return Ok(CandidateOutcome::Deduplicated {
                        memory: existing.clone(),
                    });
                }
                Some(existing) => (
                    MemoryOperation::Update,
                    Some(existing.id.clone()),
                    ConflictKind::Conflict,
                ),
                None => (MemoryOperation::Create, None, ConflictKind::None),
            },
            MemoryOperation::Update | MemoryOperation::Merge | MemoryOperation::Delete => {
                let target = match draft.target_memory_id.as_deref() {
                    Some(id) => self.repository.get(id.trim())?.ok_or_else(|| {
                        MemoryError::coded("MEM_NOT_FOUND", format!("memory {id} was not found"))
                    })?,
                    None => in_scope.cloned().ok_or_else(|| {
                        MemoryError::coded("MEM_NOT_FOUND", "no memory matches this candidate")
                    })?,
                };
                let conflict = if target.scope_type != draft.scope.kind.as_str()
                    || target.scope_id != draft.scope.id
                {
                    ConflictKind::CrossScopeUpdate
                } else {
                    ConflictKind::None
                };
                (draft.operation, Some(target.id), conflict)
            }
        };

        let requires_review = draft.requires_review(conflict, settings.auto_accept_high_confidence);
        let candidate = MemoryCandidateRecord {
            id: Uuid::new_v4().to_string(),
            operation: operation.as_str().to_owned(),
            target_memory_id: target_memory_id.clone(),
            scope_type: draft.scope.kind.as_str().to_owned(),
            scope_id: draft.scope.id.clone(),
            memory_type: draft.memory_type.as_str().to_owned(),
            content,
            normalized_key,
            reason: draft.reason.trim().to_owned(),
            confidence: draft.confidence,
            requires_review,
            status: CANDIDATE_STATUS_PENDING.to_owned(),
            source_turn_id: draft.source_turn_id.clone(),
            created_at_ms: now,
            reviewed_at_ms: None,
        };
        self.repository
            .append(MemoryEventKind::CandidateRecorded(candidate.clone()))?;

        if requires_review {
            return Ok(CandidateOutcome::Pending { candidate });
        }

        let memory = self.apply_candidate(operation, target_memory_id.as_deref(), &draft, now)?;
        let candidate = self.review(&candidate.id, CandidateDecision::Accept)?;
        Ok(CandidateOutcome::AutoAccepted { memory, candidate })
    }

    pub fn list_candidates(
        &self,
        status: &str,
        limit: Option<u32>,
    ) -> Result<Vec<MemoryCandidateRecord>, MemoryError> {
        let status = status.trim();
        if !CANDIDATE_STATUSES.contains(&status) {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                format!(
                    "candidate status must be one of {}",
                    CANDIDATE_STATUSES.join(", ")
                ),
            ));
        }
        let limit = limit
            .unwrap_or(DEFAULT_MEMORY_PAGE_SIZE)
            .clamp(1, MAX_MEMORY_PAGE_SIZE);
        Ok(self.repository.list_candidates(status, limit)?)
    }

    /// Applies or discards a pending candidate. A candidate can only be reviewed once, so a stale UI
    /// cannot re-apply a decision the user already made.
    pub fn review_candidate(
        &self,
        candidate_id: &str,
        decision: CandidateDecision,
    ) -> Result<MemoryCandidateRecord, MemoryError> {
        self.review(candidate_id, decision)
    }

    fn review(
        &self,
        candidate_id: &str,
        decision: CandidateDecision,
    ) -> Result<MemoryCandidateRecord, MemoryError> {
        let candidate_id = candidate_id.trim();
        if candidate_id.is_empty() {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "candidateId must not be blank",
            ));
        }
        let candidate = self
            .repository
            .get_candidate(candidate_id)?
            .ok_or_else(|| {
                MemoryError::coded(
                    "MEM_CANDIDATE_NOT_FOUND",
                    format!("candidate {candidate_id} was not found"),
                )
            })?;
        if candidate.status != CANDIDATE_STATUS_PENDING {
            return Err(MemoryError::coded(
                "MEM_CANDIDATE_ALREADY_REVIEWED",
                format!("candidate is already {}", candidate.status),
            ));
        }

        if matches!(decision, CandidateDecision::Accept) {
            let operation = parse_operation(&candidate.operation)?;
            let draft = CandidateDraft {
                operation,
                target_memory_id: candidate.target_memory_id.clone(),
                scope: scope_from_columns(&candidate.scope_type, candidate.scope_id.as_deref())?,
                memory_type: MemoryType::parse(&candidate.memory_type)?,
                content: candidate.content.clone(),
                reason: candidate.reason.clone(),
                confidence: candidate.confidence,
                source_type: MemorySourceType::Model,
                source_turn_id: candidate.source_turn_id.clone(),
            };
            self.apply_candidate(
                operation,
                candidate.target_memory_id.as_deref(),
                &draft,
                now_ms(),
            )?;
        }

        self.repository.append(MemoryEventKind::CandidateReviewed(
            crate::storage::memory_repository::CandidateReview {
                id: candidate_id.to_owned(),
                status: decision.status().to_owned(),
            },
        ))?;
        self.repository
            .get_candidate(candidate_id)?
            .ok_or_else(|| MemoryError::Storage("the review was not projected".into()))
    }

    /// Applies an accepted candidate. Deletion is a status change; everything else is an upsert that
    /// increments the revision of the resolved target.
    fn apply_candidate(
        &self,
        operation: MemoryOperation,
        target_memory_id: Option<&str>,
        draft: &CandidateDraft,
        now: u64,
    ) -> Result<MemoryRecord, MemoryError> {
        if matches!(operation, MemoryOperation::Delete) {
            let id = target_memory_id.ok_or_else(|| {
                MemoryError::coded("MEM_NOT_FOUND", "a delete candidate needs a target memory")
            })?;
            return self.mark_deleted(id);
        }

        let settings = self.settings()?;
        let content = draft.content.trim().to_owned();
        validate_content(&content)?;
        let sensitivity = draft.detected_sensitivity();
        if sensitivity == Sensitivity::SecretCandidate {
            return Err(MemoryError::coded(
                "MEM_SECRET_REJECTED",
                "candidate content looks like a credential; keep it in the operating system credential slot",
            ));
        }
        let normalized_key = normalize_memory_key(&content);
        if normalized_key.is_empty() {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "content must contain at least one non-whitespace character",
            ));
        }
        let (id, revision, created_at_ms) = match target_memory_id {
            Some(id) => {
                let target = self.repository.get(id.trim())?.ok_or_else(|| {
                    MemoryError::coded("MEM_NOT_FOUND", format!("memory {id} was not found"))
                })?;
                (
                    target.id,
                    target.revision.saturating_add(1),
                    target.created_at_ms,
                )
            }
            None => (Uuid::new_v4().to_string(), 1, now),
        };
        let expires_at_ms =
            effective_expiry(draft.memory_type, None, now, settings.default_ttl_days)?;
        let write = MemoryWrite {
            id: id.clone(),
            scope_type: draft.scope.kind.as_str().to_owned(),
            scope_id: draft.scope.id.clone(),
            memory_type: draft.memory_type.as_str().to_owned(),
            normalized_key,
            content,
            source_type: draft.source_type.as_str().to_owned(),
            source_ref: draft.source_turn_id.clone(),
            confidence: draft.confidence,
            sensitivity: sensitivity.as_str().to_owned(),
            status: MemoryStatus::Active.as_str().to_owned(),
            revision,
            expires_at_ms,
            created_at_ms,
        };
        self.repository
            .append(MemoryEventKind::MemoryUpserted(write))?;
        self.projected(&id)
    }

    /// Soft-deletes one memory. The confirmation token must match the memory id, mirroring the
    /// knowledge source and collection deletion contract.
    pub fn delete(&self, memory_id: &str, confirmation: &str) -> Result<MemoryRecord, MemoryError> {
        let memory_id = memory_id.trim();
        if memory_id.is_empty() {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "memoryId must not be blank",
            ));
        }
        require_confirmation(memory_id, confirmation)?;
        let memory = self.repository.get(memory_id)?.ok_or_else(|| {
            MemoryError::coded("MEM_NOT_FOUND", format!("memory {memory_id} was not found"))
        })?;
        if memory.status == MemoryStatus::Deleted.as_str() {
            // Already deleted: report the same state without writing a second audit event.
            return Ok(memory);
        }
        self.mark_deleted(memory_id)
    }

    fn mark_deleted(&self, memory_id: &str) -> Result<MemoryRecord, MemoryError> {
        self.repository
            .append(MemoryEventKind::MemoryStatusChanged(MemoryStatusChange {
                id: memory_id.to_owned(),
                status: MemoryStatus::Deleted.as_str().to_owned(),
            }))?;
        self.projected(memory_id)
    }

    /// Clears every active memory in one scope.
    ///
    /// Each memory gets its own `memory_status_changed` fact, so a clear is auditable per row rather
    /// than as a single opaque operation. Knowledge retrieval events and feedback live in separate
    /// tables and are deliberately untouched: the design requires memory and learning data to be
    /// clearable independently.
    pub fn clear(
        &self,
        scope: &MemoryScope,
        confirmation: &str,
    ) -> Result<MemoryClearOutcome, MemoryError> {
        scope.validate()?;
        require_confirmation(&scope_confirmation_token(scope), confirmation)?;
        let mut cleared = 0u64;
        let mut cursor: Option<(u64, String)> = None;
        loop {
            let page = self.repository.list_page(
                scope.kind.as_str(),
                scope.id.as_deref(),
                MemoryStatus::Active.as_str(),
                cursor.as_ref().map(|(ts, id)| (*ts, id.as_str())),
                MAX_MEMORY_PAGE_SIZE,
            )?;
            if page.is_empty() {
                break;
            }
            let exhausted = page.len() < MAX_MEMORY_PAGE_SIZE as usize;
            let last = page
                .last()
                .map(|record| (record.updated_at_ms, record.id.clone()));
            for record in page {
                self.mark_deleted(&record.id)?;
                cleared += 1;
                if cleared >= MAX_MEMORY_CLEAR_COUNT {
                    break;
                }
            }
            if exhausted || cleared >= MAX_MEMORY_CLEAR_COUNT {
                break;
            }
            cursor = last;
        }
        Ok(MemoryClearOutcome {
            scope: scope.canonical(),
            cleared_count: cleared,
        })
    }

    fn projected(&self, id: &str) -> Result<MemoryRecord, MemoryError> {
        self.repository
            .get(id)?
            .ok_or_else(|| MemoryError::Storage(format!("memory {id} was not projected")))
    }

    /// One projected row, or `None`. Read-only and status-agnostic, so callers can inspect a memory
    /// that has been archived or expired without going through the repository themselves.
    pub fn get(&self, memory_id: &str) -> Result<Option<MemoryRecord>, MemoryError> {
        let memory_id = memory_id.trim();
        if memory_id.is_empty() {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "memoryId must not be blank",
            ));
        }
        Ok(self.repository.get(memory_id)?)
    }

    /// Bounded, newest-first snapshot of active memories across every scope.
    ///
    /// This is what feeds `build_maintenance_prompt`. It is deliberately capped by the caller: the
    /// maintenance prompt has a fixed shape, and handing a model an unbounded projection would make
    /// the request size depend on how long the user has been using the app.
    pub fn maintenance_input(&self, limit: u32) -> Result<Vec<MemoryRecord>, MemoryError> {
        Ok(self.repository.list_recent_active(limit)?)
    }

    /// Offline maintenance step 1: mark every active memory whose lifetime has run out as `expired`.
    ///
    /// Expiry is a status change, never a delete, so the row stays auditable and a rebuild
    /// reproduces the same state. The lifetime rule itself lives in `memory::policy` and is shared
    /// with the context assembler, so a row cannot be un-injectable but still "active".
    ///
    /// Returns the ids that changed. Already-expired rows are skipped, so a second run is a no-op.
    pub fn expire_due(&self, now_ms: u64) -> Result<Vec<String>, MemoryError> {
        let mut expired = Vec::new();
        let mut after_id: Option<String> = None;
        loop {
            let page = self
                .repository
                .list_active_by_id(after_id.as_deref(), MAX_MEMORY_PAGE_SIZE)?;
            if page.is_empty() {
                break;
            }
            let exhausted = page.len() < MAX_MEMORY_PAGE_SIZE as usize;
            after_id = page.last().map(|record| record.id.clone());
            for record in page {
                if !memory_is_expired(&record, now_ms) {
                    continue;
                }
                self.repository
                    .append(MemoryEventKind::MemoryStatusChanged(MemoryStatusChange {
                        id: record.id.clone(),
                        status: MemoryStatus::Expired.as_str().to_owned(),
                    }))?;
                expired.push(record.id);
                if expired.len() >= MAX_MEMORY_MAINTENANCE_SWEEP {
                    break;
                }
            }
            if exhausted || expired.len() >= MAX_MEMORY_MAINTENANCE_SWEEP {
                break;
            }
        }
        expired.sort();
        Ok(expired)
    }

    /// Offline maintenance step 2: collapse duplicate deduplication keys inside one scope.
    ///
    /// A healthy projection holds one row per `(scope, normalized_key)`, but the Task 1 backfill
    /// mapped legacy rows by id, so two legacy memories with the same normalized content can coexist.
    /// The winner is the newest `updated_at_ms`, then the highest revision, then the lowest id — all
    /// host-computed and stable, so two runs over the same projection archive the same rows. Losers
    /// are `archived` rather than deleted.
    pub fn merge_duplicate_keys(&self) -> Result<Vec<MergedKeyGroup>, MemoryError> {
        let mut merged = Vec::new();
        for group in self.repository.duplicate_key_groups(MAX_MEMORY_PAGE_SIZE)? {
            if merged.len() >= MAX_MEMORY_MAINTENANCE_SWEEP {
                break;
            }
            let rows = self.repository.list_active_by_key_in_scope(
                &group.scope_type,
                group.scope_id.as_deref(),
                &group.normalized_key,
                MAX_MEMORY_PAGE_SIZE,
            )?;
            let mut rows = rows;
            rows.sort_by(|left, right| {
                right
                    .updated_at_ms
                    .cmp(&left.updated_at_ms)
                    .then_with(|| right.revision.cmp(&left.revision))
                    .then_with(|| left.id.cmp(&right.id))
            });
            let Some(kept) = rows.first().cloned() else {
                continue;
            };
            let mut archived_ids = Vec::new();
            for loser in rows.iter().skip(1) {
                self.repository
                    .append(MemoryEventKind::MemoryStatusChanged(MemoryStatusChange {
                        id: loser.id.clone(),
                        status: MemoryStatus::Archived.as_str().to_owned(),
                    }))?;
                archived_ids.push(loser.id.clone());
            }
            if archived_ids.is_empty() {
                continue;
            }
            merged.push(MergedKeyGroup {
                scope: match group.scope_id.as_deref() {
                    Some(id) => format!("{}:{id}", group.scope_type),
                    None => group.scope_type.clone(),
                },
                normalized_key: group.normalized_key.clone(),
                kept_id: kept.id,
                archived_ids,
            });
        }
        Ok(merged)
    }
}

fn validate_content(content: &str) -> Result<(), MemoryError> {
    let length = content.chars().count();
    if length == 0 || length > MAX_MEMORY_CONTENT_CHARS {
        return Err(MemoryError::coded(
            "MEM_INVALID_ARGUMENT",
            format!("content must contain 1 to {MAX_MEMORY_CONTENT_CHARS} characters"),
        ));
    }
    Ok(())
}

fn parse_operation(value: &str) -> Result<MemoryOperation, MemoryError> {
    match value {
        "create" => Ok(MemoryOperation::Create),
        "update" => Ok(MemoryOperation::Update),
        "merge" => Ok(MemoryOperation::Merge),
        "delete" => Ok(MemoryOperation::Delete),
        other => Err(MemoryError::coded(
            "MEM_INVALID_DATA",
            format!("unknown memory operation {other}"),
        )),
    }
}

/// Rebuilds a scope from the projected columns. Used when reviewing a stored candidate.
fn scope_from_columns(
    scope_type: &str,
    scope_id: Option<&str>,
) -> Result<MemoryScope, MemoryError> {
    let kind = MemoryScopeKind::parse(scope_type).ok_or_else(|| {
        MemoryError::coded(
            "MEM_INVALID_SCOPE",
            format!("unknown scope type {scope_type}"),
        )
    })?;
    let scope = MemoryScope::new(kind, scope_id.map(str::to_owned));
    scope.validate()?;
    Ok(scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::memory_repository::{MEMORY_SCOPE_USER, MEMORY_STATUS_ACTIVE};

    fn service(data_root: &std::path::Path) -> MemoryService {
        MemoryService::new(ProjectionDb::open(data_root).unwrap(), false)
    }

    fn project_scope() -> MemoryScope {
        MemoryScope::new(MemoryScopeKind::Project, Some("proj-1".into()))
    }

    fn command(scope: MemoryScope, memory_type: MemoryType, content: &str) -> UpsertMemoryCommand {
        UpsertMemoryCommand {
            memory_id: None,
            content: content.to_owned(),
            memory_type,
            scope,
            expires_at_ms: None,
        }
    }

    fn fact_log(data_root: &std::path::Path) -> String {
        std::fs::read_to_string(data_root.join("memory/events.jsonl")).unwrap_or_default()
    }

    #[test]
    fn upserting_a_new_memory_writes_a_fact_event_and_projects_it() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let outcome = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Preference,
                "  Use pnpm for scripts  ",
            ))
            .unwrap();

        assert!(!outcome.deduplicated);
        assert_eq!(outcome.memory.content, "Use pnpm for scripts");
        assert_eq!(outcome.memory.scope_type, MEMORY_SCOPE_USER);
        assert_eq!(outcome.memory.scope_id, None);
        assert_eq!(outcome.memory.memory_type, "preference");
        assert_eq!(outcome.memory.source_type, "user");
        assert_eq!(outcome.memory.sensitivity, "normal");
        assert_eq!(outcome.memory.status, MEMORY_STATUS_ACTIVE);
        assert_eq!(outcome.memory.revision, 1);
        assert_eq!(outcome.memory.expires_at_ms, None);

        let log = fact_log(data_root.path());
        assert_eq!(log.matches("memory_upserted").count(), 1);

        let page = service
            .list(&MemoryScope::user(), MemoryStatus::Active, None, None)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total, 1);
        assert_eq!(page.scope, "user");
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn upserting_identical_content_is_deduplicated_without_a_new_revision() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let first = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                "Prefer pnpm",
            ))
            .unwrap();
        let second = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                "  Prefer pnpm  ",
            ))
            .unwrap();

        assert!(second.deduplicated);
        assert_eq!(second.memory.id, first.memory.id);
        assert_eq!(second.memory.revision, 1);
        assert_eq!(
            fact_log(data_root.path())
                .matches("memory_upserted")
                .count(),
            1
        );
    }

    #[test]
    fn the_deduplication_key_decides_identity_not_the_exact_text() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let first = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                "Prefer pnpm",
            ))
            .unwrap();

        // Case and whitespace collapse into one key, so this is the same memory re-stated with
        // different formatting rather than a second memory.
        let restated = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                "  prefer   PNPM ",
            ))
            .unwrap();
        assert!(!restated.deduplicated);
        assert_eq!(restated.memory.id, first.memory.id);
        assert_eq!(restated.memory.revision, 2);

        // A genuinely different fact gets its own row.
        let different = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                "Prefer pnpm workspaces",
            ))
            .unwrap();
        assert_ne!(different.memory.id, first.memory.id);
        assert_eq!(different.memory.revision, 1);
        assert_eq!(
            service
                .list(&MemoryScope::user(), MemoryStatus::Active, None, None)
                .unwrap()
                .total,
            2
        );
    }

    #[test]
    fn upserting_changed_content_for_the_same_key_increments_the_revision() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let first = service
            .upsert(command(MemoryScope::user(), MemoryType::Fact, "Use pnpm"))
            .unwrap();
        // Same deduplication key (case and whitespace collapse), different stored content.
        let second = service
            .upsert(command(MemoryScope::user(), MemoryType::Fact, "use   pnpm"))
            .unwrap();

        assert!(!second.deduplicated);
        assert_eq!(second.memory.id, first.memory.id);
        assert_eq!(second.memory.revision, 2);
        assert_eq!(
            second.memory.created_at_ms, first.memory.created_at_ms,
            "an update must not rewrite the original creation time"
        );
        assert_eq!(second.memory.content, "use   pnpm");
    }

    #[test]
    fn an_explicit_memory_id_must_exist_and_match_the_scope() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let created = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                "Prefer pnpm",
            ))
            .unwrap();

        let mut missing = command(MemoryScope::user(), MemoryType::Fact, "Prefer pnpm");
        missing.memory_id = Some("missing-memory".into());
        assert_eq!(service.upsert(missing).unwrap_err().code(), "MEM_NOT_FOUND");

        let mut wrong_scope = command(project_scope(), MemoryType::Fact, "Prefer pnpm");
        wrong_scope.memory_id = Some(created.memory.id.clone());
        assert_eq!(
            service.upsert(wrong_scope).unwrap_err().code(),
            "MEM_INVALID_SCOPE"
        );

        let mut blank = command(MemoryScope::user(), MemoryType::Fact, "Prefer pnpm");
        blank.memory_id = Some("   ".into());
        assert_eq!(
            service.upsert(blank).unwrap_err().code(),
            "MEM_INVALID_ARGUMENT"
        );
    }

    #[test]
    fn secret_content_is_rejected_before_any_row_is_written() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let error = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                "API_KEY=sk-live-abcdefghijklmnop",
            ))
            .unwrap_err();
        assert_eq!(error.code(), "MEM_SECRET_REJECTED");
        assert_eq!(
            service
                .list(&MemoryScope::user(), MemoryStatus::Active, None, None)
                .unwrap()
                .total,
            0
        );
        assert!(!fact_log(data_root.path()).contains("memory_upserted"));
    }

    #[test]
    fn private_content_is_stored_with_the_host_detected_sensitivity() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let outcome = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                r"the repository lives at D:\code\k-coder",
            ))
            .unwrap();
        assert_eq!(outcome.memory.sensitivity, "private");
    }

    #[test]
    fn invalid_scope_type_and_expiry_fail_without_writing() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());

        let mut bad_scope = command(MemoryScope::user(), MemoryType::Fact, "content");
        bad_scope.scope = MemoryScope::new(MemoryScopeKind::Project, None);
        assert_eq!(
            service.upsert(bad_scope).unwrap_err().code(),
            "MEM_INVALID_SCOPE"
        );

        let mut blank = command(MemoryScope::user(), MemoryType::Fact, "   ");
        assert_eq!(
            service.upsert(blank.clone()).unwrap_err().code(),
            "MEM_INVALID_ARGUMENT"
        );
        blank.content = "a".repeat(MAX_MEMORY_CONTENT_CHARS + 1);
        assert_eq!(
            service.upsert(blank).unwrap_err().code(),
            "MEM_INVALID_ARGUMENT"
        );

        let mut past = command(MemoryScope::user(), MemoryType::Fact, "content");
        past.expires_at_ms = Some(now_ms().saturating_sub(1));
        assert_eq!(
            service.upsert(past).unwrap_err().code(),
            "MEM_INVALID_ARGUMENT"
        );

        let mut far = command(MemoryScope::user(), MemoryType::Fact, "content");
        far.expires_at_ms =
            Some(now_ms() + crate::memory::policy::days_to_ms(MAX_MEMORY_TTL_DAYS + 1));
        assert_eq!(
            service.upsert(far).unwrap_err().code(),
            "MEM_INVALID_ARGUMENT"
        );

        assert_eq!(
            service
                .list(&MemoryScope::user(), MemoryStatus::Active, None, None)
                .unwrap()
                .total,
            0
        );
        assert!(!fact_log(data_root.path()).contains("memory_upserted"));
    }

    #[test]
    fn listing_memories_pages_with_a_stable_cursor() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let mut expected = Vec::new();
        for content in ["first note", "second note", "third note"] {
            expected.push(
                service
                    .upsert(command(MemoryScope::user(), MemoryType::Fact, content))
                    .unwrap()
                    .memory
                    .id,
            );
        }

        let first = service
            .list(&MemoryScope::user(), MemoryStatus::Active, None, Some(2))
            .unwrap();
        assert_eq!(first.items.len(), 2);
        assert_eq!(first.total, 3);
        let cursor = first.next_cursor.clone().expect("a full page has a cursor");

        let second = service
            .list(
                &MemoryScope::user(),
                MemoryStatus::Active,
                Some(&cursor),
                Some(2),
            )
            .unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.total, 3);
        assert_eq!(
            second.next_cursor, None,
            "a short page is the end of the scope"
        );

        let mut seen = first
            .items
            .iter()
            .chain(second.items.iter())
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        seen.sort();
        expected.sort();
        assert_eq!(seen, expected);
    }

    #[test]
    fn an_invalid_cursor_is_rejected_instead_of_restarting_the_page() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let error = service
            .list(
                &MemoryScope::user(),
                MemoryStatus::Active,
                Some("not-a-cursor"),
                Some(2),
            )
            .unwrap_err();
        assert_eq!(error.code(), "MEM_INVALID_ARGUMENT");
    }

    #[test]
    fn deleting_a_memory_requires_the_host_confirmation_token() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let memory = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                "Temporary note",
            ))
            .unwrap()
            .memory;

        assert_eq!(
            service
                .delete(&memory.id, "wrong-token")
                .unwrap_err()
                .code(),
            "MEM_CONFIRMATION_REQUIRED"
        );
        assert_eq!(
            service.delete(&memory.id, "").unwrap_err().code(),
            "MEM_CONFIRMATION_REQUIRED"
        );
        assert_eq!(
            service
                .list(&MemoryScope::user(), MemoryStatus::Active, None, None)
                .unwrap()
                .total,
            1
        );

        let deleted = service.delete(&memory.id, &memory.id).unwrap();
        assert_eq!(deleted.status, "deleted");
    }

    #[test]
    fn deleting_a_memory_soft_deletes_it_and_keeps_the_audit_row() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let memory = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                "Temporary note",
            ))
            .unwrap()
            .memory;

        service.delete(&memory.id, &memory.id).unwrap();

        // The row survives, so the deletion stays auditable and a rebuild reproduces it.
        let page = service
            .list(&MemoryScope::user(), MemoryStatus::Deleted, None, None)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, memory.id);
        assert_eq!(
            service
                .list(&MemoryScope::user(), MemoryStatus::Active, None, None)
                .unwrap()
                .total,
            0
        );
        assert_eq!(
            fact_log(data_root.path())
                .matches("memory_status_changed")
                .count(),
            1
        );

        // Deleting again is idempotent and must not append a second audit event.
        service.delete(&memory.id, &memory.id).unwrap();
        assert_eq!(
            fact_log(data_root.path())
                .matches("memory_status_changed")
                .count(),
            1
        );
    }

    #[test]
    fn clearing_a_scope_writes_one_audit_event_per_memory_and_leaves_knowledge_data_alone() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        for content in ["note one", "note two", "note three"] {
            service
                .upsert(command(MemoryScope::user(), MemoryType::Fact, content))
                .unwrap();
        }
        let project_memory = service
            .upsert(command(project_scope(), MemoryType::Fact, "project note"))
            .unwrap()
            .memory;

        // Learning data lives in its own tables and must survive a memory clear.
        service
            .db
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO knowledge_retrieval_events(id,thread_id,turn_id,query_hash,
                       retrieval_mode,result_count,selected_citation_count,latency_ms,created_at_ms)
                     VALUES('event-1','thread-1','turn-1','abcdef','lexical',3,2,12,1)",
                    [],
                )?;
                connection.execute(
                    "INSERT INTO knowledge_feedback(id,citation_id,feedback_type,created_at_ms)
                     VALUES('feedback-1','citation-1','useful',1)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        assert_eq!(
            service.clear(&project_scope(), "user").unwrap_err().code(),
            "MEM_CONFIRMATION_REQUIRED"
        );

        let outcome = service.clear(&MemoryScope::user(), "user").unwrap();
        assert_eq!(outcome.cleared_count, 3);
        assert_eq!(outcome.scope, "user");
        assert_eq!(
            service
                .list(&MemoryScope::user(), MemoryStatus::Active, None, None)
                .unwrap()
                .total,
            0
        );
        assert_eq!(
            service
                .list(&project_scope(), MemoryStatus::Active, None, None)
                .unwrap()
                .total,
            1,
            "clearing one scope must not touch another"
        );
        assert_eq!(
            fact_log(data_root.path())
                .matches("memory_status_changed")
                .count(),
            3,
            "each cleared memory needs its own audit event"
        );

        let (events, feedback) = service
            .db
            .with_connection(|connection| {
                let events: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM knowledge_retrieval_events",
                    [],
                    |row| row.get(0),
                )?;
                let feedback: i64 =
                    connection.query_row("SELECT COUNT(*) FROM knowledge_feedback", [], |row| {
                        row.get(0)
                    })?;
                Ok((events, feedback))
            })
            .unwrap();
        assert_eq!(events, 1);
        assert_eq!(feedback, 1);
        assert_eq!(
            service
                .repository
                .get(&project_memory.id)
                .unwrap()
                .unwrap()
                .status,
            MEMORY_STATUS_ACTIVE
        );
    }

    #[test]
    fn reviewing_an_unknown_or_already_reviewed_candidate_fails() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        assert_eq!(
            service
                .review_candidate("missing", CandidateDecision::Accept)
                .unwrap_err()
                .code(),
            "MEM_CANDIDATE_NOT_FOUND"
        );
        assert_eq!(
            service
                .review_candidate("   ", CandidateDecision::Accept)
                .unwrap_err()
                .code(),
            "MEM_INVALID_ARGUMENT"
        );

        let draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            "the deployment target is staging",
            "observed in the transcript",
            0.4,
            MemoryScope::user(),
        );
        let CandidateOutcome::Pending { candidate } = service.record_candidate(draft).unwrap()
        else {
            panic!("a low-confidence candidate must wait for review");
        };
        service
            .review_candidate(&candidate.id, CandidateDecision::Accept)
            .unwrap();
        assert_eq!(
            service
                .review_candidate(&candidate.id, CandidateDecision::Reject)
                .unwrap_err()
                .code(),
            "MEM_CANDIDATE_ALREADY_REVIEWED"
        );
    }

    #[test]
    fn accepting_a_candidate_applies_it_and_rejecting_leaves_the_projection_alone() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());

        let accepted_draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            "the deployment target is staging",
            "observed in the transcript",
            0.4,
            MemoryScope::user(),
        );
        let CandidateOutcome::Pending { candidate } =
            service.record_candidate(accepted_draft).unwrap()
        else {
            panic!("a low-confidence candidate must wait for review");
        };
        assert!(candidate.requires_review);
        let reviewed = service
            .review_candidate(&candidate.id, CandidateDecision::Accept)
            .unwrap();
        assert_eq!(reviewed.status, "accepted");
        let page = service
            .list(&MemoryScope::user(), MemoryStatus::Active, None, None)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].content, "the deployment target is staging");

        let rejected_draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            "the deployment target is production",
            "a guess",
            0.4,
            MemoryScope::user(),
        );
        let CandidateOutcome::Pending { candidate } =
            service.record_candidate(rejected_draft).unwrap()
        else {
            panic!("a low-confidence candidate must wait for review");
        };
        let reviewed = service
            .review_candidate(&candidate.id, CandidateDecision::Reject)
            .unwrap();
        assert_eq!(reviewed.status, "rejected");
        assert_eq!(
            service
                .list(&MemoryScope::user(), MemoryStatus::Active, None, None)
                .unwrap()
                .total,
            1,
            "rejecting a candidate must not write a memory"
        );

        let pending = service.list_candidates("pending", None).unwrap();
        assert!(pending.is_empty());
        assert_eq!(service.list_candidates("accepted", None).unwrap().len(), 1);
        assert_eq!(service.list_candidates("rejected", None).unwrap().len(), 1);
        assert_eq!(
            service.list_candidates("bogus", None).unwrap_err().code(),
            "MEM_INVALID_ARGUMENT"
        );
    }

    #[test]
    fn a_conflicting_create_candidate_requires_review_and_targets_the_existing_memory() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let existing = service
            .upsert(command(MemoryScope::user(), MemoryType::Fact, "Use pnpm"))
            .unwrap()
            .memory;

        let draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            "use   PNPM",
            "the user restated the preference",
            1.0,
            MemoryScope::user(),
        );
        let CandidateOutcome::Pending { candidate } = service.record_candidate(draft).unwrap()
        else {
            panic!("a conflict must always be reviewed, even at full confidence");
        };
        assert!(candidate.requires_review);
        assert_eq!(
            candidate.operation, "update",
            "a colliding create is really an update"
        );
        assert_eq!(
            candidate.target_memory_id.as_deref(),
            Some(existing.id.as_str())
        );

        service
            .review_candidate(&candidate.id, CandidateDecision::Accept)
            .unwrap();
        let stored = service.repository.get(&existing.id).unwrap().unwrap();
        assert_eq!(stored.revision, 2);
        assert_eq!(stored.content, "use   PNPM");
    }

    #[test]
    fn a_cross_scope_update_candidate_requires_review() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let project_memory = service
            .upsert(command(project_scope(), MemoryType::Fact, "Use pnpm"))
            .unwrap()
            .memory;

        let mut draft = CandidateDraft::from_model(
            MemoryOperation::Update,
            MemoryType::Fact,
            "Use pnpm",
            "promote the project note to a user preference",
            1.0,
            MemoryScope::user(),
        );
        // Only a host-constructed draft can carry a target, and even then the service verifies it.
        draft.target_memory_id = Some(project_memory.id.clone());
        let CandidateOutcome::Pending { candidate } = service.record_candidate(draft).unwrap()
        else {
            panic!("a cross-scope update must always be reviewed");
        };
        assert!(candidate.requires_review);
        assert_eq!(candidate.operation, "update");
    }

    #[test]
    fn a_candidate_cannot_point_a_create_at_an_existing_memory() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let existing = service
            .upsert(command(MemoryScope::user(), MemoryType::Fact, "Use pnpm"))
            .unwrap()
            .memory;

        let mut draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            "Use pnpm",
            "trying to name a row directly",
            1.0,
            MemoryScope::user(),
        );
        draft.target_memory_id = Some(existing.id.clone());
        assert_eq!(
            service.record_candidate(draft).unwrap_err().code(),
            "MEM_INVALID_ARGUMENT"
        );
    }

    #[test]
    fn an_identical_candidate_is_deduplicated_without_writing() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let existing = service
            .upsert(command(MemoryScope::user(), MemoryType::Fact, "Use pnpm"))
            .unwrap()
            .memory;

        let draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            "  Use pnpm  ",
            "the user restated the preference",
            0.95,
            MemoryScope::user(),
        );
        let CandidateOutcome::Deduplicated { memory } = service.record_candidate(draft).unwrap()
        else {
            panic!("identical content must not create a second memory");
        };
        assert_eq!(memory.id, existing.id);
        assert_eq!(
            fact_log(data_root.path())
                .matches("memory_candidate_recorded")
                .count(),
            0,
            "a deduplicated candidate is not worth recording"
        );
    }

    #[test]
    fn auto_accept_applies_only_when_enabled_and_high_confidence() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());

        // The default is conservative: even a perfect candidate waits for review.
        let draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            "the deployment target is staging",
            "observed in the transcript",
            0.99,
            MemoryScope::user(),
        );
        assert!(matches!(
            service.record_candidate(draft).unwrap(),
            CandidateOutcome::Pending { .. }
        ));

        service.set_settings(true, true, 0).unwrap();
        let draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            "the release branch is main",
            "observed in the transcript",
            0.99,
            MemoryScope::user(),
        );
        let CandidateOutcome::AutoAccepted { memory, candidate } =
            service.record_candidate(draft).unwrap()
        else {
            panic!("a high-confidence, non-sensitive candidate should be auto-accepted");
        };
        assert_eq!(memory.content, "the release branch is main");
        assert_eq!(memory.source_type, "model");
        assert_eq!(candidate.status, "accepted");

        // A low-confidence candidate still waits, even with auto-accept on.
        let draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            "the release branch is develop",
            "a guess",
            0.3,
            MemoryScope::user(),
        );
        assert!(matches!(
            service.record_candidate(draft).unwrap(),
            CandidateOutcome::Pending { .. }
        ));

        // A sensitive candidate still waits, even at full confidence.
        let draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            r"the checkout lives at D:\work\checkout",
            "observed in the transcript",
            1.0,
            MemoryScope::user(),
        );
        assert!(matches!(
            service.record_candidate(draft).unwrap(),
            CandidateOutcome::Pending { .. }
        ));
    }

    #[test]
    fn settings_round_trip_and_reject_out_of_range_ttl() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let defaults = service.settings().unwrap();
        assert!(!defaults.enabled);
        assert!(!defaults.auto_accept_high_confidence);
        assert_eq!(defaults.default_ttl_days, 0);
        assert_eq!(defaults.schema_version, MEMORY_SETTINGS_SCHEMA_VERSION);

        let updated = service.set_settings(true, true, 30).unwrap();
        assert!(updated.enabled);
        assert!(updated.auto_accept_high_confidence);
        assert_eq!(updated.default_ttl_days, 30);
        assert_eq!(service.settings().unwrap(), updated);

        assert_eq!(
            service
                .set_settings(true, true, MAX_MEMORY_TTL_DAYS + 1)
                .unwrap_err()
                .code(),
            "MEM_INVALID_ARGUMENT"
        );
        assert_eq!(service.settings().unwrap(), updated);
    }

    #[test]
    fn the_legacy_enabled_flag_seeds_the_settings_row_only_once() {
        let data_root = tempfile::tempdir().unwrap();
        let seeded = MemoryService::new(ProjectionDb::open(data_root.path()).unwrap(), true);
        assert!(seeded.settings().unwrap().enabled);

        seeded.set_settings(false, false, 0).unwrap();

        // Reopening must not resurrect the legacy flag over an explicit user choice.
        let reopened = MemoryService::new(ProjectionDb::open(data_root.path()).unwrap(), true);
        assert!(!reopened.settings().unwrap().enabled);
    }

    #[test]
    fn default_ttl_applies_only_to_types_without_a_design_ttl() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        service.set_settings(true, false, 30).unwrap();

        let before = now_ms();
        let preference = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Preference,
                "Use pnpm",
            ))
            .unwrap()
            .memory;
        let work_state = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::WorkState,
                "mid-refactor on the memory module",
            ))
            .unwrap()
            .memory;
        let after = now_ms();

        let preference_expiry = preference
            .expires_at_ms
            .expect("the setting adds an expiry");
        assert!(preference_expiry >= before + crate::memory::policy::days_to_ms(30));
        assert!(preference_expiry <= after + crate::memory::policy::days_to_ms(30));

        let work_state_expiry = work_state
            .expires_at_ms
            .expect("working memory expires by design");
        assert!(work_state_expiry >= before + crate::memory::policy::days_to_ms(14));
        assert!(
            work_state_expiry < preference_expiry,
            "the design TTL must win over the configured default"
        );
    }

    #[test]
    fn memory_writes_survive_a_projection_rebuild() {
        let data_root = tempfile::tempdir().unwrap();
        let service = service(data_root.path());
        let memory = service
            .upsert(command(
                MemoryScope::user(),
                MemoryType::Fact,
                "Prefer pnpm",
            ))
            .unwrap()
            .memory;
        service.delete(&memory.id, &memory.id).unwrap();

        service.rebuild_projection().unwrap();

        let page = service
            .list(&MemoryScope::user(), MemoryStatus::Deleted, None, None)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, memory.id);
        assert_eq!(page.items[0].status, "deleted");
    }
}
