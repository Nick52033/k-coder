use std::collections::{HashMap, hash_map::Entry};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use crate::execution::{CommandMode, CommandRuntime, StartCommandRequest};
use crate::protocol::{ApprovalAction, ApprovalResolution, ToolRisk};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    RequireApproval { reason: String },
    Deny { reason: String },
}

pub trait PolicyEngine: Send + Sync {
    fn authorize(&self, tool_name: &str, arguments: &serde_json::Value) -> PolicyDecision;
    fn risk(&self, tool_name: &str, arguments: &serde_json::Value) -> ToolRisk;
}

pub struct AllowRegisteredTools;

impl PolicyEngine for AllowRegisteredTools {
    fn authorize(&self, _tool_name: &str, _arguments: &serde_json::Value) -> PolicyDecision {
        PolicyDecision::Allow
    }

    fn risk(&self, _tool_name: &str, _arguments: &serde_json::Value) -> ToolRisk {
        ToolRisk::Read
    }
}

pub struct ReadOnlyWorkspacePolicy;

impl PolicyEngine for ReadOnlyWorkspacePolicy {
    fn authorize(&self, tool_name: &str, _arguments: &serde_json::Value) -> PolicyDecision {
        match tool_name {
            "list_directory" | "read_file" => PolicyDecision::Allow,
            _ => PolicyDecision::Deny {
                reason: "only built-in workspace read tools are allowed".to_string(),
            },
        }
    }

    fn risk(&self, tool_name: &str, _arguments: &serde_json::Value) -> ToolRisk {
        match tool_name {
            "list_directory" | "read_file" => ToolRisk::Read,
            _ => ToolRisk::External,
        }
    }
}

pub struct WorkspacePolicy;

impl PolicyEngine for WorkspacePolicy {
    fn authorize(&self, tool_name: &str, _arguments: &serde_json::Value) -> PolicyDecision {
        match tool_name {
            "list_directory" | "read_file" => PolicyDecision::Allow,
            "apply_patch" => PolicyDecision::RequireApproval {
                reason: "review the proposed patch before changing workspace files".to_string(),
            },
            "write_file" => PolicyDecision::RequireApproval {
                reason: "review the complete file replacement before writing".to_string(),
            },
            _ => PolicyDecision::Deny {
                reason: "the tool is not allowed by the workspace policy".to_string(),
            },
        }
    }

    fn risk(&self, tool_name: &str, arguments: &serde_json::Value) -> ToolRisk {
        match tool_name {
            "list_directory" | "read_file" => ToolRisk::Read,
            "write_file" => ToolRisk::Write,
            "apply_patch"
                if arguments
                    .get("patch")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|patch| patch.contains("*** Delete File: ")) =>
            {
                ToolRisk::Delete
            }
            "apply_patch" => ToolRisk::Write,
            "run_command" => ToolRisk::External,
            _ => ToolRisk::External,
        }
    }
}

pub struct ExecutionWorkspacePolicy {
    pub runtime: CommandRuntime,
}

impl PolicyEngine for ExecutionWorkspacePolicy {
    fn authorize(&self, tool_name: &str, arguments: &serde_json::Value) -> PolicyDecision {
        if tool_name != "run_command" {
            return WorkspacePolicy.authorize(tool_name, arguments);
        }
        let request = StartCommandRequest {
            program: arguments
                .get("program")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            args: arguments
                .get("args")
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            cwd: arguments
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            env: HashMap::new(),
            mode: CommandMode::Foreground,
            timeout_ms: arguments
                .get("timeoutMs")
                .and_then(serde_json::Value::as_u64),
            buffer_bytes: None,
        };
        let assessment = self.runtime.assess(&request);
        if assessment.requires_approval {
            PolicyDecision::RequireApproval {
                reason: assessment.reason,
            }
        } else {
            PolicyDecision::Allow
        }
    }

    fn risk(&self, tool_name: &str, arguments: &serde_json::Value) -> ToolRisk {
        if tool_name == "run_command" {
            ToolRisk::External
        } else {
            WorkspacePolicy.risk(tool_name, arguments)
        }
    }
}

#[derive(Clone)]
pub struct ApprovalManager {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ApprovalResolution>>>>,
    timeout: Duration,
}

