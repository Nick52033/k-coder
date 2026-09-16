//! Fact candidates: the only channel a model has into the knowledge graph.
//!
//! [`FactCandidate::from_model`] cannot express a status, an entity id, a chunk id, a revision id, a
//! collection id, a timestamp or a confidence. The citation id it *does* carry is not trusted
//! either: the service resolves it through the citation table, which is turn-bound and re-checks
//! that the revision is still the live one. "No source, no relation" therefore holds structurally.

use serde::{Deserialize, Serialize};

use crate::entities::EntityError;
use crate::storage::knowledge_entity_repository::{
    MAX_ENTITY_NAME_CHARS, MAX_OBJECT_TEXT_CHARS, MAX_PREDICATE_CHARS,
};

/// Confidence recorded for a model proposal.
///
/// The model does not state one, and inventing a high number would misrepresent it as verified.
/// The value never decides whether a human reviews the fact — every model proposal is reviewed —
/// so it is only an honest annotation on the row.
pub const MODEL_PROPOSED_CONFIDENCE: f64 = 0.5;

/// Who proposed the fact. This is host-set, never model-supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactSource {
    Model,
    User,
}

/// A review decision. `Accept` is the only path to `active` for a model proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactDecision {
    Accept,
    Reject,
}

impl FactDecision {
    pub fn parse(value: &str) -> Result<Self, EntityError> {
        match value.trim() {
            "accept" => Ok(Self::Accept),
            "reject" => Ok(Self::Reject),
            _ => Err(EntityError::coded(
                "ENT_INVALID_ARGUMENT",
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
}

/// How a proposal relates to what the graph already asserts for the same subject and predicate.
///
/// The host computes this; a caller cannot claim its fact is novel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactConflictKind {
    /// Nothing is asserted for this subject and predicate.
    None,
    /// The identical relation is already `active`: nothing to write.
    Duplicate,
    /// A *different* object is already `active` for this subject and predicate. The design keeps
    /// both readings: the incoming one waits for a human, and accepting it retires the outgoing one
    /// to `disputed` instead of deleting it.
    Conflict,
}

/// A proposed relation, before the host has resolved anything.
#[derive(Debug, Clone, PartialEq)]
pub struct FactCandidate {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    /// The citation the relation was read out of. Required: this is the "no source, no relation" gate.
    pub citation_id: String,
    pub confidence: f64,
    pub source: FactSource,
}

impl FactCandidate {
    /// Builds a candidate from model output.
    ///
    /// The model supplies four strings and nothing else. It cannot name an entity, a chunk, a
    /// revision, a collection or a status, so it cannot point the graph at a row it did not discover.
    pub fn from_model(
        subject: impl Into<String>,
        predicate: impl Into<String>,
        object: impl Into<String>,
        citation_id: impl Into<String>,
    ) -> Self {
        Self {
            subject: subject.into(),
            predicate: predicate.into(),
            object: object.into(),
            citation_id: citation_id.into(),
            confidence: MODEL_PROPOSED_CONFIDENCE,
            source: FactSource::Model,
        }
    }

    /// Builds a candidate from a user-mediated action.
    ///
    /// The user is the human authority the review step exists to consult, so this draft is applied
    /// directly. It is still source-bound: a user action does not exempt a relation from needing a
    /// citation.
    pub fn from_user(
        subject: impl Into<String>,
        predicate: impl Into<String>,
        object: impl Into<String>,
        citation_id: impl Into<String>,
    ) -> Self {
        Self {
            subject: subject.into(),
            predicate: predicate.into(),
            object: object.into(),
            citation_id: citation_id.into(),
            confidence: 1.0,
            source: FactSource::User,
        }
    }

    /// A model proposal is *always* reviewed, whatever its confidence.
    ///
    /// The design allows auto-accept for memories, but for facts it is explicit — "禁止模型直接写
    /// active fact" — and a fact is a claim about the user's own code, so a wrong one is more
    /// damaging than a wrong preference. The review queue is cheap; a wrong `active` fact is not.
    pub fn requires_review(&self) -> bool {
        !matches!(self.source, FactSource::User)
    }

    pub fn validate(&self) -> Result<(), EntityError> {
        let subject_length = self.subject.trim().chars().count();
        if subject_length == 0 || subject_length > MAX_ENTITY_NAME_CHARS {
            return Err(EntityError::coded(
                "ENT_INVALID_RELATION",
                format!("subject must contain 1 to {MAX_ENTITY_NAME_CHARS} characters"),
            ));
        }
        let object_length = self.object.trim().chars().count();
        if object_length == 0 || object_length > MAX_OBJECT_TEXT_CHARS {
            return Err(EntityError::coded(
                "ENT_INVALID_RELATION",
                format!("object must contain 1 to {MAX_OBJECT_TEXT_CHARS} characters"),
            ));
        }
        if self.predicate.trim().chars().count() > MAX_PREDICATE_CHARS {
            return Err(EntityError::coded(
                "ENT_INVALID_RELATION",
                format!("predicate must contain at most {MAX_PREDICATE_CHARS} characters"),
            ));
        }
        if self.citation_id.trim().is_empty() {
            return Err(EntityError::coded(
                "ENT_CITATION_REQUIRED",
                "a fact must cite the citation it was read out of",
            ));
        }
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return Err(EntityError::coded(
                "ENT_INVALID_ARGUMENT",
                "confidence must be a finite number between 0 and 1",
            ));
        }
        Ok(())
    }
}

/// Result of proposing a fact.
#[derive(Debug, Clone, PartialEq)]
pub enum FactOutcome {
    /// The identical relation is already `active`; nothing was written.
    Deduplicated { fact: KnowledgeFactRecord },
    /// Waiting for `review_fact`. Every model proposal lands here.
    Pending { fact: KnowledgeFactRecord },
    /// The user proposed it, so it went straight to `active`.
    Activated { fact: KnowledgeFactRecord },
}

impl FactOutcome {
    pub fn status(&self) -> &'static str {
        match self {
            Self::Deduplicated { .. } => "deduplicated",
            Self::Pending { .. } => "pending",
            Self::Activated { .. } => "activated",
        }
    }

    pub fn fact(&self) -> &KnowledgeFactRecord {
        match self {
            Self::Deduplicated { fact } | Self::Pending { fact } | Self::Activated { fact } => fact,
        }
    }
}

use crate::storage::knowledge_entity_repository::KnowledgeFactRecord;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_candidate_cannot_express_status_identity_or_provenance() {
        let candidate =
            FactCandidate::from_model("MemoryService", "depends on", "store", "citation-1");
        assert_eq!(candidate.source, FactSource::Model);
        assert_eq!(candidate.confidence, MODEL_PROPOSED_CONFIDENCE);
        assert!(
            candidate.requires_review(),
            "a model proposal is reviewed regardless of confidence"
        );
        assert!(candidate.validate().is_ok());
    }

