use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::patch::{PatchError, PatchService};
use crate::policy::{
    AllowRegisteredTools, PolicyDecision, PolicyEngine, ReadOnlyWorkspacePolicy, WorkspacePolicy,
};
use crate::protocol::{
    ChangeSet, ExpectedFileHash, PatchPreview, ToolDefinition, ToolResult, ToolRisk,
};

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub thread_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub workspace_root: PathBuf,
    pub approval: Option<ApprovedToolExecution>,
}

#[derive(Debug, Clone)]
pub struct ApprovedToolExecution {
    pub patch: Option<String>,
    pub selected_paths: Vec<String>,
    pub expected_hashes: Vec<ExpectedFileHash>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),
    #[error("tool execution denied: {0}")]
    Denied(String),
    #[error("tool execution was cancelled")]
    Cancelled,
    #[error("tool execution requires approval: {0}")]
    ApprovalRequired(String),
    #[error(transparent)]
    Patch(#[from] PatchError),
    #[error("tool execution failed: {0}")]
    Execution(String),
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    fn preview(
        &self,
        _context: &ToolContext,
        _arguments: &Value,
    ) -> Result<Option<PatchPreview>, ToolError> {
        Ok(None)
    }
    async fn execute(
        &self,
        context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolAuthorization {
    pub decision: PolicyDecision,
    pub risk: ToolRisk,
}

#[derive(Clone)]
pub struct ToolRegistry {
    handlers: Arc<HashMap<String, Arc<dyn ToolHandler>>>,
    policy: Arc<dyn PolicyEngine>,
    patch_service: Option<PatchService>,
}

impl ToolRegistry {
    pub fn read_only() -> Self {
        Self::new_with_policy(
            vec![
                Arc::new(ListDirectoryTool) as Arc<dyn ToolHandler>,
                Arc::new(ReadFileTool) as Arc<dyn ToolHandler>,
            ],
            Arc::new(ReadOnlyWorkspacePolicy),
        )
        .expect("built-in tool names and schemas must be valid")
    }

    pub fn workspace_tools(patch_service: PatchService) -> Self {
        let mut registry = Self::new_with_policy(
            vec![
                Arc::new(ListDirectoryTool) as Arc<dyn ToolHandler>,
                Arc::new(ReadFileTool) as Arc<dyn ToolHandler>,
                Arc::new(ApplyPatchTool {
                    service: patch_service.clone(),
                }) as Arc<dyn ToolHandler>,
                Arc::new(WriteFileTool {
                    service: patch_service.clone(),
                }) as Arc<dyn ToolHandler>,
            ],
            Arc::new(WorkspacePolicy),
        )
        .expect("built-in workspace tool names and schemas must be valid");
        registry.patch_service = Some(patch_service);
        registry
    }

    pub fn new(handlers: Vec<Arc<dyn ToolHandler>>) -> Result<Self, ToolError> {
        Self::new_with_policy(handlers, Arc::new(AllowRegisteredTools))
    }

    pub fn new_with_policy(
        handlers: Vec<Arc<dyn ToolHandler>>,
        policy: Arc<dyn PolicyEngine>,
    ) -> Result<Self, ToolError> {
        let mut registered = HashMap::new();
        for handler in handlers {
            let definition = handler.definition();
            if definition.name.trim().is_empty() {
                return Err(ToolError::InvalidArguments(
                    "tool name must not be empty".to_string(),
                ));
            }
            jsonschema::validator_for(&definition.input_schema).map_err(|error| {
                ToolError::InvalidArguments(format!(
                    "tool {} has an invalid JSON schema: {error}",
                    definition.name
                ))
            })?;
            if registered
                .insert(definition.name.clone(), handler)
                .is_some()
            {
                return Err(ToolError::InvalidArguments(format!(
                    "duplicate tool name: {}",
                    definition.name
                )));
            }
        }
        Ok(Self {
            handlers: Arc::new(registered),
            policy,
            patch_service: None,
        })
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = self
            .handlers
            .values()
            .map(|handler| handler.definition())
            .collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        definitions
    }

    pub async fn dispatch(
        &self,
        context: &ToolContext,
        name: &str,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let authorization = self.authorization(name, &arguments)?;
        match authorization.decision {
            PolicyDecision::Allow => {}
            PolicyDecision::RequireApproval { reason } => {
                return Err(ToolError::ApprovalRequired(reason));
            }
            PolicyDecision::Deny { reason } => return Err(ToolError::Denied(reason)),
        }
        self.dispatch_authorized(context, name, arguments, cancellation)
            .await
    }

    pub fn authorization(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Result<ToolAuthorization, ToolError> {
        self.validate_arguments(name, arguments)?;
        Ok(ToolAuthorization {
            decision: self.policy.authorize(name, arguments),
            risk: self.policy.risk(name, arguments),
        })
    }

    pub fn preview(
        &self,
        context: &ToolContext,
        name: &str,
        arguments: &Value,
    ) -> Result<Option<PatchPreview>, ToolError> {
        self.validate_arguments(name, arguments)?;
        self.handlers
            .get(name)
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))?
            .preview(context, arguments)
    }

    pub async fn dispatch_authorized(
        &self,
        context: &ToolContext,
        name: &str,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        self.validate_arguments(name, &arguments)?;
        let handler = self
            .handlers
            .get(name)
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))?;
        handler.execute(context, arguments, cancellation).await
    }

    pub async fn rollback_change(
        &self,
        workspace_root: PathBuf,
        change_set: ChangeSet,
    ) -> Result<ChangeSet, ToolError> {
        let service = self.patch_service.as_ref().ok_or_else(|| {
            ToolError::Execution("the registry has no file change service".to_string())
        })?;
        service
            .undo(workspace_root, change_set)
            .await
            .map_err(ToolError::from)
    }

    fn validate_arguments(&self, name: &str, arguments: &Value) -> Result<(), ToolError> {
        let handler = self
            .handlers
            .get(name)
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))?;
        let definition = handler.definition();
        let validator = jsonschema::validator_for(&definition.input_schema).map_err(|error| {
            ToolError::Execution(format!("registered JSON schema became invalid: {error}"))
        })?;
        validator
            .validate(arguments)
            .map_err(|error| ToolError::InvalidArguments(format!("{}: {error}", definition.name)))
    }
}

