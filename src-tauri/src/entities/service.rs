//! The entity and fact domain service.
//!
//! Every entry point here is host-mediated: the service resolves the citation, resolves the entity
//! identity from the host-owned normalization, decides the conflict, decides whether a human has to
//! look at it, and only then writes an event. Nothing in this module builds SQL; persistence is
//! `storage::knowledge_entity_repository`.

use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::EntityError;
use crate::entities::candidate::{
    FactCandidate, FactConflictKind, FactDecision, FactOutcome, FactSource,
};
use crate::entities::normalize::{
    DEFAULT_ENTITY_TYPE, ENTITY_STATUS_ACTIVE, ENTITY_STATUS_CANDIDATE, normalize_entity_name,
    validate_entity_name, validate_entity_type, validate_predicate,
};
use crate::knowledge::{KnowledgeService, workspace_scope_key};
use crate::storage::knowledge_entity_repository::{
    FACT_STATUS_ACTIVE, FACT_STATUS_CANDIDATE, FACT_STATUS_DISPUTED, FACT_STATUS_REJECTED,
    FACT_STATUSES, KnowledgeEntityEventKind, KnowledgeEntityRecord, KnowledgeEntityRepository,
    KnowledgeEntityWrite, KnowledgeFactCandidateRecord, KnowledgeFactRecord, KnowledgeFactWrite,
    KnowledgeRelationRecord, KnowledgeStatusChange, MAX_KNOWLEDGE_PAGE_SIZE,
};

pub const DEFAULT_ENTITY_PAGE_SIZE: u32 = 50;
pub const DEFAULT_RELATION_LIMIT: u32 = 20;
pub const MAX_RELATION_RESULTS: u32 = 100;

/// Answer to a relation query.
///
/// `subject` is separate from `relations` on purpose: "this entity is unknown" and "this entity is
/// known but nothing is asserted about it" are different answers, and collapsing them would let the
/// caller conclude a name is unknown when it is merely unconnected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationQueryResult {
    pub subject: Option<KnowledgeEntityRecord>,
    pub relations: Vec<KnowledgeRelationRecord>,
}

#[derive(Clone)]
pub struct EntityService {
    repository: KnowledgeEntityRepository,
    knowledge: KnowledgeService,
}

impl EntityService {
    pub fn new(repository: KnowledgeEntityRepository, knowledge: KnowledgeService) -> Self {
        Self {
            repository,
            knowledge,
        }
    }

