//! The optional bounded model rewrite for knowledge retrieval.
//!
//! The design puts one hard limit on this: the rewrite may only produce short *query strings*. It
//! cannot widen the collection scope, the path range or the read permissions, because the strings it
//! returns are fed straight back into the same scoped recall channels as the original query — there
//! is no parameter here that could reach the workspace, a provider endpoint or a model name.
//!
//! Everything about this path fails open. A missing provider, a timeout, a malformed reply or an
//! empty reply all return `Err`, and the caller keeps the deterministic rewrite.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::knowledge::retrieval::{
    MAX_REWRITTEN_QUERIES, MAX_REWRITTEN_QUERY_CHARS, QueryRewriter, RewriteHints,
};
use crate::protocol::{MessageRole, PROTOCOL_VERSION, ReasoningEffort};
use crate::providers::{Provider, ProviderEvent, ProviderMessage, ProviderRequest};

/// One provider call is worth at most this much wall clock before the rewrite gives up.
pub const DEFAULT_REWRITE_TIMEOUT: Duration = Duration::from_secs(6);
/// A rewrite reply longer than this is treated as a runaway answer, not a query list.
const MAX_REWRITE_RESPONSE_CHARS: usize = 4 * 1024;

const REWRITE_INSTRUCTIONS: &str = "\
You expand a code-search query so a lexical index can find more of the right files.
Reply with at most three alternative search queries, one per line, and nothing else: no numbering,
no bullets, no explanation, no file paths, no quotes.
Each line must be a short query of at most 120 characters.
Do not invent identifiers, paths or APIs that the original query does not already imply.";

/// Rewrites a query through one bounded provider call.
pub struct ModelQueryRewriter {
    provider: Arc<dyn Provider>,
    model: String,
    reasoning_effort: ReasoningEffort,
    timeout: Duration,
}