#[derive(Debug, Clone)]
struct Workspace {
    root: PathBuf,
}

impl Workspace {
    fn new(root: &Path) -> Result<Self, ToolError> {
        let root = root.canonicalize().map_err(|error| {
            ToolError::Denied(format!("workspace root cannot be resolved: {error}"))
        })?;
        if !root.is_dir() {
            return Err(ToolError::Denied(
                "workspace root is not a directory".to_string(),
            ));
        }
        Ok(Self { root })
    }

    fn resolve_existing(&self, relative: &str) -> Result<PathBuf, ToolError> {
        let relative = Path::new(relative);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(ToolError::Denied(
                "path must be relative and must not contain parent traversal".to_string(),
            ));
        }
        let target =
            self.root.join(relative).canonicalize().map_err(|error| {
                ToolError::Execution(format!("path cannot be resolved: {error}"))
            })?;
        if !target.starts_with(&self.root) {
            return Err(ToolError::Denied(
                "path resolves outside the workspace".to_string(),
            ));
        }
        Ok(target)
    }
}

const DEFAULT_DIRECTORY_LIMIT: usize = 200;
const MAX_DIRECTORY_LIMIT: usize = 500;
const DEFAULT_READ_BYTES: usize = 128 * 1024;
const MAX_READ_BYTES: usize = 256 * 1024;
const MAX_FILE_SIZE_BYTES: u64 = 4 * 1024 * 1024;
const IGNORED_NAMES: &[&str] = &[".git", "node_modules", "target", "dist", "build"];

struct ListDirectoryTool;

#[async_trait]
impl ToolHandler for ListDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_directory".to_string(),
            description: "List direct children of a directory inside the current workspace."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_DIRECTORY_LIMIT }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(
        &self,
        context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let path = required_string(&arguments, "path")?;
        let limit = optional_usize(&arguments, "limit")?.unwrap_or(DEFAULT_DIRECTORY_LIMIT);
        let workspace = Workspace::new(&context.workspace_root)?;
        let directory = workspace.resolve_existing(path)?;
        if !directory.is_dir() {
            return Err(ToolError::InvalidArguments(
                "path is not a directory".to_string(),
            ));
        }

        let mut reader = tokio::fs::read_dir(&directory)
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        let mut entries = Vec::new();
        let mut omitted = 0usize;
        loop {
            let entry = tokio::select! {
                _ = cancellation.cancelled() => return Err(ToolError::Cancelled),
                entry = reader.next_entry() => entry,
            }
            .map_err(|error| ToolError::Execution(error.to_string()))?;
            let Some(entry) = entry else { break };
            let name = entry.file_name().to_string_lossy().to_string();
            if IGNORED_NAMES.contains(&name.as_str()) {
                continue;
            }
            if entries.len() >= limit {
                omitted = 1;
                break;
            }
            let file_type = entry
                .file_type()
                .await
                .map_err(|error| ToolError::Execution(error.to_string()))?;
            entries.push(json!({
                "name": name,
                "kind": if file_type.is_dir() { "directory" } else if file_type.is_file() { "file" } else { "link" }
            }));
        }
        entries.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
        let entry_count = entries.len();
        let output = serde_json::to_string_pretty(&json!({
            "path": path,
            "entries": entries,
            "truncated": omitted > 0,
            "omitted": omitted
        }))
        .map_err(|error| ToolError::Execution(error.to_string()))?;
        Ok(ToolResult {
            success: true,
            output,
            metadata: json!({ "entryCount": entry_count, "truncated": omitted > 0 }),
        })
    }
}

