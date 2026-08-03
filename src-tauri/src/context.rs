use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::protocol::MessageRole;
use crate::providers::ProviderMessage;

pub const DEFAULT_CONTEXT_LIMIT: usize = 128_000;
pub const AUTO_COMPACT_THRESHOLD_PERCENT: usize = 80;
const CHARS_PER_TOKEN: usize = 4;

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
    pub recent_tool_results: Vec<ProviderMessage>,
    pub compacted_message_count: usize,
}

pub fn estimate_tokens(messages: &[ProviderMessage]) -> usize {
    messages
        .iter()
        .map(message_chars)
        .sum::<usize>()
        .div_ceil(CHARS_PER_TOKEN)
}

pub fn needs_compaction(messages: &[ProviderMessage], limit: usize) -> bool {
    estimate_tokens(messages) * 100 >= limit * AUTO_COMPACT_THRESHOLD_PERCENT
}

pub fn needs_compaction_for_usage(total_tokens: u64, limit: usize) -> bool {
    total_tokens.saturating_mul(100)
        >= (limit as u64).saturating_mul(AUTO_COMPACT_THRESHOLD_PERCENT as u64)
}

pub fn compact(
    messages: &[ProviderMessage],
    limit: usize,
) -> (CompactionSummary, Vec<ProviderMessage>) {
    let budget = ContextBudget::for_limit(limit);
    let recent_tool_results = messages
        .iter()
        .rev()
        .filter(|message| matches!(message, ProviderMessage::ToolResult { .. }))
        .take(4)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let user_constraints = messages
        .iter()
        .filter_map(|message| match message {
            ProviderMessage::Text {
                role: MessageRole::User,
                text,
            } if is_constraint(text) => Some(bound(text, 600)),
            ProviderMessage::UserContent { text, .. } if is_constraint(text) => {
                Some(bound(text, 600))
            }
            _ => None,
        })
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let keep_tokens = budget.history / 2;
    let mut kept = Vec::new();
    let mut used = 0;
    for message in messages.iter().rev() {
        let tokens = message_chars(message).div_ceil(CHARS_PER_TOKEN);
        if used + tokens > keep_tokens && !kept.is_empty() {
            break;
        }
        kept.push(message.clone());
        used += tokens;
    }
    kept.reverse();
    let compacted_count = messages.len().saturating_sub(kept.len());
    let summary_text = messages[..compacted_count]
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
    let summary = CompactionSummary {
        contract_version: 1,
        summary: bound(&summary_text, budget.history * CHARS_PER_TOKEN / 2),
        user_constraints,
        recent_tool_results,
        compacted_message_count: compacted_count,
    };
    let mut result = Vec::new();
    if compacted_count > 0 {
        result.push(ProviderMessage::Text {
            role: MessageRole::User,
            text: render_summary(&summary),
        });
    }
    result.extend(kept);
    let result = repair_tool_history(result);
    (summary, result)
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
        "[Compacted context v{}]\nSummary:\n{}\nUser constraints:\n{}\nRecent tool results:\n{}",
        summary.contract_version,
        summary.summary,
        summary.user_constraints.join("\n"),
        recent_tool_results
    )
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
        let (summary, compacted) = compact(&messages, 2_000);
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
    fn reported_context_usage_uses_the_auto_compact_threshold() {
        assert!(!needs_compaction_for_usage(159_999, 200_000));
        assert!(needs_compaction_for_usage(160_000, 200_000));
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
        let (_, compacted) = compact(&messages, 4_000);
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