impl ModelQueryRewriter {
    pub fn new(
        provider: Arc<dyn Provider>,
        model: impl Into<String>,
        reasoning_effort: ReasoningEffort,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            reasoning_effort,
            timeout: DEFAULT_REWRITE_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn prompt(query: &str, hints: &RewriteHints) -> String {
        let mut prompt = format!("Original query: {query}\n");
        if let Some(project) = &hints.project_name {
            prompt.push_str(&format!("Project: {project}\n"));
        }
        if let Some(file) = &hints.current_file {
            prompt.push_str(&format!("File the user is looking at: {file}\n"));
        }
        if !hints.recent_entities.is_empty() {
            prompt.push_str(&format!(
                "Recently mentioned identifiers: {}\n",
                hints.recent_entities.join(", ")
            ));
        }
        prompt
    }
}

#[async_trait]
impl QueryRewriter for ModelQueryRewriter {
    async fn rewrite(&self, query: &str, hints: &RewriteHints) -> Result<Vec<String>, String> {
        let request = ProviderRequest {
            schema_version: PROTOCOL_VERSION,
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort,
            messages: vec![
                ProviderMessage::Text {
                    role: MessageRole::System,
                    text: REWRITE_INSTRUCTIONS.to_owned(),
                },
                ProviderMessage::Text {
                    role: MessageRole::User,
                    text: Self::prompt(query, hints),
                },
            ],
            tools: vec![],
        };
        let mut stream = tokio::time::timeout(
            self.timeout,
            self.provider.stream(request, CancellationToken::new()),
        )
        .await
        .map_err(|_| "KC_REWRITE_TIMEOUT".to_owned())?
        .map_err(|error| format!("KC_REWRITE_FAILED: {error}"))?;

        let mut reply = String::new();
        loop {
            let next = tokio::time::timeout(self.timeout, stream.next())
                .await
                .map_err(|_| "KC_REWRITE_TIMEOUT".to_owned())?;
            let Some(event) = next else {
                break;
            };
            match event.map_err(|error| format!("KC_REWRITE_FAILED: {error}"))? {
                ProviderEvent::TextDelta { delta } => {
                    reply.push_str(&delta);
                    if reply.chars().count() > MAX_REWRITE_RESPONSE_CHARS {
                        break;
                    }
                }
                ProviderEvent::Completed => break,
                _ => {}
            }
        }

        let values = parse_rewrites(&reply);
        if values.is_empty() {
            return Err("KC_REWRITE_EMPTY".to_owned());
        }
        Ok(values)
    }
}

/// Normalises one reply line and rejects the shapes that cannot be a query.
///
/// The prompt asks for one bare query per line, so formatting that only exists to decorate the
/// answer (a bullet, an inline-code wrap) is stripped, while the shapes the prompt explicitly
/// forbade (a code fence, a structured blob, an announcement such as "Here are some queries:") are
/// dropped outright instead of being searched for literally.
fn normalize_rewrite_line(raw: &str) -> Option<String> {
    let line = raw.trim();
    // A fence is markup around the answer, not part of the answer.
    if line.starts_with("```") {
        return None;
    }
    let line = line
        .trim_start_matches(|ch: char| {
            ch.is_ascii_digit() || matches!(ch, '.' | ')' | '-' | '*' | '•' | '、')
        })
        .trim()
        .trim_matches('"')
        .trim_matches('`')
        .trim();
    if line.is_empty() || line.chars().count() > MAX_REWRITTEN_QUERY_CHARS {
        return None;
    }
    // A JSON or array payload is a reply shape the prompt forbade, so it is not searchable input.
    if line.starts_with('{') || line.starts_with('[') {
        return None;
    }
    // "Here are the queries:" is an announcement; a query does not end on a colon.
    if line.ends_with(':') || line.ends_with('：') {
        return None;
    }
    Some(line.to_owned())
}

/// Pulls at most [`MAX_REWRITTEN_QUERIES`] queries out of a model reply.
///
/// Line-oriented on purpose: the prompt asks for one query per line, and a reply that does not
/// follow that shape (prose, a code block, a JSON blob) is filtered down to the lines that look like
/// queries instead of being parsed leniently into something the model never meant.
fn parse_rewrites(reply: &str) -> Vec<String> {
    let mut values = Vec::new();
    for raw in reply.lines() {
        let Some(line) = normalize_rewrite_line(raw) else {
            continue;
        };
        if values
            .iter()
            .any(|seen: &String| seen.eq_ignore_ascii_case(&line))
        {
            continue;
        }
        values.push(line);
        if values.len() >= MAX_REWRITTEN_QUERIES {
            break;
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_is_read_as_one_query_per_line() {
        let values = parse_rewrites("1. retrieval scoring\n- knowledge budget\n• 引用绑定\n");
        assert_eq!(
            values,
            vec![
                "retrieval scoring".to_owned(),
                "knowledge budget".to_owned(),
                "引用绑定".to_owned(),
            ]
        );
    }

    #[test]
    fn a_reply_is_capped_and_deduplicated() {
        let values = parse_rewrites("alpha\nALPHA\nbeta\ngamma\ndelta\n");
        assert_eq!(
            values,
            vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()],
            "at most {MAX_REWRITTEN_QUERIES} distinct queries"
        );
    }

    #[test]
    fn prose_and_oversized_lines_are_dropped() {
        let long = "x".repeat(MAX_REWRITTEN_QUERY_CHARS + 1);
        let values = parse_rewrites(&format!(
            "Sure! Here are some queries:\n\n```text\nretrieval weights\n```\n{long}\n"
        ));
        assert_eq!(values, vec!["retrieval weights".to_owned()]);
        assert!(parse_rewrites("").is_empty());
        assert!(parse_rewrites("   \n\n").is_empty());
    }

    #[test]
    fn fences_blobs_and_announcements_are_not_queries() {
        let values = parse_rewrites(
            "```json\n{\"queries\":[\"retrieval\"]}\n```\nHere are the queries:\n`budget percent`\n",
        );
        assert_eq!(
            values,
            vec!["budget percent".to_owned()],
            "only the real query survives; an inline-code wrap is decoration, not content"
        );
    }

    #[test]
    fn the_prompt_only_carries_host_context_and_never_a_path_range() {
        let prompt = ModelQueryRewriter::prompt(
            "检索评分",
            &RewriteHints {
                project_name: Some("k-coder".to_owned()),
                current_file: Some("src-tauri/src/knowledge.rs".to_owned()),
                recent_entities: vec!["retrieval".to_owned()],
            },
        );
        assert!(prompt.contains("Original query: 检索评分"));
        assert!(prompt.contains("Project: k-coder"));
        assert!(prompt.contains("retrieval"));
        assert!(
            !prompt.contains("scope") && !prompt.contains("endpoint") && !prompt.contains("model"),
            "the rewrite prompt must not offer scope or provider controls: {prompt}"
        );
    }
}