    #[test]
    fn a_user_candidate_is_applied_directly_but_still_needs_a_citation() {
        let candidate =
            FactCandidate::from_user("MemoryService", "depends_on", "store", "citation-1");
        assert!(!candidate.requires_review());
        assert_eq!(candidate.confidence, 1.0);

        let unsourced = FactCandidate::from_user("MemoryService", "depends_on", "store", "   ");
        assert_eq!(
            unsourced.validate().unwrap_err().code(),
            "ENT_CITATION_REQUIRED"
        );
    }

    #[test]
    fn validation_rejects_blank_oversized_and_non_finite_input() {
        let mut candidate = FactCandidate::from_model("a", "b", "c", "citation-1");
        candidate.subject = "  ".into();
        assert_eq!(
            candidate.validate().unwrap_err().code(),
            "ENT_INVALID_RELATION"
        );

        candidate.subject = "a".repeat(MAX_ENTITY_NAME_CHARS + 1);
        assert_eq!(
            candidate.validate().unwrap_err().code(),
            "ENT_INVALID_RELATION"
        );

        let mut empty_object = FactCandidate::from_model("a", "b", " ", "citation-1");
        assert_eq!(
            empty_object.validate().unwrap_err().code(),
            "ENT_INVALID_RELATION"
        );
        empty_object.object = "x".repeat(MAX_OBJECT_TEXT_CHARS + 1);
        assert_eq!(
            empty_object.validate().unwrap_err().code(),
            "ENT_INVALID_RELATION"
        );

        let mut bad_confidence = FactCandidate::from_model("a", "b", "c", "citation-1");
        bad_confidence.confidence = f64::NAN;
        assert_eq!(
            bad_confidence.validate().unwrap_err().code(),
            "ENT_INVALID_ARGUMENT"
        );
        bad_confidence.confidence = 1.5;
        assert_eq!(
            bad_confidence.validate().unwrap_err().code(),
            "ENT_INVALID_ARGUMENT"
        );
    }

    #[test]
    fn decision_parsing_rejects_unknown_values() {
        assert_eq!(
            FactDecision::parse(" accept ").unwrap(),
            FactDecision::Accept
        );
        assert_eq!(FactDecision::parse("reject").unwrap(), FactDecision::Reject);
        assert_eq!(
            FactDecision::parse("approve").unwrap_err().code(),
            "ENT_INVALID_ARGUMENT"
        );
        assert_eq!(FactDecision::Accept.as_str(), "accept");
    }
}
