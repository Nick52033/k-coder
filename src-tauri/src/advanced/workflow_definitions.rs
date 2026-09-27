use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::storage::now_ms;

use super::store::{append_json_line, read_json_lines};

const MAX_WORKFLOW_NAME_CHARS: usize = 80;
const MAX_WORKFLOW_DESCRIPTION_CHARS: usize = 2_000;
const MAX_WORKFLOW_NODES: usize = 20;
const MAX_NODE_TITLE_CHARS: usize = 120;
const MAX_NODE_DESCRIPTION_CHARS: usize = 2_000;
const MAX_NODE_INSTRUCTIONS_CHARS: usize = 12_000;
const MAX_NODE_CRITERIA_CHARS: usize = 2_000;
const MAX_NODE_SKILLS: usize = 24;
const MAX_CUSTOM_WORKFLOW_BYTES: usize = 256 * 1024;
const CUSTOM_WORKFLOW_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDefinitionStatus {
    Draft,
    Published,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowNodeDraft {
    pub id: String,
    pub title: String,
    pub description: String,
    pub instructions: String,
    pub completion_criteria: String,
    pub skill_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDraftRequest {
    pub workflow_id: Option<String>,
    pub expected_revision: Option<u64>,
    pub name: String,
    pub description: String,
    pub nodes: Vec<WorkflowNodeDraft>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinitionRecord {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: WorkflowDefinitionStatus,
    pub revision: u64,
    pub nodes: Vec<WorkflowNodeDraft>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone)]
pub struct WorkflowDefinitionStore {
    root: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl WorkflowDefinitionStore {
    pub fn new(data_root: &Path) -> Self {
        Self {
            root: data_root.join("advanced").join("workflow-definitions"),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn project_scope_key(workspace: &Path) -> Result<String, String> {
        let workspace = workspace
            .canonicalize()
            .map_err(|error| format!("project workflow workspace is unavailable: {error}"))?;
        if !workspace.is_dir() {
            return Err("project workflow scope requires a workspace directory".into());
        }
        let digest = Sha256::digest(workspace.to_string_lossy().as_bytes());
        Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub fn list(&self, scope_key: &str) -> Result<Vec<WorkflowDefinitionRecord>, String> {
        validate_scope_key(scope_key)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "workflow definition lock poisoned")?;
        Ok(self
            .latest_unlocked(scope_key)?
            .into_iter()
            .filter(|record| record.status != WorkflowDefinitionStatus::Deleted)
            .collect())
    }

    pub fn published(&self, scope_key: &str) -> Result<Vec<WorkflowDefinitionRecord>, String> {
        Ok(self
            .list(scope_key)?
            .into_iter()
            .filter(|record| record.status == WorkflowDefinitionStatus::Published)
            .collect())
    }

    pub fn save_draft(
        &self,
        scope_key: &str,
        request: WorkflowDraftRequest,
    ) -> Result<WorkflowDefinitionRecord, String> {
        validate_scope_key(scope_key)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "workflow definition lock poisoned")?;
        let latest = self.latest_unlocked(scope_key)?;
        let now = now_ms();
        let (id, revision, created_at_ms) = match request.workflow_id.as_deref() {
            Some(id) => {
                validate_custom_workflow_id(id)?;
                let current = latest
                    .iter()
                    .find(|record| record.id == id)
                    .ok_or("workflow draft was not found")?;
                if current.status != WorkflowDefinitionStatus::Draft {
                    return Err(
                        "published workflows are immutable; duplicate it to make changes".into(),
                    );
                }
                if request.expected_revision != Some(current.revision) {
                    return Err("workflow draft changed; reload it before saving".into());
                }
                (
                    id.to_string(),
                    current.revision.saturating_add(1),
                    current.created_at_ms,
                )
            }
            None => {
                if request.expected_revision.is_some() {
                    return Err("a new workflow draft must not include a revision".into());
                }
                (new_custom_workflow_id(), 1, now)
            }
        };
        let record = WorkflowDefinitionRecord {
            schema_version: CUSTOM_WORKFLOW_SCHEMA_VERSION,
            id,
            name: request.name,
            description: request.description,
            status: WorkflowDefinitionStatus::Draft,
            revision,
            nodes: request.nodes,
            created_at_ms,
            updated_at_ms: now,
        };
        validate_definition(&record)?;
        self.append_unlocked(scope_key, &record)?;
        Ok(record)
    }

    pub fn publish(
        &self,
        scope_key: &str,
        workflow_id: &str,
        expected_revision: u64,
    ) -> Result<WorkflowDefinitionRecord, String> {
        validate_scope_key(scope_key)?;
        validate_custom_workflow_id(workflow_id)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "workflow definition lock poisoned")?;
        let mut record = self
            .latest_unlocked(scope_key)?
            .into_iter()
            .find(|record| record.id == workflow_id)
            .ok_or("workflow draft was not found")?;
        if record.status != WorkflowDefinitionStatus::Draft {
            return Err("only a draft workflow can be published".into());
        }
        if record.revision != expected_revision {
            return Err("workflow draft changed; reload it before publishing".into());
        }
        validate_definition(&record)?;
        record.status = WorkflowDefinitionStatus::Published;
        record.revision = record.revision.saturating_add(1);
        record.updated_at_ms = now_ms();
        self.append_unlocked(scope_key, &record)?;
        Ok(record)
    }

    pub fn duplicate_published(
        &self,
        scope_key: &str,
        workflow_id: &str,
        name: Option<&str>,
    ) -> Result<WorkflowDefinitionRecord, String> {
        validate_scope_key(scope_key)?;
        validate_custom_workflow_id(workflow_id)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "workflow definition lock poisoned")?;
        let source = self
            .latest_unlocked(scope_key)?
            .into_iter()
            .find(|record| record.id == workflow_id)
            .ok_or("workflow definition was not found")?;
        if source.status != WorkflowDefinitionStatus::Published {
            return Err("only a published workflow can be copied to a new draft".into());
        }
        let now = now_ms();
        let record = WorkflowDefinitionRecord {
            schema_version: CUSTOM_WORKFLOW_SCHEMA_VERSION,
            id: new_custom_workflow_id(),
            name: name
                .map(str::to_string)
                .unwrap_or_else(|| format!("{} 副本", source.name)),
            description: source.description,
            status: WorkflowDefinitionStatus::Draft,
            revision: 1,
            nodes: source.nodes,
            created_at_ms: now,
            updated_at_ms: now,
        };
        validate_definition(&record)?;
        self.append_unlocked(scope_key, &record)?;
        Ok(record)
    }

    pub fn delete_draft(
        &self,
        scope_key: &str,
        workflow_id: &str,
        expected_revision: u64,
    ) -> Result<(), String> {
        validate_scope_key(scope_key)?;
        validate_custom_workflow_id(workflow_id)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "workflow definition lock poisoned")?;
        let mut record = self
            .latest_unlocked(scope_key)?
            .into_iter()
            .find(|record| record.id == workflow_id)
            .ok_or("workflow draft was not found")?;
        if record.status != WorkflowDefinitionStatus::Draft {
            return Err("published workflows cannot be deleted".into());
        }
        if record.revision != expected_revision {
            return Err("workflow draft changed; reload it before deleting".into());
        }
        record.status = WorkflowDefinitionStatus::Deleted;
        record.revision = record.revision.saturating_add(1);
        record.updated_at_ms = now_ms();
        self.append_unlocked(scope_key, &record)
    }

    pub fn get_published(
        &self,
        scope_key: &str,
        workflow_id: &str,
    ) -> Result<Option<WorkflowDefinitionRecord>, String> {
        validate_scope_key(scope_key)?;
        validate_custom_workflow_id(workflow_id)?;
        Ok(self.list(scope_key)?.into_iter().find(|record| {
            record.id == workflow_id && record.status == WorkflowDefinitionStatus::Published
        }))
    }

    fn append_unlocked(
        &self,
        scope_key: &str,
        record: &WorkflowDefinitionRecord,
    ) -> Result<(), String> {
        append_json_line(&self.path(scope_key)?, record)
    }

    fn latest_unlocked(&self, scope_key: &str) -> Result<Vec<WorkflowDefinitionRecord>, String> {
        let mut latest = HashMap::<String, (usize, WorkflowDefinitionRecord)>::new();
        for (sequence, record) in
            read_json_lines::<WorkflowDefinitionRecord>(&self.path(scope_key)?)?
                .into_iter()
                .enumerate()
        {
            validate_definition(&record)?;
            if latest
                .get(&record.id)
                .is_none_or(|(_, current)| current.revision < record.revision)
            {
                latest.insert(record.id.clone(), (sequence, record));
            }
        }
        let mut latest = latest.into_values().collect::<Vec<_>>();
        latest.sort_by_key(|(sequence, _)| *sequence);
        Ok(latest.into_iter().map(|(_, record)| record).collect())
    }

    fn path(&self, scope_key: &str) -> Result<PathBuf, String> {
        validate_scope_key(scope_key)?;
        Ok(self.root.join(format!("{scope_key}.jsonl")))
    }
}

fn validate_scope_key(scope_key: &str) -> Result<(), String> {
    if scope_key.len() != 64
        || !scope_key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid project workflow scope".into());
    }
    Ok(())
}

