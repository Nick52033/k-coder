use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    RequireApproval { reason: String },
    Deny { reason: String },
}

pub trait PolicyEngine: Send + Sync {
    fn authorize(&self, tool_name: &str, arguments: &serde_json::Value) -> PolicyDecision;
}
