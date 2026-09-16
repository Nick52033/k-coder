//! Tool surface for the structured knowledge layer.
//!
//! Two tools, and the split between them is the whole point of Task 6:
//!
//! * [`ProposeKnowledgeFactTool`] can only *propose*. It requires a citation id, and the service
//!   resolves that id through the turn-bound citation table, so a model can only propose a relation
//!   it actually read in this turn — and the proposal lands as `candidate`, never as `active`.
//! * [`KnowledgeRelationsTool`] is read-only. It is the only way to read the graph, and it returns
//!   `active` facts with the citation they came from.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::entities::candidate::FactCandidate;
use crate::entities::service::{EntityService, MAX_RELATION_RESULTS};
use crate::protocol::{ToolDefinition, ToolResult, ToolRisk};
use crate::storage::knowledge_entity_repository::{
    MAX_ENTITY_NAME_CHARS, MAX_OBJECT_TEXT_CHARS, MAX_PREDICATE_CHARS,
};
use crate::tools::{ToolContext, ToolError, ToolHandler};

/// Upper bound for the entity name a relation query accepts.
pub const MAX_RELATION_NAME_CHARS: usize = MAX_ENTITY_NAME_CHARS;

pub struct ProposeKnowledgeFactTool {
    service: EntityService,
}

pub struct KnowledgeRelationsTool {
    service: EntityService,
}

impl ProposeKnowledgeFactTool {
    pub fn new(service: EntityService) -> Self {
        Self { service }
    }
}

impl KnowledgeRelationsTool {
    pub fn new(service: EntityService) -> Self {
        Self { service }
    }
}

fn argument_string(arguments: &serde_json::Value, field: &str) -> Result<String, ToolError> {
    arguments
        .get(field)
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .ok_or_else(|| ToolError::InvalidArguments(format!("{field} must be a string")))
}

#[async_trait]
impl ToolHandler for ProposeKnowledgeFactTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "propose_knowledge_fact".into(),
            description: "Propose a relation about the user's code as a candidate, citing a citation id that search_knowledge returned in this turn. The proposal is queued for the user to review and never becomes an active fact by itself. Do not propose anything you did not read from a citation.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "minLength": 1, "maxLength": MAX_RELATION_NAME_CHARS},
                    "predicate": {"type": "string", "minLength": 1, "maxLength": MAX_PREDICATE_CHARS},
                    "object": {"type": "string", "minLength": 1, "maxLength": MAX_OBJECT_TEXT_CHARS},
                    "citationId": {"type": "string", "minLength": 1, "maxLength": 128}
                },
                "required": ["subject", "predicate", "object", "citationId"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(
        &self,
        context: &ToolContext,
        arguments: serde_json::Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let candidate = FactCandidate::from_model(
            argument_string(&arguments, "subject")?,
            argument_string(&arguments, "predicate")?,
            argument_string(&arguments, "object")?,
            argument_string(&arguments, "citationId")?,
        );
        let outcome = self
            .service
            .propose_fact(&context.thread_id, &context.turn_id, candidate)
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        let fact = outcome.fact();
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&json!({
                "status": outcome.status(),
                "fact": fact,
            }))
            .map_err(|error| ToolError::Execution(error.to_string()))?,
            metadata: json!({
                "status": outcome.status(),
                "factId": fact.id,
                "factStatus": fact.status,
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for KnowledgeRelationsTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "query_knowledge_relations".into(),
            description: "Read the relations that are already active about one entity name in the current workspace. Only reviewed facts are returned, each with the citation it came from. This is read-only and cannot create or change a fact.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "minLength": 1, "maxLength": MAX_RELATION_NAME_CHARS},
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_RELATION_RESULTS}
                },
                "required": ["name"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(
        &self,
        context: &ToolContext,
        arguments: serde_json::Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let name = argument_string(&arguments, "name")?;
        let limit = arguments
            .get("limit")
            .and_then(|value| value.as_u64())
            .unwrap_or(MAX_RELATION_RESULTS as u64)
            .clamp(1, MAX_RELATION_RESULTS as u64) as usize;
        let result = self
            .service
            .relations(&context.workspace_root, &name, limit)
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&result)
                .map_err(|error| ToolError::Execution(error.to_string()))?,
            metadata: json!({
                "subjectFound": result.subject.is_some(),
                "relationCount": result.relations.len(),
            }),
        })
    }
}

pub fn entity_tool_risks() -> HashMap<String, ToolRisk> {
    HashMap::from([
        // The proposal is a durable write to the knowledge store, so it is approved like any other
        // write tool; the review queue is a second, later gate that decides whether it is *true*.
        ("propose_knowledge_fact".into(), ToolRisk::Write),
        ("query_knowledge_relations".into(), ToolRisk::Read),
    ])
}
