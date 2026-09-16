//! Memory domain: entities, candidates, policy and the user-mediated service.
//!
//! Task 2 of the knowledge and memory extension. `storage::memory_repository` owns persistence —
//! the append-only fact log and the SQLite projection — while this module owns the domain rules the
//! design document assigns to `memory`: scope validation, default TTL, sensitivity grading,
//! deduplication, conflict handling, review decisions and confirmation tokens.
//!
//! Two boundaries are deliberate:
//!
//! * No SQL lives here. Every read and write goes through `MemoryRepository`, so the "commands must
//!   not run SQL" and "UI must not touch the database" rules hold by construction.
//! * No model input is trusted. A model-originated draft cannot carry a memory id, a scope, a
//!   sensitivity level, a timestamp or a deletion token; the host resolves all of them and can only
//!   raise the sensitivity level, never lower it.

pub mod candidate;
pub mod entity;
pub mod maintenance;
pub mod policy;
pub mod service;

pub use candidate::{CandidateDecision, CandidateDraft, CandidateOutcome, ConflictKind};
pub use entity::{
    MEMORY_OPERATIONS, MEMORY_SCOPE_KINDS, MEMORY_SENSITIVITIES, MEMORY_SOURCE_TYPES,
    MEMORY_STATUSES, MEMORY_TYPES, MemoryCursor, MemoryOperation, MemoryScope, MemoryScopeKind,
    MemorySourceType, MemoryStatus, MemoryType, Sensitivity,
};
pub use maintenance::{
    DEFAULT_DREAM_TOKEN_BUDGET, DEFAULT_IDLE_AFTER_MS, DEFAULT_MAINTENANCE_INTERVAL_MS,
    DreamReport, DreamStatus, MAX_MAINTENANCE_INPUT_MEMORIES, MaintenanceLease, MaintenanceOutcome,
    MaintenanceReport, MaintenanceSettings, MaintenanceTrigger, MemoryMaintenanceService,
    OfflineMaintenanceReport, bound_failure, build_maintenance_prompt, parse_proposals,
    run_offline_maintenance,
};
pub use policy::{
    AUTO_ACCEPT_CONFIDENCE, DEFAULT_EXPERIENCE_TTL_DAYS, DEFAULT_WORK_STATE_TTL_DAYS,
    MAX_MEMORY_TTL_DAYS, detect_sensitivity, effective_expiry, memory_is_expired,
    scope_confirmation_token,
};
pub use service::{
    MemoryClearOutcome, MemoryPage, MemoryService, MemorySettings, MemoryUpsertOutcome,
    MergedKeyGroup, UpsertMemoryCommand,
};

use crate::persistence::ProjectionError;

/// Coded domain error. Codes are stable, machine-readable and safe to surface to the UI, mirroring
/// the `KnowledgeError` convention.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MemoryError {
    #[error("{code}: {message}")]
    Coded { code: &'static str, message: String },
    #[error("memory storage failed: {0}")]
    Storage(String),
}

impl MemoryError {
    pub fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Coded { code, .. } => code,
            Self::Storage(_) => "MEM_STORAGE",
        }
    }
}

impl From<ProjectionError> for MemoryError {
    fn from(error: ProjectionError) -> Self {
        match error {
            // A rejected payload is a caller mistake, not an infrastructure failure, so it keeps a
            // dedicated code the UI can map to a field-level message.
            ProjectionError::InvalidData(message) => Self::coded("MEM_INVALID_DATA", message),
            other => Self::Storage(other.to_string()),
        }
    }
}
