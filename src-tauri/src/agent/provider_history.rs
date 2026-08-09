use crate::context;
use crate::protocol::{MessageRole, TokenUsage};
use crate::providers::ProviderMessage;
use crate::storage::{StoredEvent, StoredEventKind};

use super::input::chat_to_provider;

pub(super) fn provider_history(
    events: Vec<StoredEvent>,
    supports_vision: bool,
) -> Vec<ProviderMessage> {
    let mut history = Vec::new();
    for event in events {
        let message = match event.kind {
            StoredEventKind::UserMessage { message }
            | StoredEventKind::AssistantMessage { message } => {
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
            StoredEventKind::ContextCompacted { summary, .. } => {
                history.clear();
                history.push(ProviderMessage::Text {
                    role: MessageRole::User,
                    text: context::render_summary(&summary),
                });
                None
            }
            _ => None,
        };
        if let Some(message) = message {
            history.push(message);
        }
    }
    context::repair_tool_history(history)
}

pub(super) fn last_active_context_usage(events: &[StoredEvent]) -> Option<TokenUsage> {
    events.iter().fold(None, |usage, event| match &event.kind {
        StoredEventKind::ProviderCallUsage { usage, .. } => Some(*usage),
        StoredEventKind::ContextCompacted { .. } => None,
        _ => usage,
    })
}
