use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Queued,
    Streaming,
    AwaitingApproval,
    RunningTool,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRequest {
    pub thread_id: String,
    pub input: String,
    pub workspace_root: String,
}

// 可执行循环将在路线图 Phase 1 中实现。领域类型单独放在这里，避免把
// Tauri 命令或 UI 载荷变成运行时核心接口。
