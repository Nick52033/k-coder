use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::memory::policy::{detect_sensitivity, explicitly_global_preference, memory_is_expired};
use crate::memory::scope::MemoryScopeResolver;
use crate::memory::{
    CandidateDraft, CandidateOutcome, MemoryError, MemoryOperation, MemoryScope, MemoryScopeKind,
    MemoryService, MemoryStatus, MemoryType, Sensitivity,
};
use crate::protocol::{
    ContentBlock, HistorySortDirection, MessageRole, ThreadItemPayload, ToolDefinition, ToolResult,
    ToolRisk,
};
use crate::storage::memory_repository::{
    MAX_MEMORY_CONTENT_CHARS, MAX_MEMORY_PAGE_SIZE, MAX_MEMORY_REASON_CHARS, MemoryRecord,
};
use crate::storage::{JsonlThreadRepository, now_ms};
use crate::tools::{ToolContext, ToolError, ToolHandler};

const MAX_RECALL_MEMORIES: usize = 40;
const MAX_RECALL_CHARS: usize = 4_000;
const MAX_CURRENT_TURN_ITEMS: u32 = 40;
const MAX_CURRENT_TURN_PAGES: usize = 2;
const MAX_CURRENT_USER_CHARS: usize = 8_000;
const DEFAULT_PROPOSAL_CONFIDENCE: f64 = 0.5;

pub fn memory_tools(
    memory: MemoryService,
    repository: Arc<JsonlThreadRepository>,
) -> (Vec<Arc<dyn ToolHandler>>, HashMap<String, ToolRisk>) {
    let resolver = MemoryScopeResolver::new(repository.projection());
    let mut handlers = Vec::<Arc<dyn ToolHandler>>::new();
    let mut risks = HashMap::new();
    for kind in [
        MemoryToolKind::Remember,
        MemoryToolKind::Propose,
        MemoryToolKind::Recall,
    ] {
        risks.insert(kind.name().to_owned(), kind.risk());
        handlers.push(Arc::new(MemoryTool {
            memory: memory.clone(),
            repository: repository.clone(),
            resolver: resolver.clone(),
            kind,
        }));
    }
    (handlers, risks)
}

#[derive(Clone, Copy)]
enum MemoryToolKind {
    Remember,
    Propose,
    Recall,
}

impl MemoryToolKind {
    fn name(self) -> &'static str {
        match self {
            Self::Remember => "remember",
            Self::Propose => "propose_memory",
            Self::Recall => "recall_memory",
        }
    }

    fn risk(self) -> ToolRisk {
        match self {
            Self::Remember | Self::Propose => ToolRisk::External,
            Self::Recall => ToolRisk::Read,
        }
    }
}

struct MemoryTool {
    memory: MemoryService,
    repository: Arc<JsonlThreadRepository>,
    resolver: MemoryScopeResolver,
    kind: MemoryToolKind,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RememberArguments {
    content: String,
    source: String,
    retention_days: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeArguments {
    operation: String,
    #[serde(rename = "type")]
    memory_type: String,
    content: String,
    reason: String,
    #[serde(default = "default_confidence")]
    confidence: f64,
}

fn default_confidence() -> f64 {
    DEFAULT_PROPOSAL_CONFIDENCE
}

#[async_trait]
impl ToolHandler for MemoryTool {
    fn definition(&self) -> ToolDefinition {
        let (description, input_schema) = match self.kind {
            MemoryToolKind::Remember => (
                "Compatibility memory proposal (type=fact), not a direct save. Legacy retentionDays is validated for compatibility; effective retention is host-configured. The host chooses the scope and source; inspect status for pending review or acceptance.",
                json!({
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "minLength": 1, "maxLength": MAX_MEMORY_CONTENT_CHARS},
                        "source": {"type": "string", "minLength": 1, "maxLength": 240},
                        "retentionDays": {"type": "integer", "minimum": 1, "maximum": 365}
                    },
                    "required": ["content", "source", "retentionDays"],
                    "additionalProperties": false
                }),
            ),
            MemoryToolKind::Propose => (
                "Propose a memory change for host review. Supply only operation, type, content, reason and optional confidence. For update/merge/delete, content must exactly identify one existing memory of the same type. Scope and identity are host-owned; status distinguishes pending, accepted and suppressed proposals.",
                json!({
                    "type": "object",
                    "properties": {
                        "operation": {"type": "string", "enum": ["create", "update", "merge", "delete"]},
                        "type": {"type": "string", "enum": ["preference", "fact", "instruction", "constraint", "work_state", "experience"]},
                        "content": {"type": "string", "minLength": 1, "maxLength": MAX_MEMORY_CONTENT_CHARS},
                        "reason": {"type": "string", "minLength": 1, "maxLength": MAX_MEMORY_REASON_CHARS},
                        "confidence": {"type": "number", "minimum": 0, "maximum": 1}
                    },
                    "required": ["operation", "type", "content", "reason"],
                    "additionalProperties": false
                }),
            ),
            MemoryToolKind::Recall => (
                "Read enabled, unexpired, non-secret memories visible to this thread. Returns only content and type, never host identities or scope paths.",
                json!({"type": "object", "properties": {}, "additionalProperties": false}),
            ),
        };
        ToolDefinition {
            name: self.kind.name().to_owned(),
            description: description.to_owned(),
            input_schema,
        }
    }

