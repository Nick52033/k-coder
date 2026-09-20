//! 移动端安全投影。
//!
//! 手机端只接收「面向客户端的安全载荷」：正文、结构化摘要、工具名称、状态、耗时、
//! 脱敏错误、审批说明和必要的有界结果。这里统一负责剥掉：
//! - 工作区绝对路径（只给工作区相对路径）；
//! - 工具原始参数与元数据；
//! - 补丁正文、文件内容与统一 diff；
//! - 私有推理摘要；
//! - 图片 base64 载荷。

use serde::Serialize;

use crate::protocol::{
    AgentItemStatus, ApprovalResolution, ApprovalSnapshot, ChangeSet, FileOperation, ThreadItem,
    ThreadItemPayload, ThreadTurn, TurnState,
};
use crate::storage::ThreadSummary;
use crate::storage::{
    ToolActivitySnapshot, ToolActivityState, TurnTimelineItem, UserInputSnapshot,
};

use super::protocol::truncate_chars;

/// 单条工具结果摘要在手机上保留的最大字符数。
pub const MAX_TOOL_EXCERPT_CHARS: usize = 512;
/// 单条消息文本在手机上保留的最大字符数。
pub const MAX_MESSAGE_CHARS: usize = 8_000;

/// 手机端可见的项目。
///
/// 只下发展示所需的最小信息：项目 ID、显示名、归属键与最近打开时间。
/// **绝不包含 `path`**——工作区绝对路径与手机无关，且属于不该跨越设备边界的信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileProject {
    /// `projects` 表主键。
    pub id: String,
    /// 项目显示名（通常是目录名）。
    pub name: String,
    /// 项目归属键。会话用它指回项目，手机端也用它在重读之间保持分组展开态。
    ///
    /// 与桌面端 `workspacePathKey` 同源：取路径并统一大小写与分隔符，因此
    /// 手机端不需要、也拿不到真实路径。
    pub key: String,
    pub last_opened_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileThreadSummary {
    pub id: String,
    pub title: String,
    pub in_project: bool,
    pub archived: bool,
    pub updated_at_ms: u64,
    pub running: bool,
    pub active_turn_id: Option<String>,
    pub pending_approvals: u32,
    pub pending_user_inputs: u32,
    /// 会话所属项目的归属键，`None` 表示独立会话（`in_project = false`）。
    ///
    /// 桌面端的分组依赖 `sessions.workspace_path`，但它对「已注册但尚无绑定会话」
    /// 的项目和 `workspace_path` 仍为 NULL 的历史会话都不足以自洽；这里由服务端统一
    /// 解析后下发，手机端只做分组渲染，不再自行推断归属。
    pub project_key: Option<String>,
}

