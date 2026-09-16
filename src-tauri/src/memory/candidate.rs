//! Memory candidates: proposals the host decides whether to apply.
//!
//! A candidate is the only channel through which a model can influence memory. It carries no id, no
//! scope, no sensitivity level, no timestamp and no confirmation token — [`CandidateDraft::from_model`]
//! cannot express them — so "the model proposes, the host decides" holds structurally rather than by
//! convention.

use serde::{Deserialize, Serialize};

use crate::memory::policy::{AUTO_ACCEPT_CONFIDENCE, detect_sensitivity};
use crate::memory::{
    MemoryError, MemoryOperation, MemoryScope, MemorySourceType, MemoryType, Sensitivity,
};
use crate::storage::memory_repository::{
    MAX_MEMORY_CONTENT_CHARS, MAX_MEMORY_REASON_CHARS, MemoryCandidateRecord, MemoryRecord,
};

pub const CANDIDATE_STATUSES: [&str; 3] = ["pending", "accepted", "rejected"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateDecision {
    Accept,
    Reject,
}

impl CandidateDecision {
    pub fn parse(value: &str) -> Result<Self, MemoryError> {
        match value.trim() {
            "accept" => Ok(Self::Accept),
            "reject" => Ok(Self::Reject),
            _ => Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "decision must be accept or reject",
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
        }
    }

    /// The stored candidate status a decision produces.
    pub fn status(self) -> &'static str {
        match self {
            Self::Accept => "accepted",
            Self::Reject => "rejected",
        }
    }
}

/// How a draft relates to what is already stored. The host computes this, never the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// No other memory shares the deduplication key in this scope.
    None,
    /// The same key and the same content already exist: nothing to do.
    Duplicate,
    /// The same key exists in this scope with different content: the draft is really an update, and
    /// the caller's belief that it was new must be reviewed by a human.
    Conflict,
    /// An update whose scope differs from the target memory's scope.
    CrossScopeUpdate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateDraft {
    pub operation: MemoryOperation,
    pub target_memory_id: Option<String>,
    pub scope: MemoryScope,
    pub memory_type: MemoryType,
    pub content: String,
    pub reason: String,
    pub confidence: f64,
    pub source_type: MemorySourceType,
    pub source_turn_id: Option<String>,
}

impl CandidateDraft {
    /// Builds a draft from model output.
    ///
    /// The model supplies only the operation, type, content, reason and confidence. The scope is
    /// host-resolved from the running turn, and the target id is always `None`: the service resolves
    /// it from the deduplication key, so a model cannot name a memory row.
    pub fn from_model(
        operation: MemoryOperation,
        memory_type: MemoryType,
        content: impl Into<String>,
        reason: impl Into<String>,
        confidence: f64,
        host_scope: MemoryScope,
    ) -> Self {
        Self {
            operation,
            target_memory_id: None,
            scope: host_scope,
            memory_type,
            content: content.into(),
            reason: reason.into(),
            confidence,
            source_type: MemorySourceType::Model,
            source_turn_id: None,
        }
    }

    /// Builds a draft from a user-mediated action. The user is the human authority, so an
    /// unambiguous draft is applied directly instead of queued for review.
    pub fn from_user(
        scope: MemoryScope,
        memory_type: MemoryType,
        content: impl Into<String>,
    ) -> Self {
        Self {
            operation: MemoryOperation::Create,
            target_memory_id: None,
            scope,
            memory_type,
            content: content.into(),
            reason: "created through the memory settings surface".into(),
            confidence: 1.0,
            source_type: MemorySourceType::User,
            source_turn_id: None,
        }
    }

    pub fn detected_sensitivity(&self) -> Sensitivity {
        detect_sensitivity(&self.content)
    }