    async fn execute(
        &self,
        context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        check_cancelled(&cancellation)?;
        validate_context(context)?;
        match self.kind {
            MemoryToolKind::Recall => {
                if arguments
                    .as_object()
                    .is_none_or(|arguments| !arguments.is_empty())
                {
                    return Err(ToolError::InvalidArguments(
                        "recall_memory takes no arguments".into(),
                    ));
                }
                self.recall(context, &cancellation)
            }
            MemoryToolKind::Remember | MemoryToolKind::Propose => {
                let mut draft = self.draft(context, arguments)?;
                draft.validate().map_err(memory_error)?;
                check_cancelled(&cancellation)?;
                if !self.memory.settings().map_err(memory_error)?.enabled {
                    return tool_proposal_result(
                        self.kind,
                        CandidateOutcome::Suppressed {
                            reason: "memory_disabled".into(),
                        },
                        &draft,
                    );
                }
                let user_text = tokio::select! {
                    _ = cancellation.cancelled() => return Err(ToolError::Cancelled),
                    text = self.current_user_text(context) => text?,
                };
                check_cancelled(&cancellation)?;
                draft.scope = if draft.memory_type == MemoryType::Preference
                    && explicitly_global_preference(&user_text, &draft.content)
                {
                    self.resolver
                        .scopes(&context.thread_id)
                        .map_err(memory_error)?;
                    MemoryScope::user()
                } else {
                    self.resolver
                        .default_scope(&context.thread_id, draft.memory_type)
                        .map_err(memory_error)?
                };
                draft.source_turn_id = Some(context.turn_id.clone());
                draft.validate().map_err(memory_error)?;
                if draft.operation != MemoryOperation::Create {
                    draft.target_memory_id = Some(self.unique_target(&draft, &cancellation)?);
                }
                check_cancelled(&cancellation)?;
                if !self.memory.settings().map_err(memory_error)?.enabled {
                    return tool_proposal_result(
                        self.kind,
                        CandidateOutcome::Suppressed {
                            reason: "memory_disabled".into(),
                        },
                        &draft,
                    );
                }
                let outcome = self
                    .memory
                    .record_candidate(draft.clone())
                    .map_err(memory_error)?;
                tool_proposal_result(self.kind, outcome, &draft)
            }
        }
    }
}

impl MemoryTool {
    fn draft(&self, context: &ToolContext, arguments: Value) -> Result<CandidateDraft, ToolError> {
        let (operation, memory_type, content, reason, confidence) = match self.kind {
            MemoryToolKind::Remember => {
                let request: RememberArguments = serde_json::from_value(arguments)
                    .map_err(|_| ToolError::InvalidArguments("invalid remember payload".into()))?;
                if request.source.trim().is_empty()
                    || request.source.chars().count() > 240
                    || !(1..=365).contains(&request.retention_days)
                {
                    return Err(ToolError::InvalidArguments(
                        "invalid source or retentionDays".into(),
                    ));
                }
                (
                    MemoryOperation::Create,
                    MemoryType::Fact,
                    request.content,
                    request.source,
                    DEFAULT_PROPOSAL_CONFIDENCE,
                )
            }
            MemoryToolKind::Propose => {
                let request: ProposeArguments =
                    serde_json::from_value(arguments).map_err(|_| {
                        ToolError::InvalidArguments("invalid propose_memory payload".into())
                    })?;
                (
                    MemoryOperation::parse(&request.operation).map_err(memory_error)?,
                    MemoryType::parse(&request.memory_type).map_err(memory_error)?,
                    request.content,
                    request.reason,
                    request.confidence,
                )
            }
            MemoryToolKind::Recall => unreachable!(),
        };
        let scope = MemoryScope::new(MemoryScopeKind::Thread, Some(context.thread_id.clone()));
        Ok(CandidateDraft::from_model(
            operation,
            memory_type,
            content,
            reason,
            confidence,
            scope,
        ))
    }

