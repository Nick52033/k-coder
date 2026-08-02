use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::protocol::{TodoItem, TodoStatus, ToolDefinition, ToolResult};
use crate::tools::{ToolContext, ToolError, ToolHandler};

pub const TODO_WRITE_TOOL_NAME: &str = "todo_write";

/// TodoWrite 工具的参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoWriteArgs {
    pub todos: Vec<TodoItemInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItemInput {
    pub content: String,
    pub status: TodoStatus,
    #[serde(rename = "activeForm")]
    pub active_form: String,
}

impl TodoWriteArgs {
    pub fn parse(arguments: &Value) -> Result<Self, String> {
        serde_json::from_value(arguments.clone())
            .map_err(|e| format!("invalid todo_write arguments: {e}"))
    }

    pub fn to_todo_items(&self) -> Vec<TodoItem> {
        self.todos
            .iter()
            .map(|item| TodoItem {
                content: item.content.clone(),
                status: item.status,
                active_form: item.active_form.clone(),
            })
            .collect()
    }
}

/// TodoWrite 工具定义
pub fn todo_write_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: TODO_WRITE_TOOL_NAME.to_string(),
        description: "Create and update a task list for the current session. The list is rendered to the user as your working plan.\n\n- Each todo has `content`, `status` (\"pending\" | \"in_progress\" | \"completed\"), and `activeForm` (present-tense label shown while in progress).\n- Send the full list each call; it replaces the previous one.\n- Keep one item `in_progress` at a time and mark it `completed` when done.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The updated todo list",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "The task description"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "The task status"
                            },
                            "activeForm": {
                                "type": "string",
                                "description": "Present-tense label shown while in progress (e.g., \"Reading file...\")"
                            }
                        },
                        "required": ["content", "status", "activeForm"]
                    }
                }
            },
            "required": ["todos"]
        }),
    }
}

/// TodoWrite 工具 Handler（仅用于注册，实际执行在 agent 中）
pub struct TodoWriteToolHandler;

#[async_trait]
impl ToolHandler for TodoWriteToolHandler {
    fn definition(&self) -> ToolDefinition {
        todo_write_tool_definition()
    }

    async fn execute(
        &self,
        _context: &ToolContext,
        _arguments: Value,
        _cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        // 这个方法不会被调用，因为在 agent 中有特殊处理
        Err(ToolError::Execution(
            "todo_write should be handled by agent runtime".to_string(),
        ))
    }
}