impl MobileThreadSummary {
    pub fn from_summary(
        summary: &ThreadSummary,
        project_key: Option<String>,
        active_turn_id: Option<String>,
        pending_approvals: u32,
        pending_user_inputs: u32,
    ) -> Self {
        Self {
            id: summary.id.clone(),
            title: summary.title.clone(),
            in_project: summary.in_project,
            archived: summary.archived,
            updated_at_ms: summary.updated_at_ms,
            running: active_turn_id.is_some(),
            active_turn_id,
            pending_approvals,
            pending_user_inputs,
            project_key,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileFileChange {
    /// 工作区相对路径。绝不向手机下发绝对路径。
    pub path: String,
    pub operation: FileOperation,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileChangeSummary {
    pub change_id: String,
    pub undone: bool,
    pub file_count: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub files: Vec<MobileFileChange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileToolActivity {
    pub call_id: String,
    pub name: String,
    pub state: ToolActivityState,
    pub success: Option<bool>,
    pub excerpt: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileApproval {
    pub request_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub reason: String,
    pub risk: crate::protocol::ToolRisk,
    pub auto_approved: bool,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub resolved: bool,
    /// 审批涉及的工作区相对路径。手机端据此判断影响面。
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileQuestion {
    pub question: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileUserInput {
    pub request_id: String,
    pub questions: Vec<MobileQuestion>,
    pub resolved: bool,
    pub created_at_ms: u64,
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileTodo {
    pub content: String,
    pub status: crate::protocol::TodoStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileItem {
    pub id: String,
    pub kind: &'static str,
    pub status: Option<AgentItemStatus>,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
    pub text: Option<String>,
    pub tool: Option<MobileToolActivity>,
    pub change: Option<MobileChangeSummary>,
    pub approval: Option<MobileApproval>,
    pub user_input: Option<MobileUserInput>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileTurn {
    pub id: String,
    pub state: TurnState,
    /// 脱敏并截断后的错误说明。
    pub error: Option<String>,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub items: Vec<MobileItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileThreadView {
    pub thread_id: String,
    pub title: String,
    pub updated_at_ms: u64,
    pub running: bool,
    pub active_turn_id: Option<String>,
    pub turns: Vec<MobileTurn>,
    pub todos: Vec<MobileTodo>,
    pub pending_approvals: Vec<MobileApproval>,
    pub pending_user_inputs: Vec<MobileUserInput>,
}

/// 统计统一 diff 的增删行数，不返回 diff 正文。
pub fn diff_stats(unified_diff: &str) -> (usize, usize) {
    let mut insertions = 0;
    let mut deletions = 0;
    for line in unified_diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            insertions += 1;
        } else if line.starts_with('-') {
            deletions += 1;
        }
    }
    (insertions, deletions)
}

pub fn project_change_set(change_set: &ChangeSet) -> MobileChangeSummary {
    let mut files = Vec::with_capacity(change_set.files.len());
    let mut insertions = 0;
    let mut deletions = 0;
    for file in &change_set.files {
        let (added, removed) = diff_stats(&file.unified_diff);
        insertions += added;
        deletions += removed;
        files.push(MobileFileChange {
            path: file.path.clone(),
            operation: file.operation,
            insertions: added,
            deletions: removed,
        });
    }
    MobileChangeSummary {
        change_id: change_set.id.clone(),
        undone: change_set.undone,
        file_count: files.len(),
        insertions,
        deletions,
        files,
    }
}

pub fn project_approval(snapshot: &ApprovalSnapshot) -> MobileApproval {
    let request = &snapshot.request;
    let paths = request
        .preview
        .as_ref()
        .map(|preview| {
            preview
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    MobileApproval {
        request_id: request.id.clone(),
        tool_call_id: request.tool_call_id.clone(),
        tool_name: request.tool_name.clone(),
        reason: request.reason.clone(),
        risk: request.risk,
        auto_approved: request.auto_approved,
        created_at_ms: request.created_at_ms,
        expires_at_ms: request.expires_at_ms,
        resolved: snapshot.resolution.is_some(),
        paths,
    }
}

pub fn project_user_input(snapshot: &UserInputSnapshot) -> MobileUserInput {
    MobileUserInput {
        request_id: snapshot.request.id.clone(),
        questions: snapshot
            .request
            .questions
            .iter()
            .map(|question| MobileQuestion {
                question: question.question.clone(),
                options: question.options.clone(),
            })
            .collect(),
        resolved: snapshot.resolution.is_some(),
        created_at_ms: snapshot.request.created_at_ms,
        expires_at_ms: snapshot.request.expires_at_ms,
    }
}

pub fn project_tool(activity: &ToolActivitySnapshot) -> MobileToolActivity {
    MobileToolActivity {
        call_id: activity.call.id.clone(),
        name: activity.call.name.clone(),
        state: activity.state.clone(),
        success: activity.result.as_ref().map(|result| result.success),
        excerpt: activity
            .result
            .as_ref()
            .map(|result| truncate_chars(&result.output, MAX_TOOL_EXCERPT_CHARS)),
        duration_ms: activity.duration_ms,
    }
}

/// 把一条领域条目投影成移动端条目。返回 `None` 表示该条目不下发。
pub fn project_item(item: &ThreadItem) -> Option<MobileItem> {
    let base = |kind: &'static str| MobileItem {
        id: item.id.clone(),
        kind,
        status: item.status,
        started_at_ms: item.started_at_ms,
        completed_at_ms: item.completed_at_ms,
        text: None,
        tool: None,
        change: None,
        approval: None,
        user_input: None,
    };

    let mut projected = match &item.payload {
        ThreadItemPayload::UserMessage { message } => {
            let mut entry = base("user_message");
            entry.text = Some(truncate_chars(&message.visible_text(), MAX_MESSAGE_CHARS));
            entry
        }
        ThreadItemPayload::AgentMessage { message, .. } => {
            let mut entry = base("agent_message");
            entry.text = Some(truncate_chars(&message.visible_text(), MAX_MESSAGE_CHARS));
            entry
        }
        // 私有推理不下发到手机。
        ThreadItemPayload::Reasoning { .. } => return None,
        ThreadItemPayload::Tool { activity } => {
            let mut entry = base("tool");
            entry.tool = Some(project_tool(activity));
            entry
        }
        ThreadItemPayload::Approval { approval } => {
            let mut entry = base("approval");
            entry.approval = Some(project_approval(approval));
            entry
        }
        ThreadItemPayload::UserInput { user_input } => {
            let mut entry = base("user_input");
            entry.user_input = Some(project_user_input(user_input));
            entry
        }
        ThreadItemPayload::Change { change_set } => {
            let mut entry = base("change");
            entry.change = Some(project_change_set(change_set));
            entry
        }
        ThreadItemPayload::ContextCompaction { .. } => base("context_compaction"),
        ThreadItemPayload::Event => base("event"),
    };

    // 时间线条目里的文本摘要补齐到 `text`，工具类条目已经在上面处理。
    if projected.text.is_none() && projected.kind != "tool" {
        let text: String = item
            .timeline_items
            .iter()
            .filter_map(|entry| match entry {
                TurnTimelineItem::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        if !text.trim().is_empty() {
            projected.text = Some(truncate_chars(&text, MAX_MESSAGE_CHARS));
        }
    }

    Some(projected)
}

pub fn project_turn(turn: &ThreadTurn) -> MobileTurn {
    MobileTurn {
        id: turn.id.clone(),
        state: turn.state,
        error: turn
            .error
            .as_ref()
            .map(|error| truncate_chars(&error.message, 400)),
        started_at_ms: turn.started_at_ms,
        completed_at_ms: turn.completed_at_ms,
        duration_ms: turn.duration_ms,
        items: turn.items.iter().filter_map(project_item).collect(),
    }
}

/// 审批动作在移动端的表述。`timed_out` 与 `cancelled` 只能由服务端产生。
pub fn approval_action_for_mobile(action: &str) -> Option<&'static str> {
    match action {
        "approved" => Some("approved"),
        "rejected" => Some("rejected"),
        "cancelled" => Some("cancelled"),
        _ => None,
    }
}

pub fn resolved_approval_count(approvals: &[ApprovalSnapshot]) -> u32 {
    approvals
        .iter()
        .filter(|snapshot| snapshot.resolution.is_none())
        .count() as u32
}

pub fn pending_user_input_count(inputs: &[UserInputSnapshot]) -> u32 {
    inputs
        .iter()
        .filter(|snapshot| snapshot.resolution.is_none())
        .count() as u32
}

pub fn resolution_is_pending(resolution: &Option<ApprovalResolution>) -> bool {
    resolution.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        ApprovalAction, ApprovalRequest, ApprovalResolution, ApprovalSnapshot, ChangeFileSnapshot,
        ChangeSet, ChatMessage, ContentBlock, ExpectedFileHash, MessageRole, PatchFilePreview,
        PatchPreview, ToolCall, ToolResult, ToolRisk,
    };

    fn assistant_message(text: &str) -> ChatMessage {
        ChatMessage {
            schema_version: 1,
            id: "message-1".to_string(),
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: text.to_string(),
                },
                ContentBlock::Image {
                    name: "shot.png".to_string(),
                    data_url: "data:image/png;base64,AAAA".to_string(),
                },
            ],
            created_at_ms: 0,
        }
    }

    fn tool_item(output: &str) -> ThreadItem {
        ThreadItem {
            schema_version: 1,
            id: "item-tool".to_string(),
            turn_id: Some("turn-1".to_string()),
            status: Some(AgentItemStatus::Completed),
            started_at_ms: Some(1),
            completed_at_ms: Some(2),
            timeline_items: Vec::new(),
            payload: ThreadItemPayload::Tool {
                activity: ToolActivitySnapshot {
                    turn_id: "turn-1".to_string(),
                    call: ToolCall {
                        id: "call-1".to_string(),
                        name: "read_file".to_string(),
                        arguments: serde_json::json!({ "path": "D:\\code\\secret.txt" }),
                        metadata: serde_json::json!({ "cwd": "D:\\code" }),
                    },
                    state: ToolActivityState::Completed,
                    result: Some(ToolResult {
                        success: true,
                        output: output.to_string(),
                        metadata: serde_json::json!({ "absolute": "D:\\code\\secret.txt" }),
                    }),
                    started_at_ms: Some(1),
                    completed_at_ms: Some(2),
                    duration_ms: Some(1),
                },
            },
        }
    }

    #[test]
    fn tool_projection_drops_arguments_and_bounds_output() {
        let long = "y".repeat(MAX_TOOL_EXCERPT_CHARS * 2);
        let item = tool_item(&long);
        let projected = project_item(&item).unwrap();
        let encoded = serde_json::to_string(&projected).unwrap();
        let tool = projected.tool.as_ref().unwrap();
        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.success, Some(true));
        assert_eq!(
            tool.excerpt.as_ref().unwrap().chars().count(),
            MAX_TOOL_EXCERPT_CHARS + 1
        );
        assert!(!encoded.contains("secret.txt"));
    }

    #[test]
    fn message_projection_drops_image_payload() {
        let item = ThreadItem {
            schema_version: 1,
            id: "item-message".to_string(),
            turn_id: None,
            status: None,
            started_at_ms: None,
            completed_at_ms: None,
            timeline_items: Vec::new(),
            payload: ThreadItemPayload::AgentMessage {
                message: assistant_message("hello"),
                phase: crate::protocol::AgentMessagePhase::FinalAnswer,
            },
        };
        let projected = project_item(&item).unwrap();
        assert_eq!(projected.text.as_deref(), Some("hello"));
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(!encoded.contains("data:image"));
        assert!(!encoded.contains("shot.png"));
    }

    #[test]
    fn reasoning_items_are_not_projected() {
        let item = ThreadItem {
            schema_version: 1,
            id: "item-reasoning".to_string(),
            turn_id: None,
            status: None,
            started_at_ms: None,
            completed_at_ms: None,
            timeline_items: Vec::new(),
            payload: ThreadItemPayload::Reasoning {
                summary: "private chain of thought".to_string(),
            },
        };
        assert!(project_item(&item).is_none());
    }

    #[test]
    fn change_projection_keeps_counts_not_content() {
        let change_set = ChangeSet {
            id: "change-1".to_string(),
            thread_id: "t1".to_string(),
            turn_id: "turn-1".to_string(),
            tool_call_id: "call-1".to_string(),
            created_at_ms: 0,
            undone: false,
            files: vec![ChangeFileSnapshot {
                path: "src/main.ts".to_string(),
                destination_path: None,
                operation: FileOperation::Modify,
                before_hash: None,
                after_hash: None,
                before_content: Some("const a = 1;".to_string()),
                after_content: Some("const a = 2;\nconst b = 3;".to_string()),
                unified_diff: "--- a\n+++ b\n-old\n+new\n+extra\n".to_string(),
            }],
        };
        let projected = project_change_set(&change_set);
        assert_eq!(projected.file_count, 1);
        assert_eq!(projected.insertions, 2);
        assert_eq!(projected.deletions, 1);
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(!encoded.contains("const a"));
        assert!(!encoded.contains("+new"));
    }

    #[test]
    fn approval_projection_keeps_paths_but_not_patch() {
        let snapshot = ApprovalSnapshot {
            request: ApprovalRequest {
                id: "approval-1".to_string(),
                thread_id: "t1".to_string(),
                turn_id: "turn-1".to_string(),
                tool_call_id: "call-1".to_string(),
                tool_name: "apply_patch".to_string(),
                reason: "writes outside workspace".to_string(),
                auto_approved: false,
                risk: ToolRisk::Write,
                arguments: serde_json::json!({ "token": "super-secret" }),
                preview: Some(PatchPreview {
                    patch: "*** Begin Patch\n*** Update File: src/main.ts\n".to_string(),
                    files: vec![PatchFilePreview {
                        path: "src/main.ts".to_string(),
                        destination_path: None,
                        operation: FileOperation::Modify,
                        before_hash: None,
                        after_hash: None,
                        before_content: Some("secret".to_string()),
                        after_content: Some("secret2".to_string()),
                        unified_diff: "--- a\n+++ b\n-old\n+new\n".to_string(),
                    }],
                    total_snapshot_bytes: 12,
                }),
                created_at_ms: 10,
                expires_at_ms: 20,
            },
            resolution: None,
        };
        let projected = project_approval(&snapshot);
        assert_eq!(projected.paths, vec!["src/main.ts".to_string()]);
        assert_eq!(projected.resolved, false);
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(!encoded.contains("super-secret"));
        assert!(!encoded.contains("Begin Patch"));
        assert!(!encoded.contains("secret2"));
    }

    #[test]
    fn approval_resolution_state_is_visible() {
        let snapshot = ApprovalSnapshot {
            request: ApprovalRequest {
                id: "approval-1".to_string(),
                thread_id: "t1".to_string(),
                turn_id: "turn-1".to_string(),
                tool_call_id: "call-1".to_string(),
                tool_name: "run_command".to_string(),
                reason: "external".to_string(),
                auto_approved: false,
                risk: ToolRisk::External,
                arguments: serde_json::Value::Null,
                preview: None,
                created_at_ms: 10,
                expires_at_ms: 20,
            },
            resolution: Some(ApprovalResolution {
                action: ApprovalAction::Rejected,
                patch: None,
                selected_paths: Vec::new(),
                expected_hashes: Vec::<ExpectedFileHash>::new(),
            }),
        };
        assert!(project_approval(&snapshot).resolved);
        assert_eq!(resolved_approval_count(std::slice::from_ref(&snapshot)), 0);
    }

    #[test]
    fn diff_stats_ignore_file_headers() {
        let (insertions, deletions) = diff_stats("--- a\n+++ b\n+one\n+two\n-three\n context\n");
        assert_eq!((insertions, deletions), (2, 1));
    }

    #[test]
    fn pending_user_input_projection_preserves_absent_and_legacy_expiry() {
        for expiry in [None, Some(20)] {
            let snapshot = UserInputSnapshot {
                request: crate::protocol::UserInputRequest {
                    id: "input-1".into(),
                    thread_id: "thread-1".into(),
                    turn_id: "turn-1".into(),
                    tool_call_id: "call-1".into(),
                    kind: crate::protocol::UserInputRequestKind::ModelQuestion,
                    questions: vec![crate::protocol::UserInputQuestion {
                        question: "Choose an approach".into(),
                        options: vec!["Conservative".into(), "Fast".into()],
                    }],
                    created_at_ms: 1,
                    expires_at_ms: expiry,
                },
                resolution: None,
            };
            let projected = project_user_input(&snapshot);
            assert!(!projected.resolved);
            assert_eq!(projected.expires_at_ms, expiry);
            let value = serde_json::to_value(projected).unwrap();
            assert_eq!(value["expiresAtMs"], serde_json::json!(expiry));
            assert_eq!(value["questions"][0]["question"], "Choose an approach");
        }
    }

    #[test]
    fn summary_projection_reports_running_state() {
        let summary = ThreadSummary {
            schema_version: 1,
            id: "t1".to_string(),
            title: "Fix build".to_string(),
            created_at_ms: 0,
            updated_at_ms: 5,
            archived: false,
            in_project: true,
            workspace_path: Some("D:\\code\\k-coder".to_string()),
        };
        let projected = MobileThreadSummary::from_summary(
            &summary,
            Some("d:/code/k-coder".to_string()),
            Some("turn-9".to_string()),
            1,
            2,
        );
        assert!(projected.running);
        assert_eq!(projected.pending_approvals, 1);
        assert_eq!(projected.project_key.as_deref(), Some("d:/code/k-coder"));
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(
            !encoded.contains("D:\\\\code"),
            "workspace absolute path must not reach the phone: {encoded}"
        );
    }

    /// 项目键是路径的归一化形式，不是路径本身。手机端拿它分组即可，
    /// 不需要（也不应该）知道工作区在磁盘上的位置。
    #[test]
    fn project_key_is_not_the_workspace_path() {
        let project = MobileProject {
            id: "p1".to_string(),
            name: "k-coder".to_string(),
            key: crate::workbench::workspace_path_key(r"D:\code\Nick\k-coder"),
            last_opened_at_ms: 7,
        };

        let encoded = serde_json::to_string(&project).unwrap();
        assert!(
            !encoded.contains(r"D:\\"),
            "key 不得携带盘符路径: {encoded}"
        );
        assert!(!encoded.contains("Nick"), "key 不得携带父目录名: {encoded}");
        let decoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded["name"], "k-coder");
        assert!(decoded.get("path").is_none(), "项目视图不得下发 path 字段");
    }
}
