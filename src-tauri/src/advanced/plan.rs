use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::store::{append_json_line, read_json_lines};
use crate::protocol::{PROTOCOL_VERSION, ToolDefinition, ToolResult};
use crate::storage::now_ms;
use crate::tools::{ToolContext, ToolError, ToolHandler};

const MAX_PLAN_STEPS: usize = 32;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepState {
    Pending,
    InProgress,
    Completed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub id: String,
    pub step: String,
    pub status: PlanStepState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub schema_version: u32,
    pub thread_id: String,
    pub revision: u64,
    pub steps: Vec<PlanStep>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStepInput {
    #[serde(default)]
    pub id: Option<String>,
    pub step: String,
    pub status: PlanStepState,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanUpdateRequest {
    pub thread_id: String,
    pub steps: Vec<PlanStepInput>,
}

#[derive(Clone)]
pub struct PlanStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl PlanStore {
    pub fn new(data_root: &std::path::Path) -> Result<Self, String> {
        Ok(Self {
            path: data_root.join("advanced").join("plans.jsonl"),
            lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn get(&self, thread_id: &str) -> Result<Option<PlanView>, String> {
        let _guard = self.lock.lock().map_err(|_| "plan lock poisoned")?;
        Ok(read_json_lines::<PlanView>(&self.path)?
            .into_iter()
            .filter(|plan| plan.thread_id == thread_id)
            .max_by_key(|plan| plan.revision))
    }

    pub fn update(&self, request: PlanUpdateRequest) -> Result<PlanView, String> {
        let thread_id = request.thread_id.trim();
        if thread_id.is_empty() || thread_id.len() > 128 {
            return Err("plan thread ID must contain 1 to 128 characters".into());
        }
        if request.steps.is_empty() || request.steps.len() > MAX_PLAN_STEPS {
            return Err(format!("plan must contain 1 to {MAX_PLAN_STEPS} steps"));
        }
        let active = request
            .steps
            .iter()
            .filter(|step| step.status == PlanStepState::InProgress)
            .count();
        if active > 1 {
            return Err("plan may contain at most one active step".into());
        }
        let mut ids = std::collections::HashSet::new();
        let mut steps = Vec::with_capacity(request.steps.len());
        for input in request.steps {
            let step = input.step.trim().to_string();
            if step.is_empty() || step.len() > 240 {
                return Err("each plan step must contain 1 to 240 characters".into());
            }
            let detail = input
                .detail
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            if detail.as_ref().is_some_and(|value| value.len() > 2_000) {
                return Err("plan step detail may contain at most 2000 characters".into());
            }
            let id = input
                .id
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            if !ids.insert(id.clone()) {
                return Err("plan step IDs must be unique".into());
            }
            steps.push(PlanStep {
                id,
                step,
                status: input.status,
                detail,
            });
        }

        let _guard = self.lock.lock().map_err(|_| "plan lock poisoned")?;
        let revision = read_json_lines::<PlanView>(&self.path)?
            .into_iter()
            .filter(|plan| plan.thread_id == thread_id)
            .map(|plan| plan.revision)
            .max()
            .unwrap_or(0)
            + 1;
        let view = PlanView {
            schema_version: PROTOCOL_VERSION,
            thread_id: thread_id.to_string(),
            revision,
            steps,
            updated_at_ms: now_ms(),
        };
        append_json_line(&self.path, &view)?;
        Ok(view)
    }
}

pub struct PlanTool {
    store: PlanStore,
}

impl PlanTool {
    pub fn new(store: PlanStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for PlanTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "update_plan".into(),
            description: "Create or update the visible plan for this thread. At most one step may be in progress.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "steps": { "type": "array", "minItems": 1, "maxItems": MAX_PLAN_STEPS, "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "step": { "type": "string", "minLength": 1, "maxLength": 240 },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "failed", "skipped"] },
                            "detail": { "type": "string", "maxLength": 2000 }
                        },
                        "required": ["step", "status"],
                        "additionalProperties": false
                    }}
                },
                "required": ["steps"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(
        &self,
        context: &ToolContext,
        arguments: Value,
        _cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        #[derive(Deserialize)]
        struct Arguments {
            steps: Vec<PlanStepInput>,
        }
        let arguments: Arguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let view = self
            .store
            .update(PlanUpdateRequest {
                thread_id: context.thread_id.clone(),
                steps: arguments.steps,
            })
            .map_err(ToolError::Execution)?;
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string(&view)
                .map_err(|error| ToolError::Execution(error.to_string()))?,
            metadata: json!({"revision": view.revision}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(name: &str, status: PlanStepState) -> PlanStepInput {
        PlanStepInput {
            id: None,
            step: name.into(),
            status,
            detail: None,
        }
    }

    #[test]
    fn persists_revisions_and_rejects_two_active_steps() {
        let dir = tempfile::tempdir().unwrap();
        let store = PlanStore::new(dir.path()).unwrap();
        let plan = store
            .update(PlanUpdateRequest {
                thread_id: "thread".into(),
                steps: vec![
                    step("inspect", PlanStepState::InProgress),
                    step("edit", PlanStepState::Pending),
                ],
            })
            .unwrap();
        assert_eq!(plan.revision, 1);
        assert_eq!(store.get("thread").unwrap().unwrap(), plan);
        assert!(
            store
                .update(PlanUpdateRequest {
                    thread_id: "thread".into(),
                    steps: vec![
                        step("one", PlanStepState::InProgress),
                        step("two", PlanStepState::InProgress)
                    ]
                })
                .is_err()
        );
    }
}