    async fn current_user_text(&self, context: &ToolContext) -> Result<String, ToolError> {
        let mut cursor = None;
        let mut user_text = None;
        for _ in 0..MAX_CURRENT_TURN_PAGES {
            let page = self
                .repository
                .list_thread_items(
                    &context.thread_id,
                    Some(&context.turn_id),
                    cursor.as_deref(),
                    Some(MAX_CURRENT_TURN_ITEMS),
                    HistorySortDirection::Asc,
                )
                .await
                .map_err(|_| ToolError::Execution("current user message is unavailable".into()))?;
            for entry in page.data {
                if entry.turn_id.as_deref() != Some(context.turn_id.as_str())
                    || entry.item.turn_id.as_deref() != Some(context.turn_id.as_str())
                {
                    continue;
                }
                if let ThreadItemPayload::UserMessage { message } = entry.item.payload {
                    if message.role != MessageRole::User || user_text.is_some() {
                        return Err(ToolError::Denied(
                            "current user message is ambiguous".into(),
                        ));
                    }
                    let mut text = String::new();
                    let mut chars = 0;
                    for block in message.content {
                        if let ContentBlock::Text { text: part } = block {
                            chars += part.chars().count();
                            if chars > MAX_CURRENT_USER_CHARS {
                                return Err(ToolError::Denied(
                                    "current user message exceeds the confirmation budget".into(),
                                ));
                            }
                            text.push_str(&part);
                            text.push('\n');
                        }
                    }
                    user_text = Some(text);
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return user_text
                    .ok_or_else(|| ToolError::Denied("current turn has no user message".into()));
            }
        }
        Err(ToolError::Denied(
            "current turn exceeds the confirmation budget".into(),
        ))
    }

    fn unique_target(
        &self,
        draft: &CandidateDraft,
        cancellation: &CancellationToken,
    ) -> Result<String, ToolError> {
        let page = self
            .memory
            .list(
                &draft.scope,
                MemoryStatus::Active,
                None,
                Some(MAX_MEMORY_PAGE_SIZE),
            )
            .map_err(memory_error)?;
        if page.total > u64::from(MAX_MEMORY_PAGE_SIZE) {
            return Err(ToolError::Denied(
                "memory target lookup exceeds the safety budget".into(),
            ));
        }
        let now = now_ms();
        let mut target = None;
        for memory in page.items {
            check_cancelled(cancellation)?;
            if !visible_memory(&memory, &draft.scope, now)
                || memory.memory_type != draft.memory_type.as_str()
                || memory.content.trim() != draft.content.trim()
            {
                continue;
            }
            if target.is_some() {
                return Err(ToolError::Denied(
                    "memory proposal does not identify a unique target".into(),
                ));
            }
            target = Some(memory.id);
        }
        target.ok_or_else(|| {
            ToolError::Denied("memory proposal does not identify a unique target".into())
        })
    }

    fn recall(
        &self,
        context: &ToolContext,
        cancellation: &CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if !self.memory.settings().map_err(memory_error)?.enabled {
            return result(
                json!({"status": "disabled", "memories": []}),
                json!({"count": 0}),
            );
        }
        let scopes = self
            .resolver
            .scopes(&context.thread_id)
            .map_err(memory_error)?;
        let mut memories = Vec::new();
        let mut chars = json!({"status": "recalled", "memories": []})
            .to_string()
            .chars()
            .count();
        let now = now_ms();
        for scope in scopes {
            check_cancelled(cancellation)?;
            let page = self
                .memory
                .list(
                    &scope,
                    MemoryStatus::Active,
                    None,
                    Some(MAX_RECALL_MEMORIES as u32),
                )
                .map_err(memory_error)?;
            for memory in page.items {
                check_cancelled(cancellation)?;
                if !visible_memory(&memory, &scope, now) {
                    continue;
                }
                let item = json!({"content": memory.content, "type": memory.memory_type});
                let length = item.to_string().chars().count() + usize::from(!memories.is_empty());
                if chars + length > MAX_RECALL_CHARS {
                    continue;
                }
                chars += length;
                memories.push(item);
                if memories.len() >= MAX_RECALL_MEMORIES {
                    break;
                }
            }
            if memories.len() >= MAX_RECALL_MEMORIES || chars >= MAX_RECALL_CHARS {
                break;
            }
        }
        check_cancelled(cancellation)?;
        if !self.memory.settings().map_err(memory_error)?.enabled {
            return result(
                json!({"status": "disabled", "memories": []}),
                json!({"count": 0}),
            );
        }
        let count = memories.len();
        result(
            json!({"status": "recalled", "memories": memories}),
            json!({"count": count}),
        )
    }
}

fn validate_context(context: &ToolContext) -> Result<(), ToolError> {
    if Uuid::parse_str(&context.thread_id).is_err() || Uuid::parse_str(&context.turn_id).is_err() {
        return Err(ToolError::Denied(
            "invalid host thread or turn identity".into(),
        ));
    }
    Ok(())
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), ToolError> {
    if cancellation.is_cancelled() {
        return Err(ToolError::Cancelled);
    }
    Ok(())
}

fn memory_error(error: MemoryError) -> ToolError {
    match error.code() {
        "MEM_INVALID_ARGUMENT" | "MEM_INVALID_TYPE" | "MEM_INVALID_OPERATION" => {
            ToolError::InvalidArguments(error.code().to_owned())
        }
        "MEM_SECRET_REJECTED" | "MEM_INVALID_SCOPE" | "MEM_CONFIRMATION_REQUIRED" => {
            ToolError::Denied(error.code().to_owned())
        }
        _ => ToolError::Execution(error.code().to_owned()),
    }
}

fn visible_memory(memory: &MemoryRecord, scope: &MemoryScope, now: u64) -> bool {
    memory.scope_type == scope.kind.as_str()
        && memory.scope_id == scope.id
        && memory.status == MemoryStatus::Active.as_str()
        && MemoryType::parse(&memory.memory_type).is_ok()
        && matches!(memory.sensitivity.as_str(), "normal" | "private")
        && detect_sensitivity(&memory.content) != Sensitivity::SecretCandidate
        && !memory_is_expired(memory, now)
        && !memory.content.contains(&memory.id)
        && !memory
            .scope_id
            .as_ref()
            .is_some_and(|id| memory.content.contains(id))
}

fn tool_proposal_result(
    kind: MemoryToolKind,
    outcome: CandidateOutcome,
    draft: &CandidateDraft,
) -> Result<ToolResult, ToolError> {
    let mut result = proposal_result(outcome, draft)?;
    if matches!(kind, MemoryToolKind::Remember) {
        let mut output: Value = serde_json::from_str(&result.output)
            .map_err(|_| ToolError::Execution("memory output serialization failed".into()))?;
        output["retention"] = json!("host_configured");
        output["retentionNotice"] =
            json!("兼容字段 retentionDays 已校验；有效期由宿主记忆设置决定");
        result.output = output.to_string();
    }
    Ok(result)
}

fn proposal_result(
    outcome: CandidateOutcome,
    draft: &CandidateDraft,
) -> Result<ToolResult, ToolError> {
    let (status, message) = match outcome {
        CandidateOutcome::Pending { .. } => ("pending", "已提交待审核，尚未生效"),
        CandidateOutcome::AutoAccepted { .. } => ("auto_accepted", "已自动接受并生效"),
        CandidateOutcome::Deduplicated { .. } => ("deduplicated", "已有相同记忆，未重复写入"),
        CandidateOutcome::Suppressed { .. } => ("suppressed", "已跳过，未写入记忆"),
    };
    result(
        json!({
            "status": status,
            "message": message,
            "content": draft.content.trim(),
            "type": draft.memory_type.as_str()
        }),
        json!({"status": status}),
    )
}

fn result(output: Value, metadata: Value) -> Result<ToolResult, ToolError> {
    Ok(ToolResult {
        success: true,
        output: serde_json::to_string(&output)
            .map_err(|_| ToolError::Execution("memory output serialization failed".into()))?,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::UpsertMemoryCommand;
    use crate::protocol::{ChatMessage, PROTOCOL_VERSION};
    use crate::storage::memory_repository::{
        MemoryEventKind, MemoryRepository, MemoryWrite, normalize_memory_key,
    };
    use crate::storage::{StoredEvent, StoredEventKind, ThreadRepository};

    struct Fixture {
        _directory: tempfile::TempDir,
        repository: Arc<JsonlThreadRepository>,
        memory: MemoryService,
        context: ToolContext,
    }

    impl Fixture {
        async fn new(text: &str, in_project: bool) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let repository = Arc::new(JsonlThreadRepository::new(directory.path()).unwrap());
            let thread = if in_project {
                repository
                    .create_thread_in_workspace(&directory.path().canonicalize().unwrap())
                    .await
                    .unwrap()
            } else {
                repository.create_standalone_thread().await.unwrap()
            };
            let context = ToolContext {
                thread_id: thread.id,
                turn_id: Uuid::new_v4().to_string(),
                call_id: Uuid::new_v4().to_string(),
                workspace_root: directory.path().join("unrelated-ui-selection"),
                approval: None,
                progress: None,
                plan_reconciliation: None,
            };
            append_user(&repository, &context, text, None).await;
            let memory = MemoryService::new(repository.projection(), true);
            Self {
                _directory: directory,
                repository,
                memory,
                context,
            }
        }

        fn tool(&self, name: &str) -> Arc<dyn ToolHandler> {
            memory_tools(self.memory.clone(), self.repository.clone())
                .0
                .into_iter()
                .find(|tool| tool.definition().name == name)
                .unwrap()
        }

        fn scope(&self, memory_type: MemoryType) -> MemoryScope {
            MemoryScopeResolver::new(self.repository.projection())
                .default_scope(&self.context.thread_id, memory_type)
                .unwrap()
        }
    }

    async fn append_user(
        repository: &JsonlThreadRepository,
        context: &ToolContext,
        text: &str,
        hidden: Option<&str>,
    ) {
        let mut content = vec![ContentBlock::Text {
            text: text.to_owned(),
        }];
        if let Some(text) = hidden {
            content.push(ContentBlock::Context {
                text: text.to_owned(),
            });
        }
        let message = ChatMessage {
            schema_version: PROTOCOL_VERSION,
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content,
            created_at_ms: now_ms(),
        };
        for kind in [
            StoredEventKind::UserMessage { message },
            StoredEventKind::TurnStarted,
        ] {
            repository
                .append(StoredEvent::new(
                    &context.thread_id,
                    Some(context.turn_id.clone()),
                    kind,
                ))
                .await
                .unwrap();
        }
    }

    fn proposal(operation: &str, memory_type: &str, content: &str) -> Value {
        json!({"operation": operation, "type": memory_type, "content": content,
            "reason": "the user stated this memory", "confidence": 0.9})
    }

    fn decoded(result: &ToolResult) -> Value {
        serde_json::from_str(&result.output).unwrap()
    }

    #[tokio::test]
    async fn tool_schemas_and_risks_keep_host_identity_out_of_model_input() {
        let fixture = Fixture::new("请记住偏好", true).await;
        let (tools, risks) = memory_tools(fixture.memory.clone(), fixture.repository.clone());
        assert_eq!(risks["remember"], ToolRisk::External);
        assert_eq!(risks["propose_memory"], ToolRisk::External);
        assert_eq!(risks["recall_memory"], ToolRisk::Read);
        for tool in tools {
            let definition = tool.definition();
            assert_eq!(definition.input_schema["additionalProperties"], false);
            assert!(jsonschema::validator_for(&definition.input_schema).is_ok());
            let properties = definition.input_schema["properties"].as_object().unwrap();
            for forbidden in [
                "id",
                "scope",
                "scopeId",
                "memoryId",
                "confirmation",
                "source_turn_id",
            ] {
                assert!(!properties.contains_key(forbidden));
            }
        }
        for name in ["remember", "propose_memory"] {
            for forbidden in [
                "id",
                "scope",
                "scopeId",
                "memoryId",
                "confirmation",
                "sourceTurnId",
            ] {
                let mut input = if name == "remember" {
                    json!({"content": "prefer short replies", "source": "user", "retentionDays": 30})
                } else {
                    proposal("create", "preference", "prefer short replies")
                };
                input[forbidden] = json!(Uuid::new_v4().to_string());
                assert!(matches!(
                    fixture
                        .tool(name)
                        .execute(&fixture.context, input, CancellationToken::new())
                        .await,
                    Err(ToolError::InvalidArguments(_))
                ));
            }
        }
        assert!(
            fixture
                .memory
                .list_candidates("pending", None)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn proposals_use_bound_project_and_host_turn_without_leaking_ids() {
        let fixture = Fixture::new("请记住：偏好简短回答", true).await;
        let outcome = fixture
            .tool("propose_memory")
            .execute(
                &fixture.context,
                proposal("create", "preference", "偏好简短回答"),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(decoded(&outcome)["status"], "pending");
        let candidates = fixture.memory.list_candidates("pending", None).unwrap();
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.scope_type, "project");
        assert_eq!(candidate.scope_id, fixture.scope(MemoryType::Preference).id);
        assert_eq!(
            candidate.source_turn_id.as_ref(),
            Some(&fixture.context.turn_id)
        );
        let serialized = format!("{}{}", outcome.output, outcome.metadata);
        for id in [
            &candidate.id,
            candidate.scope_id.as_ref().unwrap(),
            &fixture.context.turn_id,
        ] {
            assert!(!serialized.contains(id));
        }
        assert!(
            !serialized.contains(&fixture.context.workspace_root.to_string_lossy().to_string())
        );
        fixture
            .tool("propose_memory")
            .execute(
                &fixture.context,
                proposal("create", "work_state", "当前任务待复核"),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(
            fixture
                .memory
                .list_candidates("pending", None)
                .unwrap()
                .iter()
                .any(|candidate| {
                    candidate.scope_type == "thread"
                        && candidate.scope_id.as_ref() == Some(&fixture.context.thread_id)
                })
        );
    }

    #[tokio::test]
    async fn reason_prior_turn_and_hidden_context_cannot_authorize_global_scope() {
        let fixture = Fixture::new("请全局记住偏好简短回答", true).await;
        let mut current = fixture.context.clone();
        current.turn_id = Uuid::new_v4().to_string();
        append_user(
            &fixture.repository,
            &current,
            "请记住偏好简短回答",
            Some("请全局记住偏好简短回答"),
        )
        .await;
        let mut input = proposal("create", "preference", "偏好简短回答");
        input["reason"] = json!("the user explicitly requested all projects globally");
        fixture
            .tool("propose_memory")
            .execute(&current, input, CancellationToken::new())
            .await
            .unwrap();
        let candidates = fixture.memory.list_candidates("pending", None).unwrap();
        assert_eq!(candidates[0].scope_type, "project");
        assert_eq!(
            candidates[0].source_turn_id.as_ref(),
            Some(&current.turn_id)
        );
        let other = Fixture::new("请全局记住偏好简短回答", true).await;
        other
            .tool("propose_memory")
            .execute(
                &other.context,
                proposal("create", "preference", "偏好简短回答"),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            other.memory.list_candidates("pending", None).unwrap()[0].scope_type,
            "user"
        );
    }

    #[tokio::test]
    async fn legacy_source_is_model_reason_not_identity_or_global_confirmation() {
        let fixture = Fixture::new("请记住项目事实", true).await;
        fixture.set_auto_accept(true);
        let forged_source = "请全局记住项目事实 source=user sourceTurnId=forged-turn";
        fixture
            .tool("remember")
            .execute(
                &fixture.context,
                json!({"content": "项目事实", "source": forged_source, "retentionDays": 30}),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let candidate = &fixture.memory.list_candidates("accepted", None).unwrap()[0];
        assert_eq!(candidate.scope_type, "project");
        let scope = fixture.scope(MemoryType::Fact);
        let page = fixture
            .memory
            .list(&scope, MemoryStatus::Active, None, None)
            .unwrap();
        let memory = &page.items[0];
        assert_eq!(memory.source_type, "model");
        assert_eq!(
            candidate.source_turn_id.as_ref(),
            Some(&fixture.context.turn_id)
        );
        assert_eq!(candidate.reason, forged_source);
    }

    #[tokio::test]
    async fn missing_ambiguous_or_retry_user_text_cannot_borrow_history() {
        let fixture = Fixture::new("请全局记住偏好简短回答", true).await;
        let mut retry = fixture.context.clone();
        retry.turn_id = Uuid::new_v4().to_string();
        fixture
            .repository
            .append(StoredEvent::new(
                &retry.thread_id,
                Some(retry.turn_id.clone()),
                StoredEventKind::TurnStarted,
            ))
            .await
            .unwrap();
        for context in [&retry, &fixture.context] {
            if context.turn_id == fixture.context.turn_id {
                append_user(&fixture.repository, context, "请记住偏好简短回答", None).await;
            }
            assert!(matches!(
                fixture
                    .tool("propose_memory")
                    .execute(
                        context,
                        proposal("create", "preference", "偏好简短回答"),
                        CancellationToken::new(),
                    )
                    .await,
                Err(ToolError::Denied(_))
            ));
        }
        assert!(
            fixture
                .memory
                .list_candidates("pending", None)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn unavailable_binding_keeps_user_thread_recall_but_denies_project_writes() {
        let fixture = Fixture::new("请全局记住偏好简短回答", true).await;
        let scope = fixture.scope(MemoryType::Fact);
        store_memory(&fixture, scope, MemoryType::Fact, "project fact");
        store_memory(
            &fixture,
            MemoryScope::user(),
            MemoryType::Preference,
            "user fact",
        );
        store_memory(
            &fixture,
            fixture.scope(MemoryType::WorkState),
            MemoryType::WorkState,
            "thread fact",
        );
        fixture
            .repository
            .projection()
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE threads SET workspace_path=?1 WHERE id=?2",
                    rusqlite::params![
                        fixture
                            ._directory
                            .path()
                            .join("missing")
                            .to_string_lossy()
                            .to_string(),
                        fixture.context.thread_id
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        let result = fixture
            .tool("recall_memory")
            .execute(&fixture.context, json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert!(result.output.contains("user fact"));
        assert!(result.output.contains("thread fact"));
        assert!(!result.output.contains("project fact"));
        assert!(matches!(
            fixture
                .tool("propose_memory")
                .execute(
                    &fixture.context,
                    proposal("create", "fact", "new project fact"),
                    CancellationToken::new(),
                )
                .await,
            Err(ToolError::InvalidArguments(_))
        ));
        for (memory_type, content) in [
            ("work_state", "new thread state"),
            ("preference", "偏好简短回答"),
        ] {
            fixture
                .tool("propose_memory")
                .execute(
                    &fixture.context,
                    proposal("create", memory_type, content),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
        }
        let candidates = fixture.memory.list_candidates("pending", None).unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.scope_type == "user")
        );
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.scope_type == "thread")
        );
    }

    #[tokio::test]
    async fn disabled_cancelled_and_secret_calls_do_not_record_candidates() {
        let fixture = Fixture::new("请记住偏好简短回答", true).await;
        fixture.memory.set_settings(false, false, 0).unwrap();
        let result = fixture
            .tool("propose_memory")
            .execute(
                &fixture.context,
                proposal("create", "preference", "偏好简短回答"),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(decoded(&result)["status"], "suppressed");
        assert!(
            fixture
                .repository
                .projection()
                .list_projects()
                .unwrap()
                .is_empty()
        );
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            fixture
                .tool("remember")
                .execute(&fixture.context, json!({}), cancellation)
                .await
                .unwrap_err(),
            ToolError::Cancelled
        );
        fixture.memory.set_settings(true, true, 0).unwrap();
        let mut input = proposal("create", "preference", "偏好简短回答");
        input["reason"] = json!("password: hunter2sword");
        assert!(matches!(
            fixture
                .tool("propose_memory")
                .execute(&fixture.context, input, CancellationToken::new())
                .await,
            Err(ToolError::Denied(_))
        ));
        assert!(
            fixture
                .memory
                .list_candidates("pending", None)
                .unwrap()
                .is_empty()
        );
    }

    fn store_memory(
        fixture: &Fixture,
        scope: MemoryScope,
        memory_type: MemoryType,
        content: &str,
    ) -> MemoryRecord {
        fixture
            .memory
            .upsert(UpsertMemoryCommand {
                memory_id: None,
                content: content.to_owned(),
                memory_type,
                scope,
                expires_at_ms: None,
            })
            .unwrap()
            .memory
    }

    #[tokio::test]
    async fn recall_filters_scope_ttl_secrets_and_limits_output() {
        let fixture = Fixture::new("recall", true).await;
        let scope = fixture.scope(MemoryType::Fact);
        store_memory(&fixture, scope.clone(), MemoryType::Fact, "project fact");
        store_memory(
            &fixture,
            MemoryScope::user(),
            MemoryType::Preference,
            "global preference",
        );
        store_memory(
            &fixture,
            MemoryScope::new(MemoryScopeKind::Project, Some(Uuid::new_v4().to_string())),
            MemoryType::Fact,
            "another project fact",
        );
        let repository = MemoryRepository::new(fixture.repository.projection());
        let expired = store_memory(&fixture, scope.clone(), MemoryType::Fact, "expired fact");
        repository
            .append(MemoryEventKind::MemoryUpserted(MemoryWrite {
                id: expired.id,
                scope_type: scope.kind.as_str().to_owned(),
                scope_id: scope.id.clone(),
                memory_type: "fact".into(),
                normalized_key: normalize_memory_key("expired fact"),
                content: "expired fact".into(),
                source_type: "user".into(),
                source_ref: None,
                confidence: 1.0,
                sensitivity: "normal".into(),
                status: "active".into(),
                revision: 2,
                expires_at_ms: Some(now_ms().saturating_sub(1)),
                created_at_ms: now_ms().saturating_sub(10_000),
            }))
            .unwrap();
        fixture
            .repository
            .projection()
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE memories SET content=?1 WHERE normalized_key=?2",
                    rusqlite::params![
                        "password: hunter2sword",
                        normalize_memory_key("project fact")
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        let result = fixture
            .tool("recall_memory")
            .execute(&fixture.context, json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert!(result.output.contains("global preference"));
        for forbidden in [
            "hunter2sword",
            "another project fact",
            "expired fact",
            "scopeId",
            "scopeType",
            "memoryId",
        ] {
            assert!(!result.output.contains(forbidden));
        }
        for item in decoded(&result)["memories"].as_array().unwrap() {
            assert_eq!(item.as_object().unwrap().len(), 2);
        }
        for index in 0..45 {
            store_memory(
                &fixture,
                scope.clone(),
                MemoryType::Fact,
                &format!("bounded memory {index}"),
            );
        }
        store_memory(&fixture, scope, MemoryType::Fact, &"长".repeat(3_999));
        let result = fixture
            .tool("recall_memory")
            .execute(&fixture.context, json!({}), CancellationToken::new())
            .await
            .unwrap();
        let output = decoded(&result);
        let memories = output["memories"].as_array().unwrap();
        assert!(memories.len() <= MAX_RECALL_MEMORIES);
        let chars: usize = memories
            .iter()
            .map(|memory| {
                memory["content"].as_str().unwrap().chars().count()
                    + memory["type"].as_str().unwrap().chars().count()
            })
            .sum();
        assert!(chars <= MAX_RECALL_CHARS);
        assert!(result.output.chars().count() <= MAX_RECALL_CHARS);
        fixture.memory.set_settings(false, false, 0).unwrap();
        let disabled = fixture
            .tool("recall_memory")
            .execute(&fixture.context, json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(decoded(&disabled)["status"], "disabled");
    }

    #[tokio::test]
    async fn deletion_is_a_reviewed_unique_host_match_and_never_a_direct_delete() {
        let fixture = Fixture::new("请忘记项目事实", true).await;
        fixture.memory.set_settings(true, true, 0).unwrap();
        let scope = fixture.scope(MemoryType::Fact);
        let memory = store_memory(
            &fixture,
            scope.clone(),
            MemoryType::Fact,
            "project fact to forget",
        );
        let result = fixture
            .tool("propose_memory")
            .execute(
                &fixture.context,
                proposal("delete", "fact", "project fact to forget"),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(decoded(&result)["status"], "pending");
        assert!(!result.output.contains(&memory.id));
        assert_eq!(
            fixture
                .memory
                .list(&scope, MemoryStatus::Active, None, None)
                .unwrap()
                .items
                .len(),
            1
        );
        let candidates = fixture.memory.list_candidates("pending", None).unwrap();
        assert_eq!(candidates[0].target_memory_id.as_ref(), Some(&memory.id));
        assert!(matches!(
            fixture
                .tool("propose_memory")
                .execute(
                    &fixture.context,
                    proposal("delete", "preference", "project fact to forget"),
                    CancellationToken::new()
                )
                .await,
            Err(ToolError::Denied(_))
        ));
        store_memory(
            &fixture,
            MemoryScope::new(MemoryScopeKind::Project, Some(Uuid::new_v4().to_string())),
            MemoryType::Fact,
            "foreign fact",
        );
        assert!(matches!(
            fixture
                .tool("propose_memory")
                .execute(
                    &fixture.context,
                    proposal("delete", "fact", "foreign fact"),
                    CancellationToken::new()
                )
                .await,
            Err(ToolError::Denied(_))
        ));
        assert!(matches!(
            fixture
                .tool("propose_memory")
                .execute(
                    &fixture.context,
                    proposal("delete", "fact", "project fact"),
                    CancellationToken::new()
                )
                .await,
            Err(ToolError::Denied(_))
        ));
    }

    #[tokio::test]
    async fn remember_preserves_legacy_schema_defaults_to_fact_and_host_retention() {
        let fixture = Fixture::new("请记住项目事实", true).await;
        fixture.memory.set_settings(true, false, 30).unwrap();
        let before = now_ms();
        let result = fixture.tool("remember").execute(&fixture.context,
            json!({"content": "compatibility fact", "source": "user message", "retentionDays": 30}),
            CancellationToken::new()).await.unwrap();
        assert_eq!(decoded(&result)["status"], "pending");
        assert_eq!(decoded(&result)["type"], "fact");
        let candidate = fixture
            .memory
            .list_candidates("pending", None)
            .unwrap()
            .remove(0);
        assert_eq!(decoded(&result)["retention"], "host_configured");
        fixture
            .memory
            .review_candidate(&candidate.id, crate::memory::CandidateDecision::Accept)
            .unwrap();
        let memories = fixture
            .memory
            .list(
                &fixture.scope(MemoryType::Fact),
                MemoryStatus::Active,
                None,
                None,
            )
            .unwrap();
        let expiry = memories.items[0].expires_at_ms.unwrap();
        let ttl = crate::memory::policy::days_to_ms(30);
        assert!(expiry >= before + ttl);
        assert!(expiry <= now_ms() + ttl);
        assert_eq!(memories.items[0].source_type, "model");
        assert_eq!(
            memories.items[0].source_ref.as_ref(),
            Some(&fixture.context.turn_id)
        );
    }
}