    /// Proposes a relation. The only way a model can reach the graph.
    ///
    /// Order matters: the citation is resolved *first*, so a proposal that cites something unknown,
    /// something from another turn or something whose revision has been replaced is rejected before
    /// any entity is created. A rejected proposal must not leave a half-built graph behind.
    pub fn propose_fact(
        &self,
        thread_id: &str,
        turn_id: &str,
        candidate: FactCandidate,
    ) -> Result<FactOutcome, EntityError> {
        candidate.validate()?;
        // Shape first, provenance second. Both run before anything is written, and keeping the
        // order this way means a malformed predicate is reported as a malformed predicate instead
        // of being masked by a citation problem.
        let predicate = validate_predicate(&candidate.predicate)?;
        let subject_display = validate_entity_name(&candidate.subject)?;
        let object_display = validate_entity_name(&candidate.object)?;
        let source = self
            .knowledge
            .citation_source(thread_id, turn_id, &candidate.citation_id)?;
        let now = crate::storage::now_ms();
        // A model proposal stages its entities as `candidate`, so nothing it invents is visible to
        // the relation query until a human accepts the fact.
        let entity_status = match candidate.source {
            FactSource::User => ENTITY_STATUS_ACTIVE,
            FactSource::Model => ENTITY_STATUS_CANDIDATE,
        };

        let subject = self.resolve_entity(
            &source.collection_id,
            &subject_display,
            DEFAULT_ENTITY_TYPE,
            entity_status,
            candidate.confidence,
            now,
        )?;

        // The object becomes a link only when that entity already exists. A proposal must not
        // invent a second entity behind the reviewer's back, so an unknown object is stored as text
        // and starts linking once something else makes it a subject.
        let object_normalized = normalize_entity_name(&object_display);
        let object_existing = self
            .repository
            .find_entity_by_normalized_name(&source.collection_id, &object_normalized)?;
        let (object_entity_id, object_text) = match object_existing {
            Some(entity) => (Some(entity.id), None),
            None => (None, Some(object_display)),
        };

        let existing = self
            .repository
            .list_active_facts_for_subject_predicate(&subject.id, &predicate)?;
        if let Some(duplicate) = existing
            .iter()
            .find(|fact| same_object(fact, object_entity_id.as_deref(), object_text.as_deref()))
        {
            return Ok(FactOutcome::Deduplicated {
                fact: duplicate.clone(),
            });
        }
        let conflict = if existing.is_empty() {
            FactConflictKind::None
        } else {
            FactConflictKind::Conflict
        };

        let requires_review = candidate.requires_review();
        let status = if requires_review {
            FACT_STATUS_CANDIDATE
        } else {
            FACT_STATUS_ACTIVE
        };
        let fact_id = Uuid::new_v4().to_string();
        self.repository
            .append(KnowledgeEntityEventKind::FactRecorded(KnowledgeFactWrite {
                id: fact_id.clone(),
                subject_entity_id: subject.id.clone(),
                predicate: predicate.clone(),
                object_entity_id,
                object_text,
                source_chunk_id: source.chunk_id,
                source_revision_id: source.revision_id,
                confidence: candidate.confidence,
                valid_from_ms: None,
                valid_to_ms: None,
                status: status.to_owned(),
                created_at_ms: now,
            }))?;

        // The user outranks the previous assertion, so it is retired to `disputed` — never deleted,
        // because the fact that used to be true (and its citation) is part of the audit trail.
        if !requires_review && matches!(conflict, FactConflictKind::Conflict) {
            self.dispute_competing_facts(&subject.id, &predicate, None)?;
        }

        let fact = self.get_fact(&fact_id)?;
        Ok(if requires_review {
            FactOutcome::Pending { fact }
        } else {
            FactOutcome::Activated { fact }
        })
    }

    /// Applies or discards a pending fact candidate.
    ///
    /// A candidate can only be reviewed once, so a stale review queue cannot re-apply a decision the
    /// user already made.
    pub fn review_fact(
        &self,
        fact_id: &str,
        decision: FactDecision,
        entity_type: Option<&str>,
    ) -> Result<KnowledgeFactRecord, EntityError> {
        let fact_id = fact_id.trim();
        if fact_id.is_empty() {
            return Err(EntityError::coded(
                "ENT_INVALID_ARGUMENT",
                "factId must not be blank",
            ));
        }
        let fact = self.repository.get_fact(fact_id)?.ok_or_else(|| {
            EntityError::coded("ENT_NOT_FOUND", format!("fact {fact_id} was not found"))
        })?;
        if fact.status != FACT_STATUS_CANDIDATE {
            return Err(EntityError::coded(
                "ENT_ALREADY_REVIEWED",
                format!("fact is already {}", fact.status),
            ));
        }

        if matches!(decision, FactDecision::Reject) {
            self.set_fact_status(&fact.id, FACT_STATUS_REJECTED)?;
            return self.get_fact(&fact.id);
        }

        // The reviewer is the only actor allowed to type an entity, and only while that entity is
        // still a candidate — an entity an earlier decision already settled keeps its type.
        let entity_type = entity_type.map(validate_entity_type).transpose()?;
        let endpoints = [
            Some(fact.subject_entity_id.clone()),
            fact.object_entity_id.clone(),
        ];
        for entity_id in endpoints.into_iter().flatten() {
            self.activate_candidate_entity(&entity_id, entity_type.as_deref())?;
        }

        // Accepting this reading retires the competing one instead of deleting it.
        self.dispute_competing_facts(&fact.subject_entity_id, &fact.predicate, Some(&fact.id))?;
        self.set_fact_status(&fact.id, FACT_STATUS_ACTIVE)?;
        self.get_fact(&fact.id)
    }

