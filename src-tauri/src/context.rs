use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::protocol::{MessageRole, ToolDefinition};
use crate::providers::ProviderMessage;

pub const DEFAULT_CONTEXT_LIMIT: usize = 128_000;
/// The model's advertised context window remains the hard provider boundary.
/// Compaction uses this smaller working budget so long Turns do not run at the
/// edge of the provider window on every request.
pub const DEFAULT_WORKING_CONTEXT_LIMIT: usize = 96_000;
const MIN_WORKING_CONTEXT_LIMIT: usize = 16_384;
pub const AUTO_COMPACT_THRESHOLD_PERCENT: usize = 65;
const CHARS_PER_TOKEN: usize = 4;
const RECENT_TOOL_RESULT_LIMIT: usize = 2;
const RECENT_USER_MESSAGE_LIMIT: usize = 4;
const RECENT_USER_MESSAGE_BYTES: usize = 800;
const CURRENT_USER_REQUEST_BYTES: usize = 2_000;
const IMPORTANT_TOOL_OBSERVATION_LIMIT: usize = 12;
const IMPORTANT_TOOL_OBSERVATION_BYTES: usize = 700;
const LARGE_TOOL_OUTPUT_BYTES: usize = 4 * 1_024;
const TOOL_OUTPUT_PREVIEW_BYTES: usize = 1_500;

#[derive(Debug, Clone, Copy)]
pub struct ContextBudget {
    pub system: usize,
    pub rules: usize,
    pub history: usize,
    pub tools: usize,
    pub user_input: usize,
}