impl ApprovalManager {
    pub fn new(timeout: Duration) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            timeout,
        }
    }

    pub async fn register(
        &self,
        request_id: &str,
    ) -> Result<oneshot::Receiver<ApprovalResolution>, ApprovalError> {
        let (sender, receiver) = oneshot::channel();
        match self.pending.lock().await.entry(request_id.to_string()) {
            Entry::Vacant(entry) => {
                entry.insert(sender);
            }
            Entry::Occupied(_) => {
                return Err(ApprovalError::Duplicate(request_id.to_string()));
            }
        }
        Ok(receiver)
    }

    pub async fn wait(
        &self,
        request_id: &str,
        receiver: oneshot::Receiver<ApprovalResolution>,
        cancellation: CancellationToken,
    ) -> Result<ApprovalResolution, ApprovalError> {
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err(ApprovalError::Cancelled),
            _ = tokio::time::sleep(self.timeout) => Ok(ApprovalResolution {
                action: ApprovalAction::TimedOut,
                patch: None,
                selected_paths: Vec::new(),
                expected_hashes: Vec::new(),
            }),
            resolution = receiver => resolution.map_err(|_| ApprovalError::Closed),
        };
        self.pending.lock().await.remove(request_id);
        result
    }

    pub async fn resolve(
        &self,
        request_id: &str,
        resolution: ApprovalResolution,
    ) -> Result<(), ApprovalError> {
        let sender = self
            .pending
            .lock()
            .await
            .remove(request_id)
            .ok_or_else(|| ApprovalError::NotFound(request_id.to_string()))?;
        sender.send(resolution).map_err(|_| ApprovalError::Closed)
    }

    pub async fn discard(&self, request_id: &str) {
        self.pending.lock().await.remove(request_id);
    }

    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout.as_millis().min(u64::MAX as u128) as u64
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApprovalError {
    #[error("approval request already exists: {0}")]
    Duplicate(String),
    #[error("approval request was not found: {0}")]
    NotFound(String),
    #[error("approval request channel closed")]
    Closed,
    #[error("approval wait was cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_policy_classifies_tool_risks_and_requires_write_approval() {
        let policy = WorkspacePolicy;
        assert_eq!(
            policy.risk("read_file", &serde_json::json!({})),
            ToolRisk::Read
        );
        assert_eq!(
            policy.risk("write_file", &serde_json::json!({})),
            ToolRisk::Write
        );
        assert_eq!(
            policy.risk(
                "apply_patch",
                &serde_json::json!({ "patch": "*** Delete File: old.txt" }),
            ),
            ToolRisk::Delete
        );
        assert_eq!(
            policy.risk("network", &serde_json::json!({})),
            ToolRisk::External
        );
        assert!(matches!(
            policy.authorize("apply_patch", &serde_json::json!({})),
            PolicyDecision::RequireApproval { .. }
        ));
    }

    #[tokio::test]
    async fn duplicate_registration_does_not_close_the_original_request() {
        let manager = ApprovalManager::new(Duration::from_secs(1));
        let receiver = manager.register("same").await.unwrap();
        assert_eq!(
            manager.register("same").await.unwrap_err(),
            ApprovalError::Duplicate("same".to_string())
        );
        let resolution = ApprovalResolution {
            action: ApprovalAction::Approved,
            patch: None,
            selected_paths: Vec::new(),
            expected_hashes: Vec::new(),
        };
        manager.resolve("same", resolution.clone()).await.unwrap();
        assert_eq!(
            manager
                .wait("same", receiver, CancellationToken::new())
                .await
                .unwrap(),
            resolution
        );
    }

    #[tokio::test]
    async fn cancellation_removes_a_pending_request() {
        let manager = ApprovalManager::new(Duration::from_secs(1));
        let receiver = manager.register("cancelled").await.unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            manager
                .wait("cancelled", receiver, cancellation)
                .await
                .unwrap_err(),
            ApprovalError::Cancelled
        );
        assert_eq!(manager.pending_count().await, 0);
    }

    #[tokio::test]
    async fn discarding_a_request_closes_it_without_a_resolution() {
        let manager = ApprovalManager::new(Duration::from_secs(1));
        let receiver = manager.register("discarded").await.unwrap();
        manager.discard("discarded").await;
        assert_eq!(manager.pending_count().await, 0);
        assert!(receiver.await.is_err());
    }

    #[tokio::test]
    async fn resolves_a_registered_approval_once() {
        let manager = ApprovalManager::new(Duration::from_secs(1));
        let receiver = manager.register("approval").await.unwrap();
        manager
            .resolve(
                "approval",
                ApprovalResolution {
                    action: ApprovalAction::Rejected,
                    patch: None,
                    selected_paths: Vec::new(),
                    expected_hashes: Vec::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            manager
                .wait("approval", receiver, CancellationToken::new())
                .await
                .unwrap()
                .action,
            ApprovalAction::Rejected
        );
        assert!(matches!(
            manager
                .resolve(
                    "approval",
                    ApprovalResolution {
                        action: ApprovalAction::Approved,
                        patch: None,
                        selected_paths: Vec::new(),
                        expected_hashes: Vec::new(),
                    }
                )
                .await,
            Err(ApprovalError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn times_out_without_a_resolution() {
        let manager = ApprovalManager::new(Duration::from_millis(5));
        let receiver = manager.register("approval").await.unwrap();
        let resolution = manager
            .wait("approval", receiver, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resolution.action, ApprovalAction::TimedOut);
        assert_eq!(manager.pending_count().await, 0);
    }
}