fn new_custom_workflow_id() -> String {
    format!("custom-{}", uuid::Uuid::new_v4().simple())
}

fn validate_custom_workflow_id(id: &str) -> Result<(), String> {
    let suffix = id.strip_prefix("custom-").unwrap_or_default();
    if suffix.len() == 32
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("invalid custom workflow id".into())
    }
}

fn bounded_text(value: &str, field: &str, max_chars: usize) -> Result<String, String> {
    let value = value.trim();
    let chars = value.chars().count();
    if chars == 0 || chars > max_chars {
        return Err(format!("{field} must contain 1 to {max_chars} characters"));
    }
    Ok(value.to_string())
}

fn validate_definition(record: &WorkflowDefinitionRecord) -> Result<(), String> {
    validate_custom_workflow_id(&record.id)?;
    if record.schema_version != CUSTOM_WORKFLOW_SCHEMA_VERSION || record.revision == 0 {
        return Err("unsupported custom workflow record version".into());
    }
    bounded_text(&record.name, "workflow name", MAX_WORKFLOW_NAME_CHARS)?;
    bounded_text(
        &record.description,
        "workflow description",
        MAX_WORKFLOW_DESCRIPTION_CHARS,
    )?;
    if record.nodes.is_empty() || record.nodes.len() > MAX_WORKFLOW_NODES {
        return Err(format!(
            "workflow must contain 1 to {MAX_WORKFLOW_NODES} steps"
        ));
    }
    let mut node_ids = std::collections::HashSet::new();
    let mut total_bytes = 0usize;
    for node in &record.nodes {
        if !valid_slug(&node.id) || !node_ids.insert(node.id.as_str()) {
            return Err("workflow step IDs must be unique lower-case slugs".into());
        }
        bounded_text(&node.title, "step title", MAX_NODE_TITLE_CHARS)?;
        bounded_text(
            &node.description,
            "step description",
            MAX_NODE_DESCRIPTION_CHARS,
        )?;
        bounded_text(
            &node.instructions,
            "step instructions",
            MAX_NODE_INSTRUCTIONS_CHARS,
        )?;
        bounded_text(
            &node.completion_criteria,
            "step completion criteria",
            MAX_NODE_CRITERIA_CHARS,
        )?;
        if node.skill_ids.len() > MAX_NODE_SKILLS {
            return Err(format!(
                "a workflow step can bind at most {MAX_NODE_SKILLS} Skills"
            ));
        }
        let mut skill_ids = std::collections::HashSet::new();
        for skill_id in &node.skill_ids {
            if !valid_slug(skill_id) || !skill_ids.insert(skill_id.as_str()) {
                return Err("step Skill IDs must be unique lower-case slugs".into());
            }
        }
        total_bytes = total_bytes
            .saturating_add(node.id.len())
            .saturating_add(node.title.len())
            .saturating_add(node.description.len())
            .saturating_add(node.instructions.len())
            .saturating_add(node.completion_criteria.len())
            .saturating_add(node.skill_ids.iter().map(String::len).sum::<usize>());
    }
    total_bytes = total_bytes
        .saturating_add(record.name.len())
        .saturating_add(record.description.len());
    let serialized_bytes = serde_json::to_vec(record)
        .map_err(|error| format!("workflow definition serialization failed: {error}"))?
        .len();
    if total_bytes > MAX_CUSTOM_WORKFLOW_BYTES || serialized_bytes > MAX_CUSTOM_WORKFLOW_BYTES {
        return Err(format!(
            "workflow definition exceeds {MAX_CUSTOM_WORKFLOW_BYTES} bytes"
        ));
    }
    Ok(())
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> WorkflowNodeDraft {
        WorkflowNodeDraft {
            id: "step-one".into(),
            title: "Inspect".into(),
            description: "Inspect the project".into(),
            instructions: "Review the relevant files".into(),
            completion_criteria: "A concise inventory is ready".into(),
            skill_ids: Vec::new(),
        }
    }

    fn draft_request() -> WorkflowDraftRequest {
        WorkflowDraftRequest {
            workflow_id: None,
            expected_revision: None,
            name: "Project review".into(),
            description: "Review a project in ordered steps".into(),
            nodes: vec![node()],
        }
    }

    #[test]
    fn published_workflows_are_immutable_and_survive_store_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let scope = "a".repeat(64);
        let store = WorkflowDefinitionStore::new(directory.path());

        let draft = store.save_draft(&scope, draft_request()).unwrap();
        assert_eq!(draft.status, WorkflowDefinitionStatus::Draft);
        assert!(store.published(&scope).unwrap().is_empty());

        let published = store.publish(&scope, &draft.id, draft.revision).unwrap();
        assert_eq!(published.status, WorkflowDefinitionStatus::Published);
        assert_eq!(store.published(&scope).unwrap(), vec![published.clone()]);

        let update = WorkflowDraftRequest {
            workflow_id: Some(published.id.clone()),
            expected_revision: Some(published.revision),
            ..draft_request()
        };
        assert!(store.save_draft(&scope, update).is_err());

        let reopened = WorkflowDefinitionStore::new(directory.path());
        assert_eq!(reopened.published(&scope).unwrap(), vec![published]);
    }

    #[test]
    fn a_published_workflow_can_be_copied_into_a_new_draft() {
        let directory = tempfile::tempdir().unwrap();
        let scope = "d".repeat(64);
        let store = WorkflowDefinitionStore::new(directory.path());
        let draft = store.save_draft(&scope, draft_request()).unwrap();
        let published = store.publish(&scope, &draft.id, draft.revision).unwrap();
        let copy = store
            .duplicate_published(&scope, &published.id, None)
            .unwrap();

        assert_ne!(copy.id, published.id);
        assert_eq!(copy.status, WorkflowDefinitionStatus::Draft);
        assert_eq!(copy.nodes, published.nodes);
        assert_eq!(copy.name, "Project review 副本");
    }

    #[test]
    fn saving_a_draft_requires_a_valid_scope_and_nonempty_ordered_steps() {
        let directory = tempfile::tempdir().unwrap();
        let store = WorkflowDefinitionStore::new(directory.path());
        assert!(store.save_draft("../outside", draft_request()).is_err());

        let mut invalid = draft_request();
        invalid.nodes.clear();
        assert!(store.save_draft(&"b".repeat(64), invalid).is_err());

        let mut invalid = draft_request();
        invalid.nodes[0].instructions.clear();
        assert!(store.save_draft(&"c".repeat(64), invalid).is_err());
    }

    #[test]
    fn serialized_workflow_size_is_bounded_after_json_escaping() {
        let directory = tempfile::tempdir().unwrap();
        let scope = "e".repeat(64);
        let mut request = draft_request();
        request.nodes = (0..5)
            .map(|index| WorkflowNodeDraft {
                id: format!("step-{index}"),
                instructions: std::iter::repeat_n('\0', 12_000).collect(),
                ..node()
            })
            .collect();
        let error = WorkflowDefinitionStore::new(directory.path())
            .save_draft(&scope, request)
            .unwrap_err();
        assert!(error.contains("exceeds"));
    }
}
