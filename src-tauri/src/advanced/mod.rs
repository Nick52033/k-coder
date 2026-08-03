mod browser;
mod document;
mod evaluation;
mod goal;
mod memory;
mod metrics;
mod plan;
mod request_user_input;
mod search;
mod store;
mod todo;

use std::collections::HashMap;
use std::sync::Arc;

use crate::protocol::ToolRisk;
use crate::tools::ToolHandler;

pub use browser::{BrowserArtifact, BrowserAuditEvent, BrowserService, BrowserSettings};
pub use document::{DocumentContent, extract_document};
pub use evaluation::{EvaluationReport, run_recorded_evaluation};
pub use goal::{CreateGoalRequest, GoalState, GoalStore, GoalTransitionRequest, GoalView};
pub use memory::{MemorySettings, MemoryStore, MemoryUpsertRequest, MemoryView};
pub use metrics::{MetricsSnapshot, RuntimeMetrics};
pub use plan::{PlanStep, PlanStepState, PlanStore, PlanUpdateRequest, PlanView};
pub use request_user_input::{
    REQUEST_USER_INPUT_TOOL_NAME, RequestUserInputArgs, RequestUserInputQuestion,
    RequestUserInputTool,
};
pub use search::{RepositorySearchIndex, SearchResult};
pub use todo::{TODO_WRITE_TOOL_NAME, TodoWriteArgs, TodoWriteToolHandler};

#[derive(Clone)]
pub struct AdvancedServices {
    pub plans: PlanStore,
    pub goals: GoalStore,
    pub browser: BrowserService,
    pub memory: MemoryStore,
    pub metrics: RuntimeMetrics,
}

impl AdvancedServices {
    pub fn new(data_root: &std::path::Path) -> Result<Self, String> {
        Ok(Self {
            plans: PlanStore::new(data_root)?,
            goals: GoalStore::new(data_root)?,
            browser: BrowserService::new(data_root)?,
            memory: MemoryStore::new(data_root)?,
            metrics: RuntimeMetrics::new(data_root)?,
        })
    }

    pub fn tool_handlers(
        &self,
        workspace_root: &std::path::Path,
    ) -> (Vec<Arc<dyn ToolHandler>>, HashMap<String, ToolRisk>) {
        let search = RepositorySearchIndex::new(workspace_root.to_path_buf());
        let handlers: Vec<Arc<dyn ToolHandler>> = vec![
            Arc::new(plan::PlanTool::new(self.plans.clone())),
            Arc::new(goal::GoalTool::new(self.goals.clone())),
            Arc::new(search::SearchTool::new(search)),
            Arc::new(memory::RecallMemoryTool::new(self.memory.clone())),
            Arc::new(memory::RememberTool::new(self.memory.clone())),
            Arc::new(request_user_input::RequestUserInputTool),
            Arc::new(browser::BrowserTool::navigate(self.browser.clone())),
            Arc::new(browser::BrowserTool::click(self.browser.clone())),
            Arc::new(browser::BrowserTool::type_text(self.browser.clone())),
            Arc::new(browser::BrowserTool::snapshot(self.browser.clone())),
            Arc::new(browser::BrowserTool::screenshot(self.browser.clone())),
            Arc::new(browser::BrowserTool::close(self.browser.clone())),
            Arc::new(todo::TodoWriteToolHandler),
        ];
        let mut risks = HashMap::new();
        for handler in &handlers {
            let name = handler.definition().name;
            let risk = if name.starts_with("browser_") || name == "remember" {
                ToolRisk::External
            } else {
                ToolRisk::Read
            };
            risks.insert(name, risk);
        }
        (handlers, risks)
    }

    pub fn runtime_instructions(&self, thread_id: &str) -> Result<String, String> {
        let mut instructions = String::from(
            "[Advanced agent runtime]\nFor substantial tasks, create and maintain an explicit plan with update_plan. Keep at most one step in_progress and persist status changes as work advances. Search the repository before editing when the location is not already known. Browser and memory writes are external-risk operations and require user approval. Do not claim browser output or remembered facts without the corresponding tool result.\n",
        );
        if let Some(goal) = self.goals.current(thread_id)? {
            if goal.state == GoalState::Active {
                let token_budget = goal
                    .token_budget
                    .map(|budget| budget.saturating_sub(goal.tokens_used).to_string())
                    .unwrap_or_else(|| "unlimited".into());
                instructions.push_str(&format!(
                    "Active Goal: {}. Remaining token budget: {}. Remaining time budget: {} ms. Complete it when the objective is achieved; use update_goal with blocked and a concrete reason only when progress cannot continue.\n",
                    goal.objective.replace('\n', " "),
                    token_budget,
                    goal.time_budget_ms.saturating_sub(goal.elapsed_ms),
                ));
            }
        }
        Ok(instructions)
    }
}