    /// Design §4.2: `secret_candidate`, deletion, conflicts and cross-scope updates always need a
    /// human. High-confidence, non-sensitive creates and updates may be auto-accepted, and only when
    /// the user has opted in.
    pub fn requires_review(
        &self,
        conflict: ConflictKind,
        auto_accept_high_confidence: bool,
    ) -> bool {
        if matches!(
            self.operation,
            MemoryOperation::Delete | MemoryOperation::Merge
        ) {
            return true;
        }
        if matches!(
            conflict,
            ConflictKind::Conflict | ConflictKind::CrossScopeUpdate
        ) {
            return true;
        }
        if self.detected_sensitivity() != Sensitivity::Normal {
            return true;
        }
        if !self.confidence.is_finite() || self.confidence < AUTO_ACCEPT_CONFIDENCE {
            return true;
        }
        !auto_accept_high_confidence
    }

    pub fn validate(&self) -> Result<(), MemoryError> {
        self.scope.validate()?;
        let content_length = self.content.trim().chars().count();
        if content_length == 0 || content_length > MAX_MEMORY_CONTENT_CHARS {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                format!("content must contain 1 to {MAX_MEMORY_CONTENT_CHARS} characters"),
            ));
        }
        let reason_length = self.reason.trim().chars().count();
        if reason_length == 0 || reason_length > MAX_MEMORY_REASON_CHARS {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                format!("reason must contain 1 to {MAX_MEMORY_REASON_CHARS} characters"),
            ));
        }
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "confidence must be a finite number between 0 and 1",
            ));
        }
        // A `create` that names an existing row is a caller mistake: the service resolves the target
        // from the deduplication key so no caller can point a create at an arbitrary memory.
        if matches!(self.operation, MemoryOperation::Create) && self.target_memory_id.is_some() {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "create candidates must not carry a target memory id",
            ));
        }
        if let Some(target) = self.target_memory_id.as_deref()
            && target.trim().is_empty()
        {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "targetMemoryId must not be blank",
            ));
        }
        Ok(())
    }
}

/// Result of recording a candidate.
#[derive(Debug, Clone, PartialEq)]
pub enum CandidateOutcome {
    /// The draft matched an existing memory with identical content. Nothing was written.
    Deduplicated { memory: MemoryRecord },
    /// The draft was high-confidence and non-sensitive, so it was applied immediately. The candidate
    /// is returned already closed so the audit trail shows both the proposal and the decision.
    AutoAccepted {
        memory: MemoryRecord,
        candidate: MemoryCandidateRecord,
    },
    /// The draft is waiting for `review_memory_candidate`.
    Pending { candidate: MemoryCandidateRecord },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryScopeKind;

    fn project_scope() -> MemoryScope {
        MemoryScope::new(MemoryScopeKind::Project, Some("proj-1".into()))
    }

    fn draft(operation: MemoryOperation, content: &str, confidence: f64) -> CandidateDraft {
        CandidateDraft::from_model(
            operation,
            MemoryType::Preference,
            content,
            "the user stated a preference",
            confidence,
            project_scope(),
        )
    }

    #[test]
    fn model_drafts_cannot_carry_a_memory_id_or_a_scope() {
        let draft = CandidateDraft::from_model(
            MemoryOperation::Create,
            MemoryType::Fact,
            "the deployment target is staging",
            "observed in the transcript",
            0.9,
            project_scope(),
        );
        assert_eq!(draft.target_memory_id, None);
        assert_eq!(draft.source_type, MemorySourceType::Model);
        // The scope comes from the host-resolved argument, not from the model payload.
        assert_eq!(draft.scope.canonical(), "project:proj-1");
        assert!(draft.validate().is_ok());
    }

    #[test]
    fn a_create_candidate_that_names_a_target_is_rejected() {
        let mut draft = draft(MemoryOperation::Create, "prefer pnpm", 0.9);
        draft.target_memory_id = Some("memory-1".into());
        assert_eq!(draft.validate().unwrap_err().code(), "MEM_INVALID_ARGUMENT");

        let mut blank = draft.clone();
        blank.operation = MemoryOperation::Update;
        blank.target_memory_id = Some("   ".into());
        assert_eq!(blank.validate().unwrap_err().code(), "MEM_INVALID_ARGUMENT");
    }

