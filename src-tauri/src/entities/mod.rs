//! Structured knowledge: entities, facts and the relation read model.
//!
//! Task 6 of the knowledge and memory extension. Design §3.1 gives this module one job and one
//! prohibition:
//!
//! > `entities` | 实体、事实、关系候选及来源绑定 | **不接受无来源关系**
//!
//! Both halves are structural rather than conventional:
//!
//! * **No relation without a source.** A proposal is only accepted together with a `citationId`,
//!   and the host resolves that id through [`KnowledgeService::citation_source`] — the same turn
//!   binding and the same active-revision check the citation reader uses. A model cannot name a
//!   chunk, a revision or a collection, and it cannot cite something the running turn was never
//!   given. If the provenance cannot be resolved, there is no fact.
//! * **A model never writes an `active` fact.** [`FactCandidate::from_model`] cannot express a
//!   status, an entity id, a revision or a timestamp, and the service records every model proposal
//!   as `candidate` regardless of its confidence. `active` is only reachable through
//!   [`EntityService::review_fact`] (a human decision) or [`FactCandidate::from_user`] (a
//!   user-mediated action, where the human *is* the authority).
//!
//! Persistence stays in `storage::knowledge_entity_repository`, so — exactly like `memory` — this
//! module contains no SQL.

pub mod candidate;
pub mod normalize;
pub mod service;
pub mod tools;

pub use candidate::{
    FactCandidate, FactConflictKind, FactDecision, FactOutcome, FactSource,
    MODEL_PROPOSED_CONFIDENCE,
};
pub use normalize::{
    DEFAULT_ENTITY_TYPE, ENTITY_STATUS_ACTIVE, ENTITY_STATUS_CANDIDATE, ENTITY_STATUS_REJECTED,
    ENTITY_STATUSES, ENTITY_TYPES, MAX_ENTITY_TYPE_CHARS, normalize_entity_name,
    normalize_predicate,
};
pub use service::{
    DEFAULT_ENTITY_PAGE_SIZE, DEFAULT_RELATION_LIMIT, EntityService, MAX_RELATION_RESULTS,
    RelationQueryResult,
};
pub use tools::{
    KnowledgeRelationsTool, MAX_RELATION_NAME_CHARS, ProposeKnowledgeFactTool, entity_tool_risks,
};

use crate::knowledge::KnowledgeError;
use crate::persistence::ProjectionError;

/// Coded domain error. Codes are stable, machine-readable and safe to surface to the UI, mirroring
/// the `KnowledgeError` and `MemoryError` conventions.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum EntityError {
    #[error("{code}: {message}")]
    Coded { code: &'static str, message: String },
    #[error("entity storage failed: {0}")]
    Storage(String),
}

impl EntityError {
    pub fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Coded { code, .. } => code,
            Self::Storage(_) => "ENT_STORAGE",
        }
    }
}

impl From<ProjectionError> for EntityError {
    fn from(error: ProjectionError) -> Self {
        match error {
            ProjectionError::InvalidData(message) => Self::coded("ENT_INVALID_DATA", message),
            other => Self::Storage(other.to_string()),
        }
    }
}

impl From<KnowledgeError> for EntityError {
    /// Preserves the knowledge code instead of flattening it.
    ///
    /// An unknown, foreign-turn or stale citation is the *interesting* rejection here — `KC_*` is
    /// what tells the caller whether it cited something it was never given or something that has
    /// since been replaced — so the code travels through unchanged.
    fn from(error: KnowledgeError) -> Self {
        match error {
            KnowledgeError::Coded { code, message } => Self::Coded { code, message },
            KnowledgeError::Storage(message) => Self::Storage(message),
        }
    }
}