struct ReadFileTool;

#[async_trait]
impl ToolHandler for ReadFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a bounded text range from a file inside the current workspace."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "minLength": 1 },
                    "offset": { "type": "integer", "minimum": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_READ_BYTES }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(
        &self,
        context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let path = required_string(&arguments, "path")?;
        let offset = optional_usize(&arguments, "offset")?.unwrap_or(0);
        let limit = optional_usize(&arguments, "limit")?.unwrap_or(DEFAULT_READ_BYTES);
        let workspace = Workspace::new(&context.workspace_root)?;
        let file = workspace.resolve_existing(path)?;
        if !file.is_file() {
            return Err(ToolError::InvalidArguments(
                "path is not a file".to_string(),
            ));
        }
        let file_size = tokio::fs::metadata(&file)
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?
            .len();
        if file_size > MAX_FILE_SIZE_BYTES {
            return Err(ToolError::Execution(format!(
                "file exceeds the {MAX_FILE_SIZE_BYTES} byte read limit"
            )));
        }
        let bytes = tokio::select! {
            _ = cancellation.cancelled() => return Err(ToolError::Cancelled),
            bytes = tokio::fs::read(&file) => bytes,
        }
        .map_err(|error| ToolError::Execution(error.to_string()))?;
        let text = decode_text(&bytes)?;
        if offset > text.len() || !text.is_char_boundary(offset) {
            return Err(ToolError::InvalidArguments(
                "offset must be a valid UTF-8 byte boundary within the decoded text".to_string(),
            ));
        }
        let end_limit = offset.saturating_add(limit).min(text.len());
        let mut end = end_limit;
        while end > offset && !text.is_char_boundary(end) {
            end -= 1;
        }
        let output = text[offset..end].to_string();
        Ok(ToolResult {
            success: true,
            output,
            metadata: json!({
                "path": path,
                "offset": offset,
                "bytesReturned": end - offset,
                "totalBytes": text.len(),
                "truncated": end < text.len()
            }),
        })
    }
}

struct ApplyPatchTool {
    service: PatchService,
}

#[derive(Deserialize)]
struct ApplyPatchArguments {
    patch: String,
}

#[async_trait]
impl ToolHandler for ApplyPatchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "apply_patch".to_string(),
            description: "Propose a strict multi-file patch for user review. The patch must use '*** Begin Patch', Add/Update/Delete File headers, optional '*** Move to', '@@' hunks, and '*** End Patch'. Never place a patch only in assistant text."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patch": { "type": "string", "minLength": 1, "maxLength": 1048576 }
                },
                "required": ["patch"],
                "additionalProperties": false
            }),
        }
    }

    fn preview(
        &self,
        context: &ToolContext,
        arguments: &Value,
    ) -> Result<Option<PatchPreview>, ToolError> {
        let arguments: ApplyPatchArguments = serde_json::from_value(arguments.clone())
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        Ok(Some(self.service.preview_patch(
            &context.workspace_root,
            &arguments.patch,
        )?))
    }

    async fn execute(
        &self,
        context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let arguments: ApplyPatchArguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let approval = context.approval.as_ref().ok_or_else(|| {
            ToolError::Denied("apply_patch requires a backend approval capability".to_string())
        })?;
        let patch = approval.patch.clone().unwrap_or(arguments.patch);
        let change_set = self
            .service
            .apply_patch(
                context.workspace_root.clone(),
                context.thread_id.clone(),
                context.turn_id.clone(),
                context.call_id.clone(),
                patch,
                approval.selected_paths.clone(),
                approval.expected_hashes.clone(),
            )
            .await?;
        change_result(change_set)
    }
}

struct WriteFileTool {
    service: PatchService,
}

