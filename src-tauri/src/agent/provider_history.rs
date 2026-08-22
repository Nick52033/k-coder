use crate::context::{self, CompactionSummary, CompactionUserContext};
use crate::protocol::{TokenUsage, ToolResult};
use crate::providers::ProviderMessage;
use crate::storage::{StoredEvent, StoredEventKind};

use super::input::chat_to_provider;

/// Keeps structured compaction state separate from real post-compaction messages.
pub(super) struct ProviderHistory {
    summary: Option<CompactionSummary>,
    messages: Vec<ProviderMessage>,
    user_context: CompactionUserContext,
}

impl ProviderHistory {
    pub(super) fn request_messages(&self) -> Vec<ProviderMessage> {
        context::render_provider_history(self.summary.as_ref(), &self.messages)
    }

    pub(super) fn messages(&self) -> &[ProviderMessage] {
        &self.messages
    }

    pub(super) fn summary(&self) -> Option<&CompactionSummary> {
        self.summary.as_ref()
    }

    pub(super) fn user_context(&self) -> &CompactionUserContext {
        &self.user_context
    }
}

pub(super) fn provider_history(events: Vec<StoredEvent>, supports_vision: bool) -> ProviderHistory {
    let mut history = Vec::new();
    let mut summary = None;
    let mut user_context = CompactionUserContext::default();
    for event in events {
        let message = match event.kind {
            StoredEventKind::UserMessage { message } => {
                let message = chat_to_provider(message, supports_vision);
                if let Some(text) = message.as_ref().and_then(context::user_message_text) {
                    user_context.observe(text);
                }
                message
            }
            StoredEventKind::AssistantMessage { message } => {
                chat_to_provider(message, supports_vision)
            }
            StoredEventKind::AssistantToolCalls { text, calls, .. } => {
                Some(ProviderMessage::AssistantToolCalls { text, calls })
            }
            StoredEventKind::ToolResult {
                call_id,
                name,
                result,
            } => {
                let output = provider_tool_output(&name, &result);
                Some(ProviderMessage::ToolResult {
                    call_id,
                    name,
                    success: result.success,
                    output,
                })
            }
            StoredEventKind::ProviderContext { provider, item } => {
                Some(ProviderMessage::ProviderContext { provider, item })
            }
            StoredEventKind::ContextCompacted {
                summary: compacted, ..
            } => {
                history.clear();
                summary = Some(compacted);
                None
            }
            _ => None,
        };
        if let Some(message) = message {
            history.push(message);
        }
    }
    ProviderHistory {
        summary: summary
            .map(|summary| context::normalize_compaction_summary(summary, &user_context)),
        messages: context::repair_tool_history(history),
        user_context,
    }
}

fn provider_tool_output(name: &str, result: &ToolResult) -> String {
    let Some(header) = read_file_observation_header(name, result) else {
        return result.output.clone();
    };
    if result.output.is_empty() {
        header
    } else {
        format!("{header}\n{}", result.output)
    }
}

fn read_file_observation_header(name: &str, result: &ToolResult) -> Option<String> {
    if name != "read_file" {
        return None;
    }
    let path = result.metadata.get("path")?.as_str()?;
    let revision = result.metadata.get("fileRevision")?.as_str()?;
    let start_line = result.metadata.get("startLine")?.as_u64()?;
    let end_line = result.metadata.get("endLine")?.as_u64()?;
    if path.is_empty() || revision.is_empty() || start_line == 0 || end_line < start_line {
        return None;
    }
    let provenance = serde_json::json!({
        "path": path,
        "fileRevision": revision,
        "startLine": start_line,
        "endLine": end_line,
    });
    Some(format!(
        "[read_file observation] {}",
        serde_json::to_string(&provenance).ok()?
    ))
}

pub(super) fn last_active_context_usage(events: &[StoredEvent]) -> Option<TokenUsage> {
    events.iter().fold(None, |usage, event| match &event.kind {
        StoredEventKind::ProviderCallUsage { usage, .. } => Some(*usage),
        StoredEventKind::ContextCompacted { .. } => None,
        _ => usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_file_metadata_becomes_provider_visible_provenance() {
        let result = ToolResult {
            success: true,
            output: "public class ReturnModel {}".into(),
            metadata: serde_json::json!({
                "path": "Permission.Util/Permission.Util/Model/TData.cs",
                "fileRevision": "6ab4159a",
                "startLine": 1,
                "endLine": 76,
            }),
        };

        let output = provider_tool_output("read_file", &result);

        assert!(output.starts_with("[read_file observation] "));
        assert!(output.contains(r#""path":"Permission.Util/Permission.Util/Model/TData.cs""#));
        assert!(output.contains(r#""fileRevision":"6ab4159a""#));
        assert!(output.contains(r#""startLine":1"#));
        assert!(output.contains(r#""endLine":76"#));
        assert!(output.ends_with("public class ReturnModel {}"));
    }

    #[test]
    fn non_read_and_unversioned_results_keep_the_persisted_output() {
        let result = ToolResult {
            success: true,
            output: "unchanged".into(),
            metadata: serde_json::json!({"path": "src/lib.rs"}),
        };

        assert_eq!(provider_tool_output("run_command", &result), "unchanged");
        assert_eq!(provider_tool_output("read_file", &result), "unchanged");
    }

    #[test]
    fn read_file_provenance_survives_compaction_rendering() {
        let result = ToolResult {
            success: true,
            output: "public class ReturnModel {}".into(),
            metadata: serde_json::json!({
                "path": "Permission.Util/Permission.Util/Model/TData.cs",
                "fileRevision": "6ab4159a",
                "startLine": 1,
                "endLine": 76,
            }),
        };
        let messages = vec![ProviderMessage::ToolResult {
            call_id: "read-1".into(),
            name: "read_file".into(),
            success: true,
            output: provider_tool_output("read_file", &result),
        }];
        let user_context = CompactionUserContext::default();

        let (summary, _) = context::compact(&messages, 1_024, None, &user_context);
        let rendered = context::render_summary(&summary);

        assert_eq!(summary.contract_version, 5);
        assert!(rendered.contains("Permission.Util/Permission.Util/Model/TData.cs"));
        assert!(rendered.contains("6ab4159a"));
        assert!(rendered.contains(r#""startLine":1"#));
        assert!(rendered.contains(r#""endLine":76"#));
    }
}
