use crate::context::{self, CompactionSummary, CompactionUserContext};
use crate::protocol::TokenUsage;
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
            } => Some(ProviderMessage::ToolResult {
                call_id,
                name,
                success: result.success,
                output: result.output,
            }),
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

pub(super) fn last_active_context_usage(events: &[StoredEvent]) -> Option<TokenUsage> {
    events.iter().fold(None, |usage, event| match &event.kind {
        StoredEventKind::ProviderCallUsage { usage, .. } => Some(*usage),
        StoredEventKind::ContextCompacted { .. } => None,
        _ => usage,
    })
}
