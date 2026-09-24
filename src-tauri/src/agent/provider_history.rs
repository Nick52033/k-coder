use crate::context::{self, CompactionSummary, CompactionUserContext};
use std::collections::HashMap;

use crate::protocol::{MessageRole, TokenUsage, ToolResult, UserInputAction, UserInputRequestKind};
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
        let mut messages = context::render_provider_history(self.summary.as_ref(), &self.messages);
        if self.summary.is_none() {
            if let Some(text) = self.user_context.clarification_context() {
                // Keep durable answers visible even when interruption left the question's
                // tool group incomplete. Insert outside tool groups to preserve pairing.
                messages.insert(
                    0,
                    ProviderMessage::Text {
                        role: MessageRole::User,
                        text,
                    },
                );
            }
        }
        messages
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
    provider_history_for_turn(events, supports_vision, None)
}

/// Builds provider history for a live Turn. Opaque provider context such as an OpenAI Responses
/// encrypted reasoning item is valid only for follow-up requests within the Turn that produced
/// it; retries and later Turns must not replay it, especially after the user switches models.
pub(super) fn provider_history_for_turn(
    events: Vec<StoredEvent>,
    supports_vision: bool,
    current_turn_id: Option<&str>,
) -> ProviderHistory {
    let mut history = Vec::new();
    let mut summary = None;
    let mut user_context = CompactionUserContext::default();
    let mut latest_image_message: Option<ProviderMessage> = None;
    let mut questions = HashMap::new();
    let mut assistant_progress = Vec::new();
    for event in events {
        // Preserve the legacy full-history behavior for callers that are not building a live
        // Turn. Runtime requests pass a Turn ID and only replay opaque context from that Turn.
        let include_provider_context = current_turn_id
            .map(|turn_id| event.turn_id.as_deref() == Some(turn_id))
            .unwrap_or(true);
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
            StoredEventKind::ProviderContext { provider, item } if include_provider_context => {
                Some(ProviderMessage::ProviderContext { provider, item })
            }
            StoredEventKind::ProviderContext { .. } => None,
            StoredEventKind::UserInputRequested { request }
                if request.kind == UserInputRequestKind::ModelQuestion
                    && request.thread_id == event.thread_id
                    && Some(&request.turn_id) == event.turn_id.as_ref() =>
            {
                questions.insert(request.id.clone(), request);
                None
            }
            StoredEventKind::UserInputResolved {
                request_id,
                resolution,
            } => {
                if let Some(request) = questions.remove(&request_id) {
                    if resolution.action == UserInputAction::Answered
                        && request.thread_id == event.thread_id
                        && Some(&request.turn_id) == event.turn_id.as_ref()
                    {
                        for answer in resolution.answers {
                            if request
                                .questions
                                .iter()
                                .any(|q| q.question == answer.question)
                            {
                                user_context
                                    .observe_clarification(&answer.question, &answer.answer);
                            }
                        }
                    }
                }
                None
            }
            StoredEventKind::ContextCompacted {
                summary: mut compacted,
                ..
            } => {
                history.clear();
                if let Some(image_message) = &latest_image_message {
                    history.push(image_message.clone());
                }
                // Replay original assistant events to repair v5 snapshots that discarded all
                // progress at the last workspace write. Never mine rendered summaries as facts.
                compacted.recent_assistant_progress = context::assistant_progress_history(
                    compacted
                        .recent_assistant_progress
                        .into_iter()
                        .chain(assistant_progress.clone()),
                    &[],
                );
                summary = Some(compacted);
                None
            }
            _ => None,
        };
        if let Some(message) = message {
            if matches!(
                &message,
                ProviderMessage::UserContent { images, .. }
                    | ProviderMessage::AssistantImageReference { images, .. }
                    if !images.is_empty()
            ) {
                latest_image_message = Some(message.clone());
            }
            assistant_progress = context::assistant_progress_history(
                assistant_progress,
                std::slice::from_ref(&message),
            );
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
    let mut provenance = serde_json::json!({
        "path": path,
        "fileRevision": revision,
        "startLine": start_line,
        "endLine": end_line,
    });
    if result.success
        && result.metadata["observationStatus"] == "read_observation_already_covered"
        && result.metadata["contentSuppressed"] == false
    {
        provenance["notice"] = serde_json::Value::String(
            "This content was already observed. The requested body is included below. Reuse established facts where possible; reread when context is missing or a specific unresolved detail requires it, otherwise proceed with the task or final answer."
                .into(),
        );
    }
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
    use crate::protocol::{
        UserInputAnswer, UserInputQuestion, UserInputRequest, UserInputResolution,
    };

    fn question_events(kind: UserInputRequestKind, action: UserInputAction) -> Vec<StoredEvent> {
        vec![
            StoredEvent::new(
                "thread",
                Some("turn".into()),
                StoredEventKind::UserInputRequested {
                    request: UserInputRequest {
                        id: "question".into(),
                        thread_id: "thread".into(),
                        turn_id: "turn".into(),
                        tool_call_id: "ask".into(),
                        kind,
                        questions: vec![UserInputQuestion {
                            question: "如何处理入口？".into(),
                            options: vec![],
                        }],
                        created_at_ms: 1,
                        expires_at_ms: Some(2),
                    },
                },
            ),
            StoredEvent::new(
                "thread",
                Some("turn".into()),
                StoredEventKind::UserInputResolved {
                    request_id: "question".into(),
                    resolution: UserInputResolution {
                        action,
                        answers: vec![UserInputAnswer {
                            question: "如何处理入口？".into(),
                            answer: "改成可点击的状态面板".into(),
                        }],
                    },
                },
            ),
        ]
    }

    fn legacy_empty_compaction() -> StoredEvent {
        StoredEvent::new(
            "thread",
            Some("turn".into()),
            StoredEventKind::ContextCompacted {
                summary: serde_json::from_value(serde_json::json!({
                    "contractVersion": 5, "summary": "", "userConstraints": [],
                    "recentToolResults": [], "compactedMessageCount": 100,
                }))
                .unwrap(),
                automatic: true,
            },
        )
    }

    #[test]
    fn legacy_compaction_restores_answered_choices_and_progress_from_original_events() {
        let mut events = question_events(
            UserInputRequestKind::ModelQuestion,
            UserInputAction::Answered,
        );
        events.push(StoredEvent::new(
            "thread",
            Some("turn".into()),
            StoredEventKind::AssistantToolCalls {
                item_id: None,
                text: "组件已实现，现在补充 e2e 测试。".into(),
                calls: vec![],
            },
        ));
        events.extend([legacy_empty_compaction(), legacy_empty_compaction()]);
        let restored = provider_history(events, false);
        let summary = restored.summary().unwrap();
        assert_eq!(
            summary.user_clarifications,
            ["Q: 如何处理入口？\nA: 改成可点击的状态面板"]
        );
        assert_eq!(
            summary.recent_assistant_progress,
            ["组件已实现，现在补充 e2e 测试。"]
        );
        let rendered = serde_json::to_string(&restored.request_messages()).unwrap();
        assert!(rendered.contains("改成可点击的状态面板"));
        assert!(rendered.contains("补充 e2e 测试"));
    }

    #[test]
    fn opaque_provider_context_is_limited_to_the_turn_that_created_it() {
        let events = [
            StoredEvent::new(
                "thread",
                Some("previous-turn".into()),
                StoredEventKind::ProviderContext {
                    provider: "openai_responses".into(),
                    item: serde_json::json!({
                        "type": "reasoning",
                        "encrypted_content": "previous-turn-secret-payload"
                    }),
                },
            ),
            StoredEvent::new(
                "thread",
                Some("current-turn".into()),
                StoredEventKind::ProviderContext {
                    provider: "openai_responses".into(),
                    item: serde_json::json!({
                        "type": "reasoning",
                        "encrypted_content": "current-turn-payload"
                    }),
                },
            ),
        ];

        let messages = provider_history_for_turn(events.to_vec(), false, Some("current-turn"))
            .request_messages();
        let serialized = serde_json::to_string(&messages).unwrap();

        assert!(serialized.contains("current-turn-payload"));
        assert!(!serialized.contains("previous-turn-secret-payload"));

        let legacy_messages = provider_history(events.to_vec(), false).request_messages();
        let legacy_serialized = serde_json::to_string(&legacy_messages).unwrap();

        assert!(legacy_serialized.contains("current-turn-payload"));
        assert!(legacy_serialized.contains("previous-turn-secret-payload"));
    }

    #[test]
    fn answered_question_survives_interruption_before_its_tool_result_without_compaction() {
        let mut events = vec![StoredEvent::new(
            "thread",
            Some("turn".into()),
            StoredEventKind::AssistantToolCalls {
                item_id: None,
                text: "确认入口方案".into(),
                calls: vec![crate::protocol::ToolCall {
                    id: "ask".into(),
                    name: "request_user_input".into(),
                    arguments: serde_json::json!({}),
                    metadata: serde_json::json!({}),
                }],
            },
        )];
        events.extend(question_events(
            UserInputRequestKind::ModelQuestion,
            UserInputAction::Answered,
        ));
        events.push(StoredEvent::new(
            "thread",
            Some("turn".into()),
            StoredEventKind::TurnCancelled,
        ));
        let restored = provider_history(events, false);
        assert!(restored.summary().is_none());
        let messages = restored.request_messages();
        let rendered = serde_json::to_string(&messages).unwrap();
        assert!(rendered.contains("改成可点击的状态面板"));
        assert!(rendered.contains("do not grant tool authorization"));
        assert!(!messages.iter().any(|message| matches!(
            message,
            ProviderMessage::AssistantToolCalls { .. } | ProviderMessage::ToolResult { .. }
        )));
    }

    #[test]
    fn clarification_replay_requires_a_matching_answered_model_question() {
        let valid = question_events(
            UserInputRequestKind::ModelQuestion,
            UserInputAction::Answered,
        );
        let mut cases = vec![
            question_events(
                UserInputRequestKind::TurnContinuation,
                UserInputAction::Answered,
            ),
            question_events(
                UserInputRequestKind::ModelQuestion,
                UserInputAction::Skipped,
            ),
            question_events(
                UserInputRequestKind::ModelQuestion,
                UserInputAction::Cancelled,
            ),
            vec![valid[1].clone()],
        ];
        let mut wrong_turn = valid.clone();
        wrong_turn[1].turn_id = Some("other".into());
        cases.push(wrong_turn);
        let mut wrong_thread = valid.clone();
        wrong_thread[1].thread_id = "other".into();
        cases.push(wrong_thread);
        let mut wrong_question = valid;
        if let StoredEventKind::UserInputResolved { resolution, .. } = &mut wrong_question[1].kind {
            resolution.answers[0].question = "unrelated question".into();
        }
        cases.push(wrong_question);
        for mut events in cases {
            assert!(
                !serde_json::to_string(&provider_history(events.clone(), false).request_messages())
                    .unwrap()
                    .contains("改成可点击的状态面板")
            );
            events.push(legacy_empty_compaction());
            assert!(
                provider_history(events, false)
                    .summary()
                    .unwrap()
                    .user_clarifications
                    .is_empty()
            );
        }
    }

    #[test]
    fn restored_compaction_preserves_latest_image_without_text_only_leakage() {
        let message = super::super::input::build_user_message(
            "分析图片",
            vec![crate::protocol::ImageAttachment {
                name: "example.png".into(),
                data_url: "data:image/png;base64,AA==".into(),
            }],
            true,
        )
        .unwrap();
        let provider_message = chat_to_provider(message.clone(), true).unwrap();
        let (summary, _) = context::compact(
            &[provider_message.clone()],
            2_000,
            None,
            &CompactionUserContext::default(),
        );
        let events = vec![
            StoredEvent::new("thread", None, StoredEventKind::UserMessage { message }),
            StoredEvent::new(
                "thread",
                None,
                StoredEventKind::ContextCompacted {
                    summary,
                    automatic: true,
                },
            ),
        ];
        assert!(
            provider_history(events.clone(), true)
                .request_messages()
                .contains(&provider_message)
        );
        assert!(
            !provider_history(events, false)
                .request_messages()
                .iter()
                .any(|m| matches!(m, ProviderMessage::UserContent { .. }))
        );
    }

    #[test]
    fn restored_compaction_preserves_latest_assistant_generated_image_reference() {
        let message = crate::protocol::ChatMessage {
            schema_version: crate::protocol::PROTOCOL_VERSION,
            id: "assistant-image".into(),
            role: crate::protocol::MessageRole::Assistant,
            content: vec![
                crate::protocol::ContentBlock::Text {
                    text: "已生成一张图片。".into(),
                },
                crate::protocol::ContentBlock::Image {
                    name: "generated.png".into(),
                    data_url: "data:image/png;base64,AA==".into(),
                },
            ],
            created_at_ms: 1,
        };
        let provider_message = chat_to_provider(message.clone(), true).unwrap();
        let (summary, _) = context::compact(
            std::slice::from_ref(&provider_message),
            2_000,
            None,
            &CompactionUserContext::default(),
        );
        let events = vec![
            StoredEvent::new(
                "thread",
                None,
                StoredEventKind::AssistantMessage { message },
            ),
            StoredEvent::new(
                "thread",
                None,
                StoredEventKind::ContextCompacted {
                    summary,
                    automatic: true,
                },
            ),
        ];

        assert!(
            provider_history(events.clone(), true)
                .request_messages()
                .contains(&provider_message)
        );
        assert!(
            !provider_history(events, false)
                .request_messages()
                .iter()
                .any(|message| matches!(message, ProviderMessage::AssistantImageReference { .. }))
        );
    }

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

        assert_eq!(summary.contract_version, 6);
        assert!(rendered.contains("Permission.Util/Permission.Util/Model/TData.cs"));
        assert!(rendered.contains("6ab4159a"));
        assert!(rendered.contains(r#""startLine":1"#));
        assert!(rendered.contains(r#""endLine":76"#));
    }
}
