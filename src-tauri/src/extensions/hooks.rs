use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::execution::{configure_process_group, terminate_tree};
use crate::logging::StructuredLogger;
use crate::protocol::ToolResult;
use crate::tools::{ToolContext, ToolError, ToolHookRunner};

const MAX_HOOK_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_HOOK_INPUT_BYTES: usize = 512 * 1024;
const DEFAULT_HOOK_TIMEOUT_MS: u64 = 10_000;
const MAX_HOOK_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookPhase {
    Before,
    After,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HookConfig {
    pub id: String,
    pub phase: HookPhase,
    pub tool: String,
    pub command: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_timeout() -> u64 {
    DEFAULT_HOOK_TIMEOUT_MS
}

fn default_true() -> bool {
    true
}

impl HookConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || self.id.len() > 80 {
            return Err("hook id must contain 1 to 80 characters".into());
        }
        if self.tool.trim().is_empty() || self.tool.len() > 160 {
            return Err(format!("hook {} has an invalid tool pattern", self.id));
        }
        if self.command.is_empty()
            || self.command.len() > 64
            || self.command.iter().any(|value| value.len() > 8192)
        {
            return Err(format!(
                "hook {} has an invalid structured command",
                self.id
            ));
        }
        if self.timeout_ms == 0 || self.timeout_ms > MAX_HOOK_TIMEOUT_MS {
            return Err(format!(
                "hook {} timeout must be between 1 and {MAX_HOOK_TIMEOUT_MS} ms",
                self.id
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum HookDecision {
    Allow,
    Block,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HookResponse {
    decision: HookDecision,
    #[serde(default)]
    message: String,
    output: Option<String>,
}

#[derive(Clone)]
pub struct HookPipeline {
    hooks: Arc<Vec<HookConfig>>,
    workspace_root: PathBuf,
    logger: StructuredLogger,
}

impl HookPipeline {
    pub fn new(
        mut hooks: Vec<HookConfig>,
        workspace_root: PathBuf,
        logger: StructuredLogger,
    ) -> Result<Self, String> {
        for hook in &hooks {
            hook.validate()?;
        }
        hooks.retain(|hook| hook.enabled);
        hooks.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(Self {
            hooks: Arc::new(hooks),
            workspace_root,
            logger,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    async fn run(
        &self,
        phase: HookPhase,
        context: &ToolContext,
        name: &str,
        arguments: &Value,
        result: Option<&ToolResult>,
        cancellation: CancellationToken,
    ) -> Result<Option<String>, ToolError> {
        let mut replacement = None;
        for hook in self.hooks.iter().filter(|hook| {
            matches!(hook.phase, HookPhase::Both)
                || std::mem::discriminant(&hook.phase) == std::mem::discriminant(&phase)
        }) {
            if !matches_pattern(&hook.tool, name) {
                continue;
            }
            let payload = serde_json::json!({
                "schemaVersion": 1,
                "phase": match phase { HookPhase::Before => "before", HookPhase::After => "after", HookPhase::Both => unreachable!() },
                "threadId": context.thread_id,
                "turnId": context.turn_id,
                "tool": name,
                "arguments": arguments,
                "result": result,
            });
            let response =
                run_hook_process(hook, &self.workspace_root, &payload, cancellation.clone())
                    .await?;
            let _ = self.logger.log(
                "info",
                "extension_hook_completed",
                serde_json::json!({
                    "hookId": hook.id,
                    "tool": name,
                    "phase": match phase { HookPhase::Before => "before", HookPhase::After => "after", HookPhase::Both => unreachable!() },
                    "decision": match response.decision { HookDecision::Allow => "allow", HookDecision::Block => "block" },
                }),
            );
            if matches!(response.decision, HookDecision::Block) {
                return Err(ToolError::Denied(format!(
                    "hook {} blocked execution: {}",
                    hook.id,
                    if response.message.is_empty() {
                        "no reason provided"
                    } else {
                        response.message.as_str()
                    }
                )));
            }
            if response.output.is_some() {
                replacement = response.output;
            }
        }
        Ok(replacement)
    }
}

#[async_trait]
impl ToolHookRunner for HookPipeline {
    async fn before(
        &self,
        context: &ToolContext,
        name: &str,
        arguments: &Value,
        cancellation: CancellationToken,
    ) -> Result<(), ToolError> {
        self.run(
            HookPhase::Before,
            context,
            name,
            arguments,
            None,
            cancellation,
        )
        .await?;
        Ok(())
    }

    async fn after(
        &self,
        context: &ToolContext,
        name: &str,
        arguments: &Value,
        mut result: ToolResult,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if let Some(output) = self
            .run(
                HookPhase::After,
                context,
                name,
                arguments,
                Some(&result),
                cancellation,
            )
            .await?
        {
            result.output = output;
        }
        Ok(result)
    }
}

async fn run_hook_process(
    hook: &HookConfig,
    workspace_root: &PathBuf,
    payload: &Value,
    cancellation: CancellationToken,
) -> Result<HookResponse, ToolError> {
    let mut command = Command::new(&hook.command[0]);
    command
        .args(&hook.command[1..])
        .current_dir(workspace_root)
        .env_clear()
        .envs(safe_process_environment())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(|error| {
        ToolError::Execution(format!("hook {} failed to start: {error}", hook.id))
    })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ToolError::Execution(format!("hook {} stdin is unavailable", hook.id)))?;
    let bytes = serde_json::to_vec(payload)
        .map_err(|error| ToolError::Execution(format!("hook payload failed: {error}")))?;
    if bytes.len() > MAX_HOOK_INPUT_BYTES {
        return Err(ToolError::Denied(format!(
            "hook {} input exceeded {MAX_HOOK_INPUT_BYTES} bytes",
            hook.id
        )));
    }
    stdin
        .write_all(&bytes)
        .await
        .map_err(|error| ToolError::Execution(format!("hook {} input failed: {error}", hook.id)))?;
    drop(stdin);
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::Execution(format!("hook {} stdout is unavailable", hook.id)))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::Execution(format!("hook {} stderr is unavailable", hook.id)))?;
    let stdout_task = tokio::spawn(read_bounded(stdout, MAX_HOOK_OUTPUT_BYTES));
    let stderr_task = tokio::spawn(read_bounded(stderr, 4096));
    let pid = child
        .id()
        .ok_or_else(|| ToolError::Execution(format!("hook {} has no process id", hook.id)))?;
    let output = tokio::select! {
        _ = cancellation.cancelled() => {
            terminate_tree(pid).await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(ToolError::Cancelled);
        },
        _ = tokio::time::sleep(Duration::from_millis(hook.timeout_ms)) => {
            terminate_tree(pid).await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(ToolError::Denied(format!("hook {} timed out", hook.id)));
        },
        status = child.wait() => {
            status.map_err(|error| ToolError::Execution(format!("hook {} failed: {error}", hook.id)))?
        }
    };
    let stdout = stdout_task.await.map_err(|error| {
        ToolError::Execution(format!("hook {} output task failed: {error}", hook.id))
    })??;
    let stderr = stderr_task.await.map_err(|error| {
        ToolError::Execution(format!("hook {} error task failed: {error}", hook.id))
    })??;
    if !output.success() {
        return Err(ToolError::Denied(format!(
            "hook {} exited with {}: {}",
            hook.id,
            output,
            bounded_utf8(&stderr)
        )));
    }
    serde_json::from_slice(&stdout).map_err(|error| {
        ToolError::Denied(format!(
            "hook {} returned an invalid decision: {error}",
            hook.id
        ))
    })
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, ToolError> {
    let mut result = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        if read == 0 {
            return Ok(result);
        }
        if result.len().saturating_add(read) > limit {
            return Err(ToolError::Denied(format!(
                "hook output exceeded {limit} bytes"
            )));
        }
        result.extend_from_slice(&buffer[..read]);
    }
}

fn safe_process_environment() -> HashMap<String, String> {
    [
        "PATH",
        "PATHEXT",
        "SYSTEMROOT",
        "WINDIR",
        "HOME",
        "TMP",
        "TEMP",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
    .collect()
}

fn matches_pattern(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match pattern.split_once('*') {
        Some((prefix, suffix)) => value.starts_with(prefix) && value.ends_with(suffix),
        None => pattern == value,
    }
}

fn bounded_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(4096)])
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn validates_hook_contracts_and_patterns() {
        let hook = HookConfig {
            id: "guard".into(),
            phase: HookPhase::Before,
            tool: "mcp__github__*".into(),
            command: vec!["guard".into()],
            timeout_ms: 1000,
            enabled: true,
        };
        assert!(hook.validate().is_ok());
        assert!(matches_pattern("mcp__github__*", "mcp__github__issue"));
        assert!(!matches_pattern("mcp__github__*", "read_file"));
    }

    #[tokio::test]
    async fn hook_pipeline_blocks_and_fails_closed_on_invalid_output() {
        let workspace = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(workspace.path()).unwrap();
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test-fixtures")
            .join("hook.mjs");
        let context = ToolContext {
            thread_id: "thread".into(),
            turn_id: "turn".into(),
            call_id: "call".into(),
            workspace_root: workspace.path().into(),
            approval: None,
        };
        for mode in ["block", "invalid"] {
            let pipeline = HookPipeline::new(
                vec![HookConfig {
                    id: format!("{mode}-hook"),
                    phase: HookPhase::Before,
                    tool: "mcp__*".into(),
                    command: vec![
                        "node".into(),
                        fixture.to_string_lossy().to_string(),
                        mode.into(),
                    ],
                    timeout_ms: 10_000,
                    enabled: true,
                }],
                workspace.path().into(),
                logger.clone(),
            )
            .unwrap();
            assert!(
                pipeline
                    .before(
                        &context,
                        "mcp__fixture__tool",
                        &serde_json::json!({}),
                        CancellationToken::new(),
                    )
                    .await
                    .is_err()
            );
        }
    }
}