#[derive(Deserialize)]
struct WriteFileArguments {
    path: String,
    content: String,
}

#[async_trait]
impl ToolHandler for WriteFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_file".to_string(),
            description: "Propose a complete UTF-8 file replacement for user review. Use apply_patch for normal localized edits; use write_file only when replacing the whole file is explicitly necessary."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "minLength": 1 },
                    "content": { "type": "string", "maxLength": 524288 }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        }
    }

    fn preview(
        &self,
        context: &ToolContext,
        arguments: &Value,
    ) -> Result<Option<PatchPreview>, ToolError> {
        let arguments: WriteFileArguments = serde_json::from_value(arguments.clone())
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        Ok(Some(self.service.preview_write_file(
            &context.workspace_root,
            &arguments.path,
            &arguments.content,
        )?))
    }

    async fn execute(
        &self,
        context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let arguments: WriteFileArguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let approval = context.approval.as_ref().ok_or_else(|| {
            ToolError::Denied("write_file requires a backend approval capability".to_string())
        })?;
        let change_set = self
            .service
            .apply_write_file(
                context.workspace_root.clone(),
                context.thread_id.clone(),
                context.turn_id.clone(),
                context.call_id.clone(),
                arguments.path,
                arguments.content,
                approval.expected_hashes.clone(),
            )
            .await?;
        change_result(change_set)
    }
}

fn change_result(change_set: ChangeSet) -> Result<ToolResult, ToolError> {
    let file_count = change_set.files.len();
    Ok(ToolResult {
        success: true,
        output: format!(
            "applied approved change {} to {file_count} file(s)",
            change_set.id
        ),
        metadata: json!({ "changeSet": change_set }),
    })
}

fn required_string<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments(format!("{name} must be a string")))
}

fn optional_usize(arguments: &Value, name: &str) -> Result<Option<usize>, ToolError> {
    arguments
        .get(name)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| ToolError::InvalidArguments(format!("{name} must be an integer")))
        })
        .transpose()
}

