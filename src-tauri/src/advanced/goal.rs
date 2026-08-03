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

const MAX_GOAL_TIME_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    Active,
    Paused,
    Blocked,
    Completed,
    BudgetExhausted,
}

impl GoalState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::BudgetExhausted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalView {
    pub schema_version: u32,
    pub id: String,
    pub thread_id: String,
    pub objective: String,
    pub state: GoalState,
    pub token_budget: Option<u64>,
    pub tokens_used: u64,
    pub time_budget_ms: u64,
    pub elapsed_ms: u64,
    pub reason: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGoalRequest {
    pub thread_id: String,
    pub objective: String,
    #[serde(default)]
    pub token_budget: Option<u64>,
    pub time_budget_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalTransitionRequest {
    pub goal_id: String,
    pub state: GoalState,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone)]
pub struct GoalStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl GoalStore {
    pub fn new(data_root: &std::path::Path) -> Result<Self, String> {
        Ok(Self {
            path: data_root.join("advanced").join("goals.jsonl"),
            lock: Arc::new(Mutex::new(())),
        })
    }

    fn latest_unlocked(&self) -> Result<Vec<GoalView>, String> {
        let mut latest = std::collections::HashMap::<String, GoalView>::new();
        for goal in read_json_lines::<GoalView>(&self.path)? {
            if latest
                .get(&goal.id)
                .is_none_or(|current| current.revision < goal.revision)
            {
                latest.insert(goal.id.clone(), goal);
            }
        }
        Ok(latest.into_values().collect())
    }

    pub fn current(&self, thread_id: &str) -> Result<Option<GoalView>, String> {
        let _guard = self.lock.lock().map_err(|_| "goal lock poisoned")?;
        Ok(self
            .latest_unlocked()?
            .into_iter()
            .filter(|goal| goal.thread_id == thread_id)
            .max_by_key(|goal| goal.updated_at_ms))
    }

    pub fn create(&self, request: CreateGoalRequest) -> Result<GoalView, String> {
        let thread_id = request.thread_id.trim();
        let objective = request.objective.trim();
        if thread_id.is_empty() || objective.is_empty() || objective.len() > 2_000 {
            return Err(
                "goal requires a thread and an objective of at most 2000 characters".into(),
            );
        }
        if request.token_budget == Some(0) {
            return Err("goal token budget must be greater than zero when provided".into());
        }
        if request.time_budget_ms == 0 || request.time_budget_ms > MAX_GOAL_TIME_MS {
            return Err(format!(
                "goal time budget must contain 1 to {MAX_GOAL_TIME_MS} milliseconds"
            ));
        }
        let _guard = self.lock.lock().map_err(|_| "goal lock poisoned")?;
        if self
            .latest_unlocked()?
            .iter()
            .any(|goal| goal.thread_id == thread_id && !goal.state.terminal())
        {
            return Err("this thread already has an unfinished goal".into());
        }
        let now = now_ms();
        let goal = GoalView {
            schema_version: PROTOCOL_VERSION,
            id: Uuid::new_v4().to_string(),
            thread_id: thread_id.into(),
            objective: objective.into(),
            state: GoalState::Active,
            token_budget: request.token_budget,
            tokens_used: 0,
            time_budget_ms: request.time_budget_ms,
            elapsed_ms: 0,
            reason: None,
            created_at_ms: now,
            updated_at_ms: now,
            revision: 1,
        };
        append_json_line(&self.path, &goal)?;
        Ok(goal)
    }

    pub fn transition(&self, request: GoalTransitionRequest) -> Result<GoalView, String> {
        let _guard = self.lock.lock().map_err(|_| "goal lock poisoned")?;
        let mut goal = self
            .latest_unlocked()?
            .into_iter()
            .find(|goal| goal.id == request.goal_id)
            .ok_or("goal was not found")?;
        if goal.state.terminal() {
            return Err("terminal goals cannot transition".into());
        }
        let allowed = matches!(
            (goal.state, request.state),
            (
                GoalState::Active,
                GoalState::Paused | GoalState::Blocked | GoalState::Completed
            ) | (GoalState::Paused, GoalState::Active | GoalState::Completed)
                | (GoalState::Blocked, GoalState::Active | GoalState::Completed)
        );
        if !allowed {
            return Err("goal state transition is not allowed".into());
        }
        goal.state = request.state;
        goal.reason = request
            .reason
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if goal
            .reason
            .as_ref()
            .is_some_and(|value| value.len() > 2_000)
        {
            return Err("goal reason may contain at most 2000 characters".into());
        }
        goal.updated_at_ms = now_ms();
        goal.revision += 1;
        append_json_line(&self.path, &goal)?;
        Ok(goal)
    }