    pub fn list_entities(
        &self,
        collection_id: &str,
        status: &str,
        limit: Option<u32>,
    ) -> Result<Vec<KnowledgeEntityRecord>, EntityError> {
        validate_entity_status(status)?;
        Ok(self
            .repository
            .list_entities(collection_id.trim(), status.trim(), clamp_page(limit))?)
    }

    pub fn list_facts(
        &self,
        collection_id: &str,
        status: &str,
        limit: Option<u32>,
    ) -> Result<Vec<KnowledgeFactCandidateRecord>, EntityError> {
        if !FACT_STATUSES.contains(&status.trim()) {
            return Err(EntityError::coded(
                "ENT_INVALID_ARGUMENT",
                format!("status must be one of {}", FACT_STATUSES.join(", ")),
            ));
        }
        Ok(self.repository.list_facts_by_status_for_collection(
            collection_id.trim(),
            status.trim(),
            clamp_page(limit),
        )?)
    }

    /// The read-only relation query behind both the tool and the settings surface.
    pub fn relations(
        &self,
        workspace: &Path,
        name: &str,
        limit: usize,
    ) -> Result<RelationQueryResult, EntityError> {
        let display = validate_entity_name(name)?;
        let normalized = normalize_entity_name(&display);
        let scope_key = workspace_scope_key(workspace)?;
        let bounded = (limit as u32).clamp(1, MAX_RELATION_RESULTS);
        let subject = self
            .repository
            .list_entities_by_normalized_name(&scope_key, &normalized, 1)?
            .into_iter()
            .next();
        let relations = self
            .repository
            .list_relations(&scope_key, &normalized, bounded)?;
        Ok(RelationQueryResult { subject, relations })
    }

    pub fn get_fact(&self, fact_id: &str) -> Result<KnowledgeFactRecord, EntityError> {
        self.repository.get_fact(fact_id)?.ok_or_else(|| {
            EntityError::coded("ENT_NOT_FOUND", format!("fact {fact_id} was not found"))
        })
    }

    pub fn get_entity(&self, entity_id: &str) -> Result<KnowledgeEntityRecord, EntityError> {
        self.repository.get_entity(entity_id)?.ok_or_else(|| {
            EntityError::coded("ENT_NOT_FOUND", format!("entity {entity_id} was not found"))
        })
    }

    /// Resolves a name to an entity, creating it when the collection has never seen that identity.
    fn resolve_entity(
        &self,
        collection_id: &str,
        name: &str,
        entity_type: &str,
        status: &str,
        confidence: f64,
        now: u64,
    ) -> Result<KnowledgeEntityRecord, EntityError> {
        let display = validate_entity_name(name)?;
        let normalized = normalize_entity_name(&display);
        if let Some(existing) = self
            .repository
            .find_entity_by_normalized_name(collection_id, &normalized)?
        {
            return Ok(existing);
        }
        let id = Uuid::new_v4().to_string();
        self.repository
            .append(KnowledgeEntityEventKind::EntityUpserted(
                KnowledgeEntityWrite {
                    id: id.clone(),
                    collection_id: collection_id.to_owned(),
                    entity_type: entity_type.to_owned(),
                    name: display,
                    normalized_name: normalized,
                    description: None,
                    confidence,
                    status: status.to_owned(),
                    created_at_ms: now,
                },
            ))?;
        self.repository.get_entity(&id)?.ok_or_else(|| {
            EntityError::Storage("the entity was not projected after it was recorded".into())
        })
    }