    #[test]
    fn candidate_validation_rejects_empty_content_bad_confidence_and_bad_scope() {
        let mut empty = draft(MemoryOperation::Create, "  ", 0.9);
        assert_eq!(empty.validate().unwrap_err().code(), "MEM_INVALID_ARGUMENT");

        empty.content = "a".repeat(MAX_MEMORY_CONTENT_CHARS + 1);
        assert_eq!(empty.validate().unwrap_err().code(), "MEM_INVALID_ARGUMENT");

        let mut nan = draft(MemoryOperation::Create, "prefer pnpm", f64::NAN);
        assert_eq!(nan.validate().unwrap_err().code(), "MEM_INVALID_ARGUMENT");
        nan.confidence = 1.5;
        assert_eq!(nan.validate().unwrap_err().code(), "MEM_INVALID_ARGUMENT");

        let mut bad_scope = draft(MemoryOperation::Create, "prefer pnpm", 0.9);
        bad_scope.scope = MemoryScope::new(MemoryScopeKind::Project, None);
        assert_eq!(
            bad_scope.validate().unwrap_err().code(),
            "MEM_INVALID_SCOPE"
        );
    }

    #[test]
    fn sensitive_delete_and_conflict_candidates_always_require_review() {
        // Even with auto-accept enabled and full confidence.
        let secret = draft(
            MemoryOperation::Create,
            "API_KEY=sk-live-abcdefghijklmnop",
            1.0,
        );
        assert!(secret.requires_review(ConflictKind::None, true));

        let private = draft(MemoryOperation::Create, r"repo at D:\code\k-coder", 1.0);
        assert!(private.requires_review(ConflictKind::None, true));

        let delete = draft(MemoryOperation::Delete, "prefer pnpm", 1.0);
        assert!(delete.requires_review(ConflictKind::None, true));

        let merge = draft(MemoryOperation::Merge, "prefer pnpm", 1.0);
        assert!(merge.requires_review(ConflictKind::None, true));

        let conflict = draft(MemoryOperation::Update, "prefer pnpm", 1.0);
        assert!(conflict.requires_review(ConflictKind::Conflict, true));

        let cross_scope = draft(MemoryOperation::Update, "prefer pnpm", 1.0);
        assert!(cross_scope.requires_review(ConflictKind::CrossScopeUpdate, true));
    }

    #[test]
    fn high_confidence_non_sensitive_candidates_auto_accept_only_when_enabled() {
        let clean = draft(MemoryOperation::Create, "prefer pnpm workspaces", 0.95);
        assert!(!clean.requires_review(ConflictKind::None, true));
        assert!(
            clean.requires_review(ConflictKind::None, false),
            "auto-accept is opt-in, so the default must queue for review"
        );
    }

    #[test]
    fn low_confidence_candidates_require_review() {
        let low = draft(MemoryOperation::Create, "prefer pnpm workspaces", 0.5);
        assert!(low.requires_review(ConflictKind::None, true));
        let boundary = draft(
            MemoryOperation::Create,
            "prefer pnpm workspaces",
            AUTO_ACCEPT_CONFIDENCE,
        );
        assert!(!boundary.requires_review(ConflictKind::None, true));
    }

    #[test]
    fn decision_parsing_rejects_unknown_values() {
        assert_eq!(
            CandidateDecision::parse("accept").unwrap(),
            CandidateDecision::Accept
        );
        assert_eq!(
            CandidateDecision::parse(" reject ").unwrap(),
            CandidateDecision::Reject
        );
        assert_eq!(
            CandidateDecision::parse("approve").unwrap_err().code(),
            "MEM_INVALID_ARGUMENT"
        );
    }
}