    pub fn turn_budget(&self, thread_id: &str) -> Result<Option<(String, Option<u64>)>, String> {
        let Some(goal) = self.current(thread_id)? else {
            return Ok(None);
        };
        match goal.state {
            GoalState::Active => Ok(Some((
                goal.id,
                goal.token_budget
                    .map(|budget| budget.saturating_sub(goal.tokens_used)),
            ))),
            GoalState::Paused => Err("goal is paused".into()),
            GoalState::Blocked => Err("goal is blocked".into()),
            GoalState::Completed => Ok(None),
            GoalState::BudgetExhausted => Err("goal budget is exhausted".into()),
        }
    }

    pub fn record_turn(
        &self,
        goal_id: &str,
        tokens: u64,
        elapsed_ms: u64,
    ) -> Result<GoalView, String> {
        let _guard = self.lock.lock().map_err(|_| "goal lock poisoned")?;
        let mut goal = self
            .latest_unlocked()?
            .into_iter()
            .find(|goal| goal.id == goal_id)
            .ok_or("goal was not found")?;
        if goal.state != GoalState::Active {
            return Ok(goal);
        }
        goal.tokens_used = goal.tokens_used.saturating_add(tokens);
        goal.elapsed_ms = goal.elapsed_ms.saturating_add(elapsed_ms);
        let token_budget_exhausted = goal
            .token_budget
            .is_some_and(|budget| goal.tokens_used >= budget);
        if token_budget_exhausted || goal.elapsed_ms >= goal.time_budget_ms {
            goal.state = GoalState::BudgetExhausted;
            goal.reason = Some(
                if token_budget_exhausted {
                    "token budget exhausted"
                } else {
                    "time budget exhausted"
                }
                .into(),
            );
        }
        goal.updated_at_ms = now_ms();
        goal.revision += 1;
        append_json_line(&self.path, &goal)?;
        Ok(goal)
    }
}

pub struct GoalTool {
    store: GoalStore,
}
impl GoalTool {
    pub fn new(store: GoalStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for GoalTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition { name: "update_goal".into(), description: "Mark the active goal blocked or completed. Budgets and pause/resume remain user-controlled.".into(), input_schema: json!({"type":"object","properties":{"state":{"type":"string","enum":["blocked","completed"]},"reason":{"type":"string","maxLength":2000}},"required":["state"],"additionalProperties":false}) }
    }
    async fn execute(
        &self,
        context: &ToolContext,
        arguments: Value,
        _cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        #[derive(Deserialize)]
        struct Arguments {
            state: GoalState,
            reason: Option<String>,
        }
        let args: Arguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        if !matches!(args.state, GoalState::Blocked | GoalState::Completed) {
            return Err(ToolError::InvalidArguments(
                "agents may only block or complete goals".into(),
            ));
        }
        let goal = self
            .store
            .current(&context.thread_id)
            .map_err(ToolError::Execution)?
            .ok_or_else(|| ToolError::Execution("this thread has no goal".into()))?;
        let updated = self
            .store
            .transition(GoalTransitionRequest {
                goal_id: goal.id,
                state: args.state,
                reason: args.reason,
            })
            .map_err(ToolError::Execution)?;
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string(&updated)
                .map_err(|error| ToolError::Execution(error.to_string()))?,
            metadata: json!({"state": updated.state}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn goal_budget_and_transitions_are_host_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path()).unwrap();
        let goal = store
            .create(CreateGoalRequest {
                thread_id: "thread".into(),
                objective: "finish phase".into(),
                token_budget: Some(100),
                time_budget_ms: 1_000,
            })
            .unwrap();
        assert_eq!(store.turn_budget("thread").unwrap().unwrap().1, Some(100));
        let exhausted = store.record_turn(&goal.id, 100, 10).unwrap();
        assert_eq!(exhausted.state, GoalState::BudgetExhausted);
        assert!(
            store
                .transition(GoalTransitionRequest {
                    goal_id: goal.id,
                    state: GoalState::Active,
                    reason: None
                })
                .is_err()
        );
    }

    #[test]
    fn goal_without_token_budget_tracks_usage_until_time_exhaustion() {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path()).unwrap();
        let goal = store
            .create(CreateGoalRequest {
                thread_id: "thread".into(),
                objective: "finish without a token cap".into(),
                token_budget: None,
                time_budget_ms: 1_000,
            })
            .unwrap();

        assert_eq!(store.turn_budget("thread").unwrap().unwrap().1, None);
        let active = store.record_turn(&goal.id, 2_500_000, 10).unwrap();
        assert_eq!(active.state, GoalState::Active);
        assert_eq!(active.tokens_used, 2_500_000);
    }

    #[test]
    fn omitted_goal_token_budget_defaults_to_unlimited() {
        let request: CreateGoalRequest = serde_json::from_value(json!({
            "threadId": "thread",
            "objective": "finish the task",
            "timeBudgetMs": 1000
        }))
        .unwrap();

        assert_eq!(request.token_budget, None);
    }
}