    /// Promotes an entity that only exists because of the fact being reviewed.
    fn activate_candidate_entity(
        &self,
        entity_id: &str,
        entity_type: Option<&str>,
    ) -> Result<(), EntityError> {
        let Some(entity) = self.repository.get_entity(entity_id)? else {
            return Ok(());
        };
        if entity.status != ENTITY_STATUS_CANDIDATE {
            return Ok(());
        }
        self.repository
            .append(KnowledgeEntityEventKind::EntityUpserted(
                KnowledgeEntityWrite {
                    id: entity.id,
                    collection_id: entity.collection_id,
                    entity_type: entity_type.unwrap_or(&entity.entity_type).to_owned(),
                    name: entity.name,
                    normalized_name: entity.normalized_name,
                    description: entity.description,
                    confidence: entity.confidence,
                    status: ENTITY_STATUS_ACTIVE.to_owned(),
                    created_at_ms: entity.created_at_ms,
                },
            ))?;
        Ok(())
    }

    /// Moves every other `active` reading of the same subject and predicate to `disputed`.
    fn dispute_competing_facts(
        &self,
        subject_entity_id: &str,
        predicate: &str,
        keep: Option<&str>,
    ) -> Result<(), EntityError> {
        for other in self
            .repository
            .list_active_facts_for_subject_predicate(subject_entity_id, predicate)?
        {
            if keep.is_some_and(|keep| keep == other.id) {
                continue;
            }
            self.set_fact_status(&other.id, FACT_STATUS_DISPUTED)?;
        }
        Ok(())
    }

    fn set_fact_status(&self, fact_id: &str, status: &str) -> Result<(), EntityError> {
        self.repository
            .append(KnowledgeEntityEventKind::FactStatusChanged(
                KnowledgeStatusChange {
                    id: fact_id.to_owned(),
                    status: status.to_owned(),
                },
            ))?;
        Ok(())
    }
}

/// Whether an existing fact already asserts the candidate's object.
fn same_object(
    fact: &KnowledgeFactRecord,
    object_entity_id: Option<&str>,
    object_text: Option<&str>,
) -> bool {
    if let (Some(existing), Some(candidate)) = (fact.object_entity_id.as_deref(), object_entity_id)
    {
        return existing == candidate;
    }
    if fact.object_entity_id.is_some() || object_entity_id.is_some() {
        return false;
    }
    match (fact.object_text.as_deref(), object_text) {
        // Compared through the host identity rule, so "Store" and "store" are one object.
        (Some(existing), Some(candidate)) => {
            normalize_entity_name(existing) == normalize_entity_name(candidate)
        }
        _ => false,
    }
}

fn clamp_page(limit: Option<u32>) -> u32 {
    limit
        .unwrap_or(DEFAULT_ENTITY_PAGE_SIZE)
        .clamp(1, MAX_KNOWLEDGE_PAGE_SIZE)
}