impl ContextBudget {
    pub fn for_limit(limit: usize) -> Self {
        Self {
            system: limit / 10,
            rules: limit / 10,
            history: limit * 55 / 100,
            tools: limit * 15 / 100,
            user_input: limit / 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummary {
    pub contract_version: u32,
    pub summary: String,
    pub user_constraints: Vec<String>,
    #[serde(default)]
    pub recent_user_messages: Vec<String>,
    #[serde(default)]
    pub current_user_request: String,
    #[serde(default)]
    pub important_tool_observations: Vec<String>,
    pub recent_tool_results: Vec<ProviderMessage>,
    pub compacted_message_count: usize,
    #[serde(default)]
    pub estimated_before_tokens: usize,
    #[serde(default)]
    pub estimated_after_tokens: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// User intent reconstructed only from persisted `UserMessage` events.
/// Rendered compaction summaries must never be observed by this accumulator.
pub struct CompactionUserContext {
    current_user_request: String,
    recent_user_messages: Vec<String>,
    user_constraints: Vec<String>,
}

impl CompactionUserContext {
    pub fn from_messages(messages: &[ProviderMessage]) -> Self {
        let mut context = Self::default();
        for message in messages {
            if let Some(text) = user_message_text(message) {
                context.observe(text);
            }
        }
        context
    }

    pub fn observe(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        self.current_user_request = bound(&text, CURRENT_USER_REQUEST_BYTES);
        self.recent_user_messages
            .push(bound(&text, RECENT_USER_MESSAGE_BYTES));
        if self.recent_user_messages.len() > RECENT_USER_MESSAGE_LIMIT {
            self.recent_user_messages
                .drain(..self.recent_user_messages.len() - RECENT_USER_MESSAGE_LIMIT);
        }
        if is_constraint(&text) {
            self.user_constraints = merge_string_history(
                self.user_constraints
                    .iter()
                    .cloned()
                    .chain(std::iter::once(bound(&text, 600))),
                8,
            );
        }
    }

    fn is_empty(&self) -> bool {
        self.current_user_request.is_empty()
            && self.recent_user_messages.is_empty()
            && self.user_constraints.is_empty()
    }
}

pub fn default_working_context_limit(hard_limit: usize) -> usize {
    let hard_limit = hard_limit.max(1_024);
    hard_limit
        .min(DEFAULT_WORKING_CONTEXT_LIMIT)
        .max(hard_limit.min(MIN_WORKING_CONTEXT_LIMIT))
}

pub fn normalize_working_context_limit(hard_limit: usize, requested: usize) -> usize {
    let hard_limit = hard_limit.max(1_024);
    let floor = hard_limit.min(MIN_WORKING_CONTEXT_LIMIT);
    requested.max(floor).min(hard_limit)
}

pub fn estimate_tokens(messages: &[ProviderMessage]) -> usize {
    messages
        .iter()
        .map(message_chars)
        .sum::<usize>()
        .div_ceil(CHARS_PER_TOKEN)
}

pub fn estimate_provider_request_tokens(
    messages: &[ProviderMessage],
    runtime_instructions: &str,
    tools: &[ToolDefinition],
) -> usize {
    let message_tokens = estimate_tokens(messages);
    let instruction_tokens = runtime_instructions.len().div_ceil(CHARS_PER_TOKEN);
    let tool_tokens = serde_json::to_vec(tools)
        .map(|serialized| serialized.len().div_ceil(CHARS_PER_TOKEN))
        .unwrap_or_default();
    message_tokens
        .saturating_add(instruction_tokens)
        .saturating_add(tool_tokens)
}

pub fn needs_compaction(messages: &[ProviderMessage], limit: usize) -> bool {
    estimate_tokens(messages) * 100 >= limit * AUTO_COMPACT_THRESHOLD_PERCENT
}

pub fn needs_compaction_for_usage(total_tokens: u64, limit: usize) -> bool {
    total_tokens.saturating_mul(100)
        >= (limit as u64).saturating_mul(AUTO_COMPACT_THRESHOLD_PERCENT as u64)
}

pub fn needs_compaction_for_request(
    messages: &[ProviderMessage],
    runtime_instructions: &str,
    tools: &[ToolDefinition],
    limit: usize,
) -> bool {
    let estimated = estimate_provider_request_tokens(messages, runtime_instructions, tools);
    estimated.saturating_mul(100) >= limit.saturating_mul(AUTO_COMPACT_THRESHOLD_PERCENT)
}

pub fn compact(
    messages: &[ProviderMessage],
    limit: usize,
    previous_summary: Option<&CompactionSummary>,
    authoritative_user_context: &CompactionUserContext,
) -> (CompactionSummary, Vec<ProviderMessage>) {
    let budget = ContextBudget::for_limit(limit);
    let observed_user_context;
    let user_context = if authoritative_user_context.is_empty() {
        observed_user_context = CompactionUserContext::from_messages(messages);
        &observed_user_context
    } else {
        authoritative_user_context
    };
    let previous_summary = previous_summary
        .cloned()
        .map(|summary| normalize_compaction_summary(summary, user_context));
    let previous_summary = previous_summary.as_ref();
    let recent_tool_results = previous_summary
        .into_iter()
        .flat_map(|summary| summary.recent_tool_results.iter())
        .chain(messages.iter())
        .filter(|message| matches!(message, ProviderMessage::ToolResult { .. }))
        .map(summarize_large_tool_result)
        .collect::<Vec<_>>();
    let recent_tool_results = recent_tool_results[recent_tool_results
        .len()
        .saturating_sub(RECENT_TOOL_RESULT_LIMIT)..]
        .to_vec();
    let user_constraints = merge_string_history(
        previous_summary
            .into_iter()
            .flat_map(|summary| summary.user_constraints.iter().cloned())
            .chain(user_context.user_constraints.iter().cloned()),
        8,
    );
    let recent_user_messages = if user_context.recent_user_messages.is_empty() {
        previous_summary
            .map(|summary| summary.recent_user_messages.clone())
            .unwrap_or_default()
    } else {
        user_context.recent_user_messages.clone()
    };
    // Leave a low-water mark after compaction. Keeping only a quarter of the
    // history budget leaves room for the summary, system instructions, tools,
    // and the next response without immediately retriggering compaction.
    let keep_tokens = budget.history / 4;
    let mut kept = Vec::new();
    let mut used = 0;
    for message in messages.iter().rev() {
        let tokens = message_chars(message).div_ceil(CHARS_PER_TOKEN);
        if used + tokens > keep_tokens && !kept.is_empty() {
            break;
        }
        kept.push(summarize_large_tool_result(message));
        used += tokens;
    }
    kept.reverse();
    let compacted_count = messages.len().saturating_sub(kept.len());
    let current_user_request = if user_context.current_user_request.is_empty() {
        previous_summary
            .map(|summary| summary.current_user_request.clone())
            .unwrap_or_default()
    } else {
        user_context.current_user_request.clone()
    };
    let important_tool_observations = merge_string_history(
        previous_summary
            .into_iter()
            .flat_map(|summary| summary.important_tool_observations.iter().cloned())
            .chain(important_tool_observations(&messages[..compacted_count])),
        IMPORTANT_TOOL_OBSERVATION_LIMIT,
    );
    let new_summary_text = messages[..compacted_count]
        .iter()
        .filter_map(|message| match message {
            ProviderMessage::Text { role, text } => {
                Some(format!("{:?}: {}", role, bound(text, 500)))
            }
            ProviderMessage::UserContent { text, images } => Some(format!(
                "User: {} [{} image attachment(s)]",
                bound(text, 500),
                images.len()
            )),
            ProviderMessage::ToolResult {
                name,
                success,
                output,
                ..
            } => Some(format!("tool {name} ({success}): {}", bound(output, 300))),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let summary_text = previous_summary
        .and_then(stable_previous_summary_text)
        .into_iter()
        .chain((!new_summary_text.is_empty()).then_some(new_summary_text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    let mut summary = CompactionSummary {
        contract_version: 4,
        summary: bound(&summary_text, budget.history * CHARS_PER_TOKEN / 4),
        user_constraints,
        recent_user_messages,
        current_user_request,
        important_tool_observations,
        recent_tool_results,
        compacted_message_count: compacted_count,
        estimated_before_tokens: estimate_tokens(&render_provider_history(
            previous_summary,
            messages,
        )),
        estimated_after_tokens: 0,
    };
    let result = render_provider_history(Some(&summary), &kept);
    summary.estimated_after_tokens = estimate_tokens(&result);
    (summary, result)
}

pub(crate) fn render_provider_history(
    summary: Option<&CompactionSummary>,
    messages: &[ProviderMessage],
) -> Vec<ProviderMessage> {
    let mut result = Vec::with_capacity(messages.len() + usize::from(summary.is_some()));
    if let Some(summary) = summary {
        result.push(ProviderMessage::Text {
            role: MessageRole::User,
            text: render_summary(summary),
        });
    }
    result.extend_from_slice(messages);
    repair_tool_history(result)
}

pub(crate) fn normalize_compaction_summary(
    mut summary: CompactionSummary,
    user_context: &CompactionUserContext,
) -> CompactionSummary {
    let recursive_legacy_summary = summary.contract_version < 4
        && (summary.summary.contains("[Compacted context v")
            || summary
                .current_user_request
                .contains("[Compacted context v")
            || summary
                .recent_user_messages
                .iter()
                .any(|message| message.contains("[Compacted context v"))
            || summary
                .user_constraints
                .iter()
                .any(|constraint| constraint.contains("[Compacted context v")));
    if !user_context.current_user_request.is_empty() {
        summary.current_user_request = user_context.current_user_request.clone();
    }
    if !user_context.recent_user_messages.is_empty() {
        summary.recent_user_messages = user_context.recent_user_messages.clone();
    }
    if !user_context.is_empty() && recursive_legacy_summary {
        summary.summary.clear();
        summary.user_constraints = user_context.user_constraints.clone();
    } else {
        summary.user_constraints = merge_string_history(
            summary
                .user_constraints
                .into_iter()
                .chain(user_context.user_constraints.iter().cloned()),
            8,
        );
    }
    summary
}

fn stable_previous_summary_text(summary: &CompactionSummary) -> Option<&str> {
    let text = summary.summary.trim();
    (!text.is_empty()).then_some(text)
}

fn merge_string_history(values: impl IntoIterator<Item = String>, limit: usize) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        if value.trim().is_empty() {
            continue;
        }
        if let Some(index) = result.iter().position(|current| current == &value) {
            result.remove(index);
        }
        result.push(value);
        if result.len() > limit {
            result.remove(0);
        }
    }
    result
}

fn important_tool_observations(messages: &[ProviderMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| {
            let ProviderMessage::ToolResult { name, output, .. } = message else {
                return None;
            };
            let mut high_signal = output
                .lines()
                .filter(|line| is_important_tool_line(line))
                .take(4)
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>();
            if high_signal.is_empty() {
                high_signal = output
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .take(1)
                    .collect();
            }
            if high_signal.is_empty() {
                return None;
            }
            Some(bound(
                &format!("tool {name}: {}", high_signal.join(" | ")),
                IMPORTANT_TOOL_OBSERVATION_BYTES,
            ))
        })
        .rev()
        .take(IMPORTANT_TOOL_OBSERVATION_LIMIT)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn is_important_tool_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    [
        "error", "failed", "failure", "warning", "test", "passed", "success", "exit", "modified",
        "changed", "path", "失败", "错误", "警告", "测试", "通过", "修改", "路径",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn summarize_large_tool_result(message: &ProviderMessage) -> ProviderMessage {
    let ProviderMessage::ToolResult {
        call_id,
        name,
        success,
        output,
    } = message
    else {
        return message.clone();
    };
    if output.len() <= LARGE_TOOL_OUTPUT_BYTES {
        return message.clone();
    }

    let mut tail_start = output.len().saturating_sub(TOOL_OUTPUT_PREVIEW_BYTES);
    while tail_start < output.len() && !output.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    ProviderMessage::ToolResult {
        call_id: call_id.clone(),
        name: name.clone(),
        success: *success,
        output: format!(
            "[Tool output summary: {} bytes, {} lines; middle omitted]\nFirst section:\n{}\nLast section:\n{}",
            output.len(),
            output.lines().count(),
            bound(output, TOOL_OUTPUT_PREVIEW_BYTES),
            &output[tail_start..],
        ),
    }
}

/// Keep only complete assistant-tool groups before sending history to a provider.
pub(crate) fn repair_tool_history(messages: Vec<ProviderMessage>) -> Vec<ProviderMessage> {
    let mut result: Vec<ProviderMessage> = Vec::with_capacity(messages.len());
    let mut iter = messages.into_iter().peekable();
    while let Some(message) = iter.next() {
        match message {
            ProviderMessage::AssistantToolCalls { text, calls } => {
                let expected_ids = calls
                    .iter()
                    .map(|call| call.id.as_str())
                    .collect::<HashSet<_>>();
                let mut tool_results = Vec::new();
                while matches!(iter.peek(), Some(ProviderMessage::ToolResult { .. })) {
                    tool_results.push(iter.next().expect("peeked tool result must exist"));
                }

                let result_ids = tool_results
                    .iter()
                    .filter_map(|message| match message {
                        ProviderMessage::ToolResult { call_id, .. } => Some(call_id.as_str()),
                        _ => None,
                    })
                    .collect::<HashSet<_>>();
                let complete = !calls.is_empty()
                    && expected_ids.len() == calls.len()
                    && tool_results.len() == calls.len()
                    && result_ids == expected_ids;

                if complete {
                    result.push(ProviderMessage::AssistantToolCalls { text, calls });
                    result.extend(tool_results);
                } else if !text.trim().is_empty() {
                    result.push(ProviderMessage::Text {
                        role: MessageRole::Assistant,
                        text,
                    });
                }
            }
            ProviderMessage::ToolResult { .. } => {
                // Tool results are retained only as part of a complete group above.
            }
            other => result.push(other),
        }
    }
    result
}

pub fn render_summary(summary: &CompactionSummary) -> String {
    let recent_user_messages = summary.recent_user_messages.join("\n");
    let important_tool_observations = summary.important_tool_observations.join("\n");
    let recent_tool_results = summary
        .recent_tool_results
        .iter()
        .filter_map(|message| match message {
            ProviderMessage::ToolResult {
                name,
                success,
                output,
                ..
            } => Some(format!("tool {name} ({success}): {}", bound(output, 1_000))),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "[Compacted context v{}]\nSummary:\n{}\nCurrent user request:\n{}\nRecent user requests:\n{}\nUser constraints:\n{}\nImportant tool observations:\n{}\nRecent tool results:\n{}",
        summary.contract_version,
        summary.summary,
        summary.current_user_request,
        recent_user_messages,
        summary.user_constraints.join("\n"),
        important_tool_observations,
        recent_tool_results
    )
}

pub(crate) fn user_message_text(message: &ProviderMessage) -> Option<String> {
    match message {
        ProviderMessage::Text {
            role: MessageRole::User,
            text,
        } => (!text.trim().is_empty()).then_some(text.clone()),
        ProviderMessage::UserContent { text, images } => {
            if !text.trim().is_empty() {
                Some(text.clone())
            } else if !images.is_empty() {
                Some(format!("[用户上传了 {} 张图片]", images.len()))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_constraint(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "must",
        "never",
        "always",
        "不要",
        "必须",
        "只能",
        "不能",
        "请使用",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

fn message_chars(message: &ProviderMessage) -> usize {
    match message {
        ProviderMessage::Text { text, .. } => text.len(),
        ProviderMessage::UserContent { text, images } => text.len() + images.len() * 4096,
        ProviderMessage::AssistantToolCalls { text, calls } => {
            text.len()
                + calls
                    .iter()
                    .map(|call| call.name.len() + call.arguments.to_string().len())
                    .sum::<usize>()
        }
        ProviderMessage::ToolResult { name, output, .. } => name.len() + output.len(),
        ProviderMessage::ProviderContext { item, .. } => item.to_string().len(),
    }
}

fn bound(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compact_once(
        messages: &[ProviderMessage],
        limit: usize,
    ) -> (CompactionSummary, Vec<ProviderMessage>) {
        let user_context = CompactionUserContext::from_messages(messages);
        compact(messages, limit, None, &user_context)
    }

    #[test]
    fn compaction_keeps_constraints_and_recent_tool_results() {
        let mut messages = vec![ProviderMessage::Text {
            role: MessageRole::User,
            text: "You must never delete files".repeat(100),
        }];
        messages.extend((0..20).map(|n| ProviderMessage::Text {
            role: MessageRole::Assistant,
            text: format!("history {n} {}", "x".repeat(500)),
        }));
        messages.push(ProviderMessage::ToolResult {
            call_id: "1".into(),
            name: "read_file".into(),
            success: true,
            output: "important".into(),
        });
        let (summary, compacted) = compact_once(&messages, 2_000);
        assert!(
            summary
                .user_constraints
                .iter()
                .any(|value| value.contains("never delete"))
        );
        assert_eq!(summary.recent_tool_results.len(), 1);
        assert!(render_summary(&summary).contains("tool read_file (true): important"));
        assert!(estimate_tokens(&compacted) < estimate_tokens(&messages));
    }

    #[test]
    fn compaction_preserves_recent_user_requests() {
        let mut messages = vec![ProviderMessage::Text {
            role: MessageRole::User,
            text: "早期请求".into(),
        }];
        messages.extend((0..24).map(|index| ProviderMessage::Text {
            role: MessageRole::Assistant,
            text: format!("历史回复 {index} {}", "x".repeat(500)),
        }));
        messages.extend([
            ProviderMessage::Text {
                role: MessageRole::User,
                text: "请检查登录流程并保留现有 API".into(),
            },
            ProviderMessage::UserContent {
                text: "最后请运行测试并报告失败原因".into(),
                images: Vec::new(),
            },
        ]);

        let (summary, compacted) = compact_once(&messages, 2_000);

        assert_eq!(
            summary.recent_user_messages,
            vec![
                "早期请求".to_string(),
                "请检查登录流程并保留现有 API".to_string(),
                "最后请运行测试并报告失败原因".to_string(),
            ]
        );
        assert!(render_summary(&summary).contains("Recent user requests:"));
        assert!(render_summary(&summary).contains("最后请运行测试并报告失败原因"));
        assert!(estimate_tokens(&compacted) < estimate_tokens(&messages));
    }

    #[test]
    fn legacy_compaction_summary_defaults_recent_user_requests() {
        let legacy = serde_json::json!({
            "contractVersion": 1,
            "summary": "old summary",
            "userConstraints": [],
            "recentToolResults": [],
            "compactedMessageCount": 1
        });

        let summary: CompactionSummary = serde_json::from_value(legacy).unwrap();

        assert!(summary.recent_user_messages.is_empty());
    }

    #[test]
    fn compaction_keeps_a_marker_for_image_only_user_requests() {
        let messages = vec![ProviderMessage::UserContent {
            text: String::new(),
            images: vec![crate::providers::ProviderImage {
                name: "screenshot.png".into(),
                data_url: "data:image/png;base64,AA==".into(),
            }],
        }];

        let (summary, _) = compact_once(&messages, 2_000);

        assert_eq!(
            summary.recent_user_messages,
            vec!["[用户上传了 1 张图片]".to_string()]
        );
    }

    #[test]
    fn reported_context_usage_uses_the_auto_compact_threshold() {
        assert!(!needs_compaction_for_usage(129_999, 200_000));
        assert!(needs_compaction_for_usage(130_000, 200_000));
    }

    #[test]
    fn working_context_limit_stays_below_the_hard_model_limit() {
        assert_eq!(default_working_context_limit(128_000), 96_000);
        assert_eq!(normalize_working_context_limit(128_000, 64_000), 64_000);
        assert_eq!(normalize_working_context_limit(128_000, 200_000), 128_000);
        assert_eq!(default_working_context_limit(32_000), 32_000);
    }

    #[test]
    fn request_estimate_includes_runtime_instructions_and_tool_schemas() {
        let messages = vec![ProviderMessage::Text {
            role: MessageRole::User,
            text: "hello".into(),
        }];
        let tools = vec![ToolDefinition {
            name: "read_file".into(),
            description: "read a file".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }),
        }];
        let base = estimate_provider_request_tokens(&messages, "", &[]);
        let with_overhead = estimate_provider_request_tokens(&messages, "system rules", &tools);
        assert!(with_overhead > base);
        assert!(needs_compaction_for_request(
            &messages,
            &"x".repeat(30_000),
            &tools,
            10_000
        ));
    }

    #[test]
    fn compaction_records_current_request_observations_and_size() {
        let mut messages = vec![ProviderMessage::Text {
            role: MessageRole::User,
            text: "请修复并运行测试".into(),
        }];
        messages.push(ProviderMessage::ToolResult {
            call_id: "call-1".into(),
            name: "run_command".into(),
            success: false,
            output: "exit code: 1\nerror: test failed\nlong details".into(),
        });
        messages.extend((0..12).map(|index| ProviderMessage::Text {
            role: MessageRole::Assistant,
            text: format!("历史 {index} {}", "x".repeat(500)),
        }));
        let (summary, compacted) = compact_once(&messages, 1_024);
        assert_eq!(summary.contract_version, 4);
        assert_eq!(summary.current_user_request, "请修复并运行测试");
        assert!(
            summary
                .important_tool_observations
                .iter()
                .any(|line| line.contains("test failed"))
        );
        assert_eq!(
            summary.estimated_before_tokens,
            estimate_tokens(&render_provider_history(None, &messages))
        );
        assert_eq!(summary.estimated_after_tokens, estimate_tokens(&compacted));
    }

    #[test]
    fn long_turn_replay_shows_working_budget_reduces_repeated_input() {
        let tools = vec![ToolDefinition {
            name: "read_file".into(),
            description: "read a bounded file".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }),
        }];

        let replay = |working_limit| {
            let instructions = "stable runtime rules ".repeat(200);
            let mut messages = vec![ProviderMessage::Text {
                role: MessageRole::User,
                text: "请完成一个多步骤检查任务".into(),
            }];
            let user_context = CompactionUserContext::from_messages(&messages);
            let mut summary = None;
            let mut total_estimated_input = 0usize;
            let mut compactions = 0usize;
            for index in 0..19 {
                let mut request_history = render_provider_history(summary.as_ref(), &messages);
                if needs_compaction_for_request(
                    &request_history,
                    &instructions,
                    &tools,
                    working_limit,
                ) {
                    let (next_summary, compacted) =
                        compact(&messages, working_limit, summary.as_ref(), &user_context);
                    if next_summary.compacted_message_count > 0 {
                        compactions += 1;
                        summary = Some(next_summary);
                        messages.clear();
                        request_history = compacted;
                    }
                }
                total_estimated_input +=
                    estimate_provider_request_tokens(&request_history, &instructions, &tools);
                messages.push(ProviderMessage::AssistantToolCalls {
                    text: format!("checking step {index}"),
                    calls: vec![crate::protocol::ToolCall {
                        id: format!("call-{index}"),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "src/lib.rs"}),
                        metadata: serde_json::Value::Null,
                    }],
                });
                messages.push(ProviderMessage::ToolResult {
                    call_id: format!("call-{index}"),
                    name: "read_file".into(),
                    success: true,
                    output: format!("step {index} path src/lib.rs\n{}", "result ".repeat(3_000)),
                });
            }
            (total_estimated_input, compactions)
        };

        let (hard_window_total, _) = replay(128_000);
        let (working_window_total, compactions) = replay(DEFAULT_WORKING_CONTEXT_LIMIT);
        assert!(compactions > 0);
        assert!(working_window_total < hard_window_total);
    }

    #[test]
    fn repeated_compaction_preserves_the_real_request_without_recursive_summaries() {
        let request = "请设计 workflow 设置并对照参考实现";
        let mut user_context = CompactionUserContext::default();
        user_context.observe(request.to_string());
        let mut previous_summary = None;
        let mut messages = vec![ProviderMessage::Text {
            role: MessageRole::User,
            text: request.to_string(),
        }];

        for cycle in 0..16 {
            messages.extend((0..12).map(|index| ProviderMessage::Text {
                role: MessageRole::Assistant,
                text: format!("cycle {cycle} observation {index} {}", "x".repeat(600)),
            }));
            let (summary, rendered) =
                compact(&messages, 2_000, previous_summary.as_ref(), &user_context);

            assert!(summary.compacted_message_count > 0);
            assert_eq!(summary.current_user_request, request);
            assert_eq!(summary.recent_user_messages, [request]);
            assert!(!summary.summary.contains("[Compacted context v"));
            let rendered = match rendered.first() {
                Some(ProviderMessage::Text { text, .. }) => text,
                other => panic!("expected rendered compaction summary, got {other:?}"),
            };
            assert_eq!(rendered.matches("[Compacted context v4]").count(), 1);

            previous_summary = Some(summary);
            messages.clear();
        }
    }

    #[test]
    fn legacy_recursive_summary_uses_authoritative_user_events_for_recovery() {
        let mut user_context = CompactionUserContext::default();
        user_context.observe("原始 workflow 请求".to_string());
        let summary = CompactionSummary {
            contract_version: 3,
            summary: "User: [Compacted context v3]\nSummary:\nrecursive".to_string(),
            user_constraints: vec!["[Compacted context v3]\n必须递归".to_string()],
            recent_user_messages: vec!["[Compacted context v3]".to_string()],
            current_user_request: "[Compacted context v3]".to_string(),
            important_tool_observations: vec!["tool read_file: useful".to_string()],
            recent_tool_results: Vec::new(),
            compacted_message_count: 12,
            estimated_before_tokens: 10_000,
            estimated_after_tokens: 2_000,
        };

        let normalized = normalize_compaction_summary(summary, &user_context);

        assert!(normalized.summary.is_empty());
        assert_eq!(normalized.current_user_request, "原始 workflow 请求");
        assert_eq!(normalized.recent_user_messages, ["原始 workflow 请求"]);
        assert!(normalized.user_constraints.is_empty());
        assert_eq!(
            normalized.important_tool_observations,
            ["tool read_file: useful"]
        );
    }

    #[test]
    fn compaction_keeps_two_recent_tools_and_summarizes_large_outputs() {
        let messages = (0..3)
            .map(|index| ProviderMessage::ToolResult {
                call_id: format!("call-{index}"),
                name: "run_command".into(),
                success: true,
                output: if index == 2 {
                    format!("first-line\n{}\nlast-line", "x".repeat(6_000))
                } else {
                    format!("result-{index}")
                },
            })
            .collect::<Vec<_>>();

        let (summary, _) = compact_once(&messages, 2_000);

        assert_eq!(summary.recent_tool_results.len(), 2);
        assert!(matches!(
            &summary.recent_tool_results[0],
            ProviderMessage::ToolResult { call_id, .. } if call_id == "call-1"
        ));
        assert!(matches!(
            &summary.recent_tool_results[1],
            ProviderMessage::ToolResult { output, .. }
                if output.contains("Tool output summary: 6021 bytes")
                    && output.contains("first-line")
                    && output.contains("last-line")
                    && output.len() < LARGE_TOOL_OUTPUT_BYTES
        ));
    }

    #[test]
    fn compaction_preserves_tool_call_pairing() {
        let call = crate::protocol::ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "README.md"}),
            metadata: serde_json::Value::Null,
        };
        let mut messages = vec![ProviderMessage::Text {
            role: MessageRole::User,
            text: "请阅读 README".into(),
        }];
        messages.extend((0..40).map(|n| ProviderMessage::Text {
            role: MessageRole::Assistant,
            text: format!("history {n} {}", "x".repeat(500)),
        }));
        messages.push(ProviderMessage::AssistantToolCalls {
            text: String::new(),
            calls: vec![call],
        });
        messages.push(ProviderMessage::ToolResult {
            call_id: "call-1".into(),
            name: "read_file".into(),
            success: true,
            output: "ok".into(),
        });
        let (_, compacted) = compact_once(&messages, 4_000);
        let mut iter = compacted.iter();
        while let Some(msg) = iter.next() {
            if let ProviderMessage::AssistantToolCalls { calls, .. } = msg {
                for _ in 0..calls.len() {
                    let next = iter
                        .next()
                        .expect("missing tool result after assistant tool_calls");
                    assert!(
                        matches!(next, ProviderMessage::ToolResult { .. }),
                        "assistant(tool_calls) must be followed by ToolResult"
                    );
                }
            }
        }
    }

    #[test]
    fn tool_history_repair_matches_every_result_by_call_id() {
        let call = |id: &str| crate::protocol::ToolCall {
            id: id.into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": format!("{id}.md")}),
            metadata: serde_json::Value::Null,
        };
        let result = |id: &str| ProviderMessage::ToolResult {
            call_id: id.into(),
            name: "read_file".into(),
            success: true,
            output: "ok".into(),
        };

        let complete = repair_tool_history(vec![
            ProviderMessage::AssistantToolCalls {
                text: String::new(),
                calls: vec![call("a"), call("b")],
            },
            result("b"),
            result("a"),
        ]);
        assert_eq!(complete.len(), 3);

        let repaired = repair_tool_history(vec![
            ProviderMessage::AssistantToolCalls {
                text: "Checking files".into(),
                calls: vec![call("a"), call("b")],
            },
            result("a"),
            ProviderMessage::Text {
                role: MessageRole::User,
                text: "continue".into(),
            },
            result("orphan"),
            ProviderMessage::AssistantToolCalls {
                text: String::new(),
                calls: vec![call("c")],
            },
            result("wrong-id"),
        ]);

        assert!(matches!(
            repaired.as_slice(),
            [
                ProviderMessage::Text { role: MessageRole::Assistant, text },
                ProviderMessage::Text { role: MessageRole::User, .. }
            ] if text == "Checking files"
        ));
    }
}
