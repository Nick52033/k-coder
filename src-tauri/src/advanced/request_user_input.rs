use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::protocol::{ToolDefinition, ToolResult};
use crate::tools::{ToolContext, ToolError, ToolHandler};

/// `request_user_input` 工具的名称。
pub const REQUEST_USER_INPUT_TOOL_NAME: &str = "request_user_input";

const MAX_QUESTIONS: usize = 3;
const MIN_OPTIONS: usize = 2;
const MAX_OPTIONS: usize = 4;
const MAX_QUESTION_LEN: usize = 500;
const MAX_OPTION_LEN: usize = 200;

#[derive(Debug, Clone, Deserialize)]
pub struct RequestUserInputQuestion {
    pub question: String,
    pub options: Vec<String>,
}

/// 工具参数（来自模型）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestUserInputArgs {
    pub questions: Vec<RequestUserInputQuestion>,
}

/// `request_user_input` 工具定义。该工具不直接执行——AgentRuntime 会拦截它，
/// 通过 `UserInputManager` 向前端发起提问并阻塞等待回答。
pub struct RequestUserInputTool;

impl RequestUserInputTool {
    pub fn definition() -> ToolDefinition {
        ToolDefinition {
            name: REQUEST_USER_INPUT_TOOL_NAME.into(),
            description: "Ask the user 1-3 clarifying questions with multiple-choice options. \
                           Each question must have 2-4 mutually exclusive options. \
                           Use this tool when missing information materially changes the plan \
                           and cannot be discovered by reading files or searching. \
                           Do not ask questions whose answers are discoverable from the repo."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_QUESTIONS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "minLength": 1,
                                    "maxLength": MAX_QUESTION_LEN
                                },
                                "options": {
                                    "type": "array",
                                    "minItems": MIN_OPTIONS,
                                    "maxItems": MAX_OPTIONS,
                                    "items": {
                                        "type": "string",
                                        "minLength": 1,
                                        "maxLength": MAX_OPTION_LEN
                                    }
                                }
                            },
                            "required": ["question", "options"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["questions"],
                "additionalProperties": false
            }),
        }
    }

    /// 解析并验证工具参数。
    pub fn parse_arguments(arguments: &Value) -> Result<RequestUserInputArgs, ToolError> {
        let args: RequestUserInputArgs = serde_json::from_value(arguments.clone())
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        if args.questions.is_empty() || args.questions.len() > MAX_QUESTIONS {
            return Err(ToolError::InvalidArguments(format!(
                "questions must contain 1 to {MAX_QUESTIONS} items"
            )));
        }
        for question in &args.questions {
            if question.question.trim().is_empty() {
                return Err(ToolError::InvalidArguments(
                    "question text must not be empty".into(),
                ));
            }
            if question.options.len() < MIN_OPTIONS || question.options.len() > MAX_OPTIONS {
                return Err(ToolError::InvalidArguments(format!(
                    "each question must have {MIN_OPTIONS} to {MAX_OPTIONS} options"
                )));
            }
            if question
                .options
                .iter()
                .any(|option| option.trim().is_empty())
            {
                return Err(ToolError::InvalidArguments(
                    "option text must not be empty".into(),
                ));
            }
        }
        Ok(args)
    }
}

/// 占位 handler——仅在 ToolRegistry 中注册以让工具定义可见。
/// 真正的执行由 `AgentRuntime::execute_tool_call` 拦截处理。
#[async_trait]
impl ToolHandler for RequestUserInputTool {
    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    async fn execute(
        &self,
        _context: &ToolContext,
        _arguments: Value,
        _cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Execution(
            "request_user_input must be intercepted by the agent runtime".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_arguments() {
        let args = RequestUserInputTool::parse_arguments(&json!({
            "questions": [{
                "question": "Which approach?",
                "options": ["A", "B"]
            }]
        }));
        assert!(args.is_ok());
        assert_eq!(args.unwrap().questions.len(), 1);
    }

    #[test]
    fn rejects_too_many_questions() {
        let args = RequestUserInputTool::parse_arguments(&json!({
            "questions": [
                {"question": "Q1", "options": ["A", "B"]},
                {"question": "Q2", "options": ["A", "B"]},
                {"question": "Q3", "options": ["A", "B"]},
                {"question": "Q4", "options": ["A", "B"]}
            ]
        }));
        assert!(args.is_err());
    }

    #[test]
    fn rejects_single_option() {
        let args = RequestUserInputTool::parse_arguments(&json!({
            "questions": [{
                "question": "Q",
                "options": ["only"]
            }]
        }));
        assert!(args.is_err());
    }

    #[test]
    fn rejects_empty_question() {
        let args = RequestUserInputTool::parse_arguments(&json!({
            "questions": [{
                "question": "  ",
                "options": ["A", "B"]
            }]
        }));
        assert!(args.is_err());
    }
}