fn validate_entity_status(status: &str) -> Result<(), EntityError> {
    use crate::entities::normalize::ENTITY_STATUSES;
    if !ENTITY_STATUSES.contains(&status.trim()) {
        return Err(EntityError::coded(
            "ENT_INVALID_ARGUMENT",
            format!("status must be one of {}", ENTITY_STATUSES.join(", ")),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::candidate::MODEL_PROPOSED_CONFIDENCE;
    use crate::entities::normalize::ENTITY_TYPES;
    use crate::knowledge::{AddSourceRequest, UpsertCollectionRequest};
    use crate::persistence::ProjectionDb;
    use crate::providers::{CredentialError, CredentialStore};
    use std::sync::Arc;
    use std::time::Duration;

    const THREAD: &str = "thread-1";
    const TURN: &str = "turn-1";

    /// The knowledge service needs *a* credential store; the entity tests never enable semantic
    /// search, so "no credentials configured" is the honest stub.
    #[derive(Default)]
    struct NoCredentials;

    impl CredentialStore for NoCredentials {
        fn get_api_key(&self, _provider_id: &str) -> Result<Option<String>, CredentialError> {
            Ok(None)
        }
        fn set_api_key(&self, _provider_id: &str, _api_key: &str) -> Result<(), CredentialError> {
            Ok(())
        }
        fn delete_api_key(&self, _provider_id: &str) -> Result<(), CredentialError> {
            Ok(())
        }
    }

    fn services() -> (KnowledgeService, EntityService) {
        let db = ProjectionDb::memory().unwrap();
        let knowledge = KnowledgeService::new(db.clone(), Arc::new(NoCredentials));
        let entities = EntityService::new(KnowledgeEntityRepository::new(db), knowledge.clone());
        (knowledge, entities)
    }

    async fn wait_job(knowledge: &KnowledgeService, job_id: &str) {
        for _ in 0..400 {
            if let Ok(job) = knowledge.get_job(job_id)
                && !matches!(job.state.as_str(), "queued" | "running")
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("index job {job_id} never finished");
    }

    async fn index(knowledge: &KnowledgeService, root: &Path, files: &[&str]) -> String {
        knowledge.set_enabled(true).unwrap();
        let collection = knowledge
            .upsert_collection(
                root,
                UpsertCollectionRequest {
                    id: None,
                    name: "Docs".into(),
                    scope: None,
                    enabled: true,
                },
            )
            .unwrap();
        for file in files {
            let source = knowledge
                .add_source(
                    root,
                    AddSourceRequest {
                        collection_id: collection.id.clone(),
                        workspace_relative_path: (*file).into(),
                    },
                )
                .await
                .unwrap();
            let job_id = source
                .initial_job_id
                .clone()
                .expect("a fresh source always starts a job");
            wait_job(knowledge, &job_id).await;
        }
        collection.id
    }

    async fn citation(knowledge: &KnowledgeService, root: &Path, query: &str) -> String {
        let response = knowledge
            .search(root, THREAD, TURN, query, 3)
            .await
            .unwrap();
        response
            .results
            .first()
            .expect("the fixture must be searchable")
            .citation_id
            .clone()
    }

    async fn indexed_workspace() -> (tempfile::TempDir, KnowledgeService, EntityService, String) {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("guide.md"), "cargo test 完成部署").unwrap();
        let (knowledge, entities) = services();
        let collection_id = index(&knowledge, root.path(), &["guide.md"]).await;
        (root, knowledge, entities, collection_id)
    }

    #[test]
    fn the_default_entity_type_is_part_of_the_vocabulary() {
        assert!(
            ENTITY_TYPES.contains(&DEFAULT_ENTITY_TYPE),
            "the default must be a type the reviewer can also choose: {ENTITY_TYPES:?}"
        );
        assert_eq!(
            clamp_page(None),
            DEFAULT_ENTITY_PAGE_SIZE,
            "an omitted limit falls back to the default page"
        );
        assert_eq!(clamp_page(Some(0)), 1);
        assert_eq!(clamp_page(Some(10_000)), MAX_KNOWLEDGE_PAGE_SIZE);
    }

    #[test]
    fn a_same_object_comparison_uses_the_host_identity_rule() {
        let text_fact = |text: &str| KnowledgeFactRecord {
            id: "fact-1".into(),
            subject_entity_id: "entity-1".into(),
            predicate: "reveals".into(),
            object_entity_id: None,
            object_text: Some(text.to_owned()),
            source_chunk_id: "chunk-1".into(),
            source_revision_id: "revision-1".into(),
            confidence: 0.5,
            valid_from_ms: None,
            valid_to_ms: None,
            status: FACT_STATUS_ACTIVE.into(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        assert!(same_object(&text_fact("Store"), None, Some("store")));
        assert!(!same_object(&text_fact("Store"), None, Some("cache")));
        // An entity object and a text object are never the same reading.
        assert!(!same_object(&text_fact("Store"), Some("entity-2"), None));
        let entity_fact = KnowledgeFactRecord {
            object_entity_id: Some("entity-2".into()),
            object_text: None,
            ..text_fact("Store")
        };
        assert!(same_object(&entity_fact, Some("entity-2"), None));
        assert!(!same_object(&entity_fact, Some("entity-3"), None));
        assert!(!same_object(&entity_fact, None, Some("store")));
    }

    #[test]
    fn status_validation_rejects_unknown_values() {
        assert!(validate_entity_status("active").is_ok());
        assert!(validate_entity_status(" candidate ").is_ok());
        assert_eq!(
            validate_entity_status("pending").unwrap_err().code(),
            "ENT_INVALID_ARGUMENT"
        );
    }

    #[tokio::test]
    async fn a_relation_without_a_resolvable_citation_is_rejected_and_writes_nothing() {
        let (root, _knowledge, entities, collection_id) = indexed_workspace().await;

        let error = entities
            .propose_fact(
                THREAD,
                TURN,
                FactCandidate::from_model("MemoryService", "depends_on", "store", "not-a-citation"),
            )
            .unwrap_err();
        assert_eq!(error.code(), "KC_CITATION_FORBIDDEN");

        // The rejection happened before the graph was touched, so a bad proposal cannot leave a
        // half-built entity behind.
        assert!(
            entities
                .list_facts(&collection_id, "candidate", None)
                .unwrap()
                .is_empty()
        );
        assert!(
            entities
                .list_entities(&collection_id, "candidate", None)
                .unwrap()
                .is_empty()
        );
        assert!(
            entities
                .relations(root.path(), "MemoryService", 10)
                .unwrap()
                .subject
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_citation_from_another_turn_is_rejected() {
        let (root, knowledge, entities, _id) = indexed_workspace().await;
        let citation_id = citation(&knowledge, root.path(), "cargo").await;

        let error = entities
            .propose_fact(
                "another-thread",
                TURN,
                FactCandidate::from_model(
                    "MemoryService",
                    "depends_on",
                    "store",
                    citation_id.clone(),
                ),
            )
            .unwrap_err();
        assert_eq!(error.code(), "KC_CITATION_FORBIDDEN");

        // The same citation is accepted from the turn that actually received it.
        assert!(
            entities
                .propose_fact(
                    THREAD,
                    TURN,
                    FactCandidate::from_model("MemoryService", "depends_on", "store", citation_id),
                )
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_citation_whose_revision_was_replaced_is_stale() {
        let (root, knowledge, entities, collection_id) = indexed_workspace().await;
        let citation_id = citation(&knowledge, root.path(), "cargo").await;

        // Re-indexing the changed file activates a new revision and retires the old one.
        std::fs::write(
            root.path().join("guide.md"),
            "cargo test 完成部署与新的内容",
        )
        .unwrap();
        let source = knowledge
            .list_sources(root.path(), &collection_id)
            .unwrap()
            .remove(0);
        let job = knowledge
            .refresh_source(root.path(), &source.source_id)
            .await
            .unwrap();
        wait_job(&knowledge, &job.job_id).await;

        let error = entities
            .propose_fact(
                THREAD,
                TURN,
                FactCandidate::from_model("MemoryService", "depends_on", "store", citation_id),
            )
            .unwrap_err();
        assert_eq!(error.code(), "KC_CITATION_STALE");
    }

    #[tokio::test]
    async fn an_illegal_relation_is_rejected_before_the_citation_is_resolved() {
        let (_root, _knowledge, entities, _id) = indexed_workspace().await;

        // A non-ASCII predicate cannot be represented in the storage token vocabulary, so it is
        // refused instead of being transliterated into something the caller never wrote.
        let error = entities
            .propose_fact(
                THREAD,
                TURN,
                FactCandidate::from_model("MemoryService", "依赖", "store", "not-a-citation"),
            )
            .unwrap_err();
        assert_eq!(error.code(), "ENT_INVALID_RELATION");

        // Decoration-only names have no identity either.
        let error = entities
            .propose_fact(
                THREAD,
                TURN,
                FactCandidate::from_model("\"\"``", "depends_on", "store", "not-a-citation"),
            )
            .unwrap_err();
        assert_eq!(error.code(), "ENT_INVALID_RELATION");
    }

    #[tokio::test]
    async fn a_model_proposal_stays_a_candidate_until_the_user_accepts_it() {
        let (root, knowledge, entities, collection_id) = indexed_workspace().await;
        let citation_id = citation(&knowledge, root.path(), "cargo").await;

        let outcome = entities
            .propose_fact(
                THREAD,
                TURN,
                FactCandidate::from_model(
                    "  MemoryService  ",
                    "depends on",
                    "knowledge   store",
                    citation_id,
                ),
            )
            .unwrap();
        assert_eq!(outcome.status(), "pending");
        let fact = outcome.fact();
        assert_eq!(fact.status, FACT_STATUS_CANDIDATE);
        assert_eq!(
            fact.predicate, "depends_on",
            "the host normalizes the predicate"
        );
        assert_eq!(fact.object_text.as_deref(), Some("knowledge   store"));
        assert!(fact.object_entity_id.is_none());
        assert_eq!(fact.confidence, MODEL_PROPOSED_CONFIDENCE);
        assert!(!fact.source_chunk_id.is_empty() && !fact.source_revision_id.is_empty());

        // Nothing the model proposed is readable before the review.
        assert!(
            entities
                .relations(root.path(), "MemoryService", 10)
                .unwrap()
                .subject
                .is_none()
        );
        assert_eq!(
            entities
                .list_entities(&collection_id, "candidate", None)
                .unwrap()
                .len(),
            1
        );
        assert!(
            entities
                .list_facts(&collection_id, "candidate", None)
                .unwrap()
                .len()
                == 1
        );

        let reviewed = entities
            .review_fact(&fact.id, FactDecision::Accept, Some("module"))
            .unwrap();
        assert_eq!(reviewed.status, FACT_STATUS_ACTIVE);

        let result = entities
            .relations(root.path(), "MemoryService", 10)
            .unwrap();
        let subject = result
            .subject
            .expect("accepting the fact activates its entity");
        assert_eq!(subject.entity_type, "module");
        assert_eq!(result.relations.len(), 1);
        let relation = &result.relations[0];
        assert_eq!(relation.subject_name, "MemoryService");
        assert_eq!(relation.predicate, "depends_on");
        assert_eq!(relation.object_text.as_deref(), Some("knowledge   store"));
        assert!(
            relation.source_path.is_some() && relation.locator.is_some(),
            "an active relation always carries its citation provenance"
        );
    }

    #[tokio::test]
    async fn rejecting_a_candidate_leaves_the_reading_out_of_the_graph() {
        let (root, knowledge, entities, collection_id) = indexed_workspace().await;
        let citation_id = citation(&knowledge, root.path(), "cargo").await;
        let outcome = entities
            .propose_fact(
                THREAD,
                TURN,
                FactCandidate::from_model("MemoryService", "depends_on", "store", citation_id),
            )
            .unwrap();
        let fact_id = outcome.fact().id.clone();

        let reviewed = entities
            .review_fact(&fact_id, FactDecision::Reject, None)
            .unwrap();
        assert_eq!(reviewed.status, FACT_STATUS_REJECTED);
        assert!(
            entities
                .relations(root.path(), "MemoryService", 10)
                .unwrap()
                .subject
                .is_none()
        );
        assert!(
            entities
                .list_facts(&collection_id, "candidate", None)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn an_identical_proposal_is_deduplicated_and_a_competing_one_is_disputed() {
        let (root, knowledge, entities, _id) = indexed_workspace().await;
        let citation_id = citation(&knowledge, root.path(), "cargo").await;
        let propose = |object: &str| {
            entities
                .propose_fact(
                    THREAD,
                    TURN,
                    FactCandidate::from_model(
                        "MemoryService",
                        "depends_on",
                        object,
                        citation_id.clone(),
                    ),
                )
                .unwrap()
        };

        let first = propose("store A");
        let first_id = first.fact().id.clone();
        entities
            .review_fact(&first_id, FactDecision::Accept, None)
            .unwrap();

        // The same reading again is recognized, not duplicated.
        let duplicate = propose("store A");
        assert_eq!(duplicate.status(), "deduplicated");
        assert_eq!(duplicate.fact().id, first_id);

        // A different object for the same subject and predicate is a conflict.
        let competing = propose("store B");
        assert_eq!(competing.status(), "pending");
        let competing_id = competing.fact().id.clone();
        assert_eq!(
            entities.get_fact(&first_id).unwrap().status,
            FACT_STATUS_ACTIVE,
            "the standing reading is untouched while the new one waits for review"
        );

        entities
            .review_fact(&competing_id, FactDecision::Accept, None)
            .unwrap();
        assert_eq!(
            entities.get_fact(&first_id).unwrap().status,
            FACT_STATUS_DISPUTED,
            "accepting the new reading retires the old one instead of deleting it"
        );
        let relations = entities
            .relations(root.path(), "MemoryService", 10)
            .unwrap();
        assert_eq!(
            relations.relations.len(),
            1,
            "only the accepted reading is a relation"
        );
        assert_eq!(
            relations.relations[0].object_text.as_deref(),
            Some("store B")
        );
    }

    #[tokio::test]
    async fn reviewing_the_same_fact_twice_is_refused() {
        let (_root, knowledge, entities, _id) = indexed_workspace().await;
        let citation_id = citation(&knowledge, _root.path(), "cargo").await;
        let outcome = entities
            .propose_fact(
                THREAD,
                TURN,
                FactCandidate::from_model("MemoryService", "depends_on", "store", citation_id),
            )
            .unwrap();
        let fact_id = outcome.fact().id.clone();
        assert_eq!(
            entities
                .review_fact("missing", FactDecision::Accept, None)
                .unwrap_err()
                .code(),
            "ENT_NOT_FOUND"
        );
        entities
            .review_fact(&fact_id, FactDecision::Accept, None)
            .unwrap();
        assert_eq!(
            entities
                .review_fact(&fact_id, FactDecision::Reject, None)
                .unwrap_err()
                .code(),
            "ENT_ALREADY_REVIEWED"
        );
        assert_eq!(
            entities
                .review_fact("   ", FactDecision::Accept, None)
                .unwrap_err()
                .code(),
            "ENT_INVALID_ARGUMENT"
        );
        assert_eq!(
            entities
                .review_fact(&fact_id, FactDecision::Accept, Some("not-a-type"))
                .unwrap_err()
                .code(),
            "ENT_ALREADY_REVIEWED"
        );
    }

    #[tokio::test]
    async fn an_unknown_entity_name_is_not_the_same_answer_as_an_unconnected_one() {
        let (root, knowledge, entities, _id) = indexed_workspace().await;
        let citation_id = citation(&knowledge, root.path(), "cargo").await;
        let outcome = entities
            .propose_fact(
                THREAD,
                TURN,
                FactCandidate::from_model("MemoryService", "depends_on", "store", citation_id),
            )
            .unwrap();
        entities
            .review_fact(&outcome.fact().id, FactDecision::Accept, None)
            .unwrap();

        let connected = entities
            .relations(root.path(), "MemoryService", 10)
            .unwrap();
        assert!(connected.subject.is_some());
        assert_eq!(connected.relations.len(), 1);

        let unknown = entities
            .relations(root.path(), "SomethingElse", 10)
            .unwrap();
        assert!(unknown.subject.is_none());
        assert!(unknown.relations.is_empty());
    }
}