fn decode_text(bytes: &[u8]) -> Result<String, ToolError> {
    if bytes.starts_with(&[0xff, 0xfe]) {
        if !(bytes.len() - 2).is_multiple_of(2) {
            return Err(ToolError::Execution(
                "file contains truncated UTF-16LE".to_string(),
            ));
        }
        let words = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&words)
            .map_err(|_| ToolError::Execution("file contains invalid UTF-16LE".to_string()));
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        if !(bytes.len() - 2).is_multiple_of(2) {
            return Err(ToolError::Execution(
                "file contains truncated UTF-16BE".to_string(),
            ));
        }
        let words = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&words)
            .map_err(|_| ToolError::Execution("file contains invalid UTF-16BE".to_string()));
    }
    if bytes.iter().any(|byte| *byte == 0) {
        return Err(ToolError::Execution(
            "binary files are not supported".to_string(),
        ));
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| ToolError::Execution("file is not UTF-8 or BOM-marked UTF-16".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(root: &Path) -> ToolContext {
        ToolContext {
            thread_id: "thread".to_string(),
            turn_id: "turn".to_string(),
            call_id: "call".to_string(),
            workspace_root: root.to_path_buf(),
            approval: None,
        }
    }

    #[tokio::test]
    async fn registry_has_stable_sorted_names_and_rejects_bad_arguments() {
        let directory = tempfile::tempdir().unwrap();
        let registry = ToolRegistry::read_only();
        assert_eq!(
            registry
                .definitions()
                .into_iter()
                .map(|definition| definition.name)
                .collect::<Vec<_>>(),
            vec!["list_directory", "read_file"]
        );
        let result = registry
            .dispatch(
                &context(directory.path()),
                "read_file",
                json!({ "path": 2 }),
                CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
        let result = registry
            .dispatch(
                &context(directory.path()),
                "shell",
                json!({}),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(result, Err(ToolError::UnknownTool("shell".to_string())));
    }

    #[tokio::test]
    async fn rejects_absolute_parent_and_link_escape_paths() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        let registry = ToolRegistry::read_only();
        for path in [
            outside
                .path()
                .join("secret.txt")
                .to_string_lossy()
                .to_string(),
            "../secret.txt".to_string(),
        ] {
            let result = registry
                .dispatch(
                    &context(workspace.path()),
                    "read_file",
                    json!({ "path": path }),
                    CancellationToken::new(),
                )
                .await;
            assert!(matches!(result, Err(ToolError::Denied(_))));
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("escape")).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(outside.path(), workspace.path().join("escape"))
            .is_err()
        {
            return;
        }
        let result = registry
            .dispatch(
                &context(workspace.path()),
                "read_file",
                json!({ "path": "escape/secret.txt" }),
                CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(ToolError::Denied(_))));
    }

    #[tokio::test]
    async fn lists_with_ignore_and_limits_and_reads_utf16_with_bounds() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("node_modules")).unwrap();
        std::fs::write(workspace.path().join("b.txt"), "b").unwrap();
        std::fs::write(workspace.path().join("a.txt"), "a").unwrap();
        let utf16 = "hello world"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let mut encoded = vec![0xff, 0xfe];
        encoded.extend(utf16);
        std::fs::write(workspace.path().join("utf16.txt"), encoded).unwrap();
        let registry = ToolRegistry::read_only();
        let listed = registry
            .dispatch(
                &context(workspace.path()),
                "list_directory",
                json!({ "path": ".", "limit": 1 }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(listed.metadata["truncated"].as_bool().unwrap());
        assert!(!listed.output.contains("node_modules"));

        let read = registry
            .dispatch(
                &context(workspace.path()),
                "read_file",
                json!({ "path": "utf16.txt", "limit": 5 }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(read.output, "hello");
        assert_eq!(read.metadata["truncated"], true);
    }

    #[tokio::test]
    async fn cancelled_dispatch_does_not_execute() {
        let directory = tempfile::tempdir().unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = ToolRegistry::read_only()
            .dispatch(
                &context(directory.path()),
                "list_directory",
                json!({ "path": "." }),
                cancellation,
            )
            .await;
        assert_eq!(result, Err(ToolError::Cancelled));
    }

    #[tokio::test]
    async fn workspace_write_tools_are_reviewable_and_cannot_self_approve() {
        let workspace = tempfile::tempdir().unwrap();
        let registry = ToolRegistry::workspace_tools(PatchService::new());
        assert_eq!(
            registry
                .definitions()
                .into_iter()
                .map(|definition| definition.name)
                .collect::<Vec<_>>(),
            vec!["apply_patch", "list_directory", "read_file", "write_file"]
        );
        let patch = "*** Begin Patch\n*** Add File: added.txt\n+hello\n*** End Patch\n";
        let arguments = json!({ "patch": patch });
        let preview = registry
            .preview(&context(workspace.path()), "apply_patch", &arguments)
            .unwrap()
            .unwrap();
        assert_eq!(preview.files.len(), 1);
        assert!(!workspace.path().join("added.txt").exists());

        let result = registry
            .dispatch(
                &context(workspace.path()),
                "apply_patch",
                arguments.clone(),
                CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(ToolError::ApprovalRequired(_))));
        assert!(!workspace.path().join("added.txt").exists());

        let result = registry
            .dispatch_authorized(
                &context(workspace.path()),
                "apply_patch",
                arguments,
                CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(ToolError::Denied(_))));
        assert!(!workspace.path().join("added.txt").exists());
    }

    #[tokio::test]
    async fn model_arguments_cannot_include_approval_capabilities() {
        let workspace = tempfile::tempdir().unwrap();
        let registry = ToolRegistry::workspace_tools(PatchService::new());
        let result = registry
            .dispatch(
                &context(workspace.path()),
                "write_file",
                json!({
                    "path": "file.txt",
                    "content": "content",
                    "approved": true,
                    "expectedHashes": []
                }),
                CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
        assert!(!workspace.path().join("file.txt").exists());
    }

    #[tokio::test]
    async fn backend_approval_capability_allows_a_reviewed_full_file_replacement() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("file.txt"), "before\n").unwrap();
        let registry = ToolRegistry::workspace_tools(PatchService::new());
        let arguments = json!({ "path": "file.txt", "content": "after\n" });
        let preview = registry
            .preview(&context(workspace.path()), "write_file", &arguments)
            .unwrap()
            .unwrap();
        let mut approved_context = context(workspace.path());
        approved_context.approval = Some(ApprovedToolExecution {
            patch: None,
            selected_paths: vec!["file.txt".to_string()],
            expected_hashes: preview
                .files
                .iter()
                .map(|file| ExpectedFileHash {
                    path: file.path.clone(),
                    before_hash: file.before_hash.clone(),
                })
                .collect(),
        });

        let result = registry
            .dispatch_authorized(
                &approved_context,
                "write_file",
                arguments,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.metadata.get("changeSet").is_some());
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("file.txt")).unwrap(),
            "after\n"
        );
    }
}
