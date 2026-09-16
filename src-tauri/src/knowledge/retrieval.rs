//! Deterministic retrieval scoring, query rewriting and the knowledge budget.
//!
//! Everything in this module is pure: no SQL, no Provider, no clock reads that are not passed in.
//! That is deliberate, because the ranking contract is the part of knowledge retrieval that must be
//! reproducible — the same candidates and the same signals have to produce the same order on every
//! run, and a reviewer has to be able to see *why* a chunk outranked another.
//!
//! Two rules from the design live here rather than in the caller:
//!
//! * **The weights are fixed.** `0.35 lexical + 0.35 semantic + 0.10 title/symbol + 0.10 path
//!   + 0.05 freshness + 0.05 feedback`. A model never submits weights, and no caller can pass a
//!   custom vector, so "the ranking changed because a model asked for it" is impossible by
//!   construction.
//! * **The budget is a slice of the working context**, not a result count. `knowledge.budget_percent`
//!   is applied here so the cap cannot drift away from the setting the user sees.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;

/// Design §6.2: lexical recall carries the largest fixed weight.
pub const WEIGHT_LEXICAL: f64 = 0.35;
/// Design §6.2: semantic recall matches lexical when it is available.
pub const WEIGHT_SEMANTIC: f64 = 0.35;
/// Design §6.2: an exact title or symbol hit is worth more than a path hit.
pub const WEIGHT_TITLE_OR_SYMBOL: f64 = 0.10;
pub const WEIGHT_PATH_MATCH: f64 = 0.10;
pub const WEIGHT_FRESHNESS: f64 = 0.05;
pub const WEIGHT_USER_FEEDBACK: f64 = 0.05;

/// Design §6.1 step 7: at most six chunks are returned, independent of the byte budget.
pub const MAX_KNOWLEDGE_CHUNKS: usize = 6;
/// Design §6.1 steps 2-3: each recall channel contributes at most 24 candidates.
pub const MAX_CHANNEL_CANDIDATES: usize = 24;
/// Default for `knowledge.budget_percent`; the persisted setting overrides it.
pub const DEFAULT_KNOWLEDGE_BUDGET_PERCENT: usize = 8;
/// Guard rail matching the settings row, so a hand-edited setting cannot take over the context.
pub const MIN_KNOWLEDGE_BUDGET_PERCENT: usize = 1;
pub const MAX_KNOWLEDGE_BUDGET_PERCENT: usize = 50;
/// Working context used when the caller cannot supply the active model's limit.
///
/// Matches the figure the context assembler documents, so the knowledge slice and the assembly
/// budget are both expressed against the same working context.
pub const DEFAULT_WORKING_CONTEXT_TOKENS: usize = 96_000;
/// Rough token/character ratio, shared with the context assembler.
pub const CHARS_PER_TOKEN: usize = 4;
/// Deterministic rewrite may add at most this many queries beyond the original.
pub const MAX_REWRITTEN_QUERIES: usize = 3;
/// One rewritten query is a short query; a long one is a sign the rewrite went wrong.
pub const MAX_REWRITTEN_QUERY_CHARS: usize = 120;
/// Cap on the query terms used by the title and path channels.
const MAX_QUERY_TERMS: usize = 16;
/// Freshness half-life. A chunk whose revision is this old scores 0.5.
const FRESHNESS_HALF_LIFE_MS: u64 = 90 * 24 * 60 * 60 * 1_000;
/// Neutral value for a chunk nobody has rated yet.
const NEUTRAL_FEEDBACK: f64 = 0.5;

/// The six ranking signals, each already normalised to `0.0..=1.0`.
///
/// Keeping them separate (instead of collapsing straight to one number) is what makes the ordering
/// testable per signal and the debug output explainable.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RetrievalSignals {
    /// Rank within the FTS5/BM25 channel; `0.0` when the channel did not return the chunk.
    pub lexical: f64,
    /// Rank within the semantic channel; `0.0` when semantic recall is unavailable or missed.
    pub semantic: f64,
    /// Query-term overlap with the chunk title, which also covers code symbols.
    pub title_or_symbol: f64,
    /// Query-term overlap with the source path.
    pub path_match: f64,
    /// Recency of the revision the chunk belongs to.
    pub freshness: f64,
    /// What the user said about this chunk before.
    pub user_feedback: f64,
}

impl RetrievalSignals {
    /// Applies the fixed weights. Values are clamped, so a caller cannot inflate a score by passing
    /// an out-of-range signal.
    pub fn score(&self) -> f64 {
        let clamp = |value: f64| {
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        clamp(self.lexical) * WEIGHT_LEXICAL
            + clamp(self.semantic) * WEIGHT_SEMANTIC
            + clamp(self.title_or_symbol) * WEIGHT_TITLE_OR_SYMBOL
            + clamp(self.path_match) * WEIGHT_PATH_MATCH
            + clamp(self.freshness) * WEIGHT_FRESHNESS
            + clamp(self.user_feedback) * WEIGHT_USER_FEEDBACK
    }
}

/// Turns a 1-based rank inside one recall channel into a `0.0..=1.0` signal.
///
/// Linear decay over the channel window: the best candidate scores `1.0`, the last one scores close
/// to `0.0`, and a chunk the channel did not return scores `0.0`. Linear rather than RRF on purpose
/// — the weighted sum above needs spread across the range, while RRF deliberately compresses it.
pub fn rank_signal(rank: Option<usize>) -> f64 {
    let Some(rank) = rank.filter(|rank| *rank > 0) else {
        return 0.0;
    };
    let offset = (rank - 1) as f64;
    (1.0 - offset / MAX_CHANNEL_CANDIDATES as f64).clamp(0.0, 1.0)
}

/// Freshness of a revision, halving every 90 days.
pub fn freshness_signal(updated_at_ms: u64, now_ms: u64) -> f64 {
    if updated_at_ms == 0 {
        // An unknown timestamp is not evidence of staleness, so it stays neutral.
        return NEUTRAL_FEEDBACK;
    }
    let age = now_ms.saturating_sub(updated_at_ms);
    let halvings = age as f64 / FRESHNESS_HALF_LIFE_MS as f64;
    0.5_f64.powf(halvings).clamp(0.0, 1.0)
}

/// Laplace-free feedback ratio: unrated chunks are neutral, not penalised.
///
/// `useful` and `negative` are counts over the four design feedback types
/// (`useful` positive; `irrelevant` / `outdated` / `wrong` negative).
pub fn feedback_signal(useful: u64, negative: u64) -> f64 {
    let total = useful.saturating_add(negative);
    if total == 0 {
        return NEUTRAL_FEEDBACK;
    }
    (useful as f64 / total as f64).clamp(0.0, 1.0)
}

/// Share of query terms present in the title (or symbol) text.
pub fn title_or_symbol_signal(query: &str, title: &str) -> f64 {
    let terms = query_terms(query);
    if terms.is_empty() {
        return 0.0;
    }
    let haystack = title.to_lowercase();
    let matched = terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count();
    let ratio = matched as f64 / terms.len() as f64;
    // An exact title hit is a stronger signal than partial overlap, so it saturates the channel.
    if matched == terms.len() && !haystack.trim().is_empty() {
        1.0
    } else {
        ratio.clamp(0.0, 1.0)
    }
}

/// Share of query terms present in the source path, weighted towards the file name.
pub fn path_match_signal(query: &str, path: &str) -> f64 {
    let terms = query_terms(query);
    if terms.is_empty() {
        return 0.0;
    }
    let normalized = path.replace('\\', "/").to_lowercase();
    let file_name = normalized
        .rsplit('/')
        .next()
        .unwrap_or(normalized.as_str())
        .to_owned();
    let matched = terms
        .iter()
        .filter(|term| normalized.contains(term.as_str()))
        .count();
    if matched == 0 {
        return 0.0;
    }
    let ratio = matched as f64 / terms.len() as f64;
    let in_file_name = terms
        .iter()
        .filter(|term| file_name.contains(term.as_str()))
        .count();
    // A hit inside the file name is worth more than one inside a parent directory.
    let file_ratio = in_file_name as f64 / terms.len() as f64;
    (ratio * 0.5 + file_ratio * 0.5).clamp(0.0, 1.0)
}

/// Splits a query into the terms the title and path channels match on.
///
/// ASCII words keep `_`, `-` and `.` because symbol and file names carry them (`is_private`,
/// `bootstrap.ps1`). CJK runs contribute bigrams, matching how the lexical channel already indexes
/// CJK text, so a two-character Chinese word is still searchable.
pub fn query_terms(value: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut ascii = String::new();
    let mut cjk: Vec<char> = Vec::new();

    let push = |term: String, terms: &mut Vec<String>, seen: &mut HashSet<String>| {
        if term.chars().count() < 2 || terms.len() >= MAX_QUERY_TERMS {
            return;
        }
        if seen.insert(term.clone()) {
            terms.push(term);
        }
    };

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            if !cjk.is_empty() {
                for window in cjk.windows(2) {
                    push(window.iter().collect::<String>(), &mut terms, &mut seen);
                }
                if cjk.len() == 1 {
                    push(cjk[0].to_string(), &mut terms, &mut seen);
                }
                cjk.clear();
            }
            ascii.push(ch);
            continue;
        }
        if !ascii.is_empty() {
            push(ascii.to_lowercase(), &mut terms, &mut seen);
            ascii.clear();
        }
        if is_cjk(ch) {
            cjk.push(ch);
        } else if !cjk.is_empty() {
            for window in cjk.windows(2) {
                push(window.iter().collect::<String>(), &mut terms, &mut seen);
            }
            if cjk.len() == 1 {
                push(cjk[0].to_string(), &mut terms, &mut seen);
            }
            cjk.clear();
        }
    }
    if !ascii.is_empty() {
        push(ascii.to_lowercase(), &mut terms, &mut seen);
    }
    if !cjk.is_empty() {
        for window in cjk.windows(2) {
            push(window.iter().collect::<String>(), &mut terms, &mut seen);
        }
        if cjk.len() == 1 {
            push(cjk[0].to_string(), &mut terms, &mut seen);
        }
    }
    terms
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
}

/// Host-supplied context a rewrite may add to the query.
///
/// The model never supplies these: they are read from the host (workspace name, open file) or from
/// the knowledge entity projection. `recent_entities` stays empty until Task 6 populates it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RewriteHints {
    pub project_name: Option<String>,
    pub current_file: Option<String>,
    pub recent_entities: Vec<String>,
}

/// The deterministic rewrite: never fails, never widens scope, always keeps the original query first.
///
/// Design §6.3 asks for the current project, recent entities and the current file to be folded in.
/// Everything added comes from the host, so a rewrite cannot introduce a path, a permission or a
/// scope the caller did not already have.
pub fn deterministic_rewrite(query: &str, hints: &RewriteHints) -> Vec<String> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let mut candidates = vec![query.to_owned()];
    let lower = query.to_lowercase();

    let add = |value: &str, candidates: &mut Vec<String>| {
        let value = value.trim();
        if value.is_empty() || candidates.len() > MAX_REWRITTEN_QUERIES {
            return;
        }
        let combined = format!("{query} {value}");
        if combined.to_lowercase() == lower {
            return;
        }
        if combined.chars().count() > MAX_REWRITTEN_QUERY_CHARS {
            return;
        }
        candidates.push(combined);
    };

    if let Some(project) = hints.project_name.as_deref() {
        if !lower.contains(&project.to_lowercase()) {
            add(project, &mut candidates);
        }
    }
    for entity in hints.recent_entities.iter().take(MAX_REWRITTEN_QUERIES) {
        if lower.contains(&entity.to_lowercase()) {
            continue;
        }
        add(entity, &mut candidates);
    }
    if let Some(file) = hints.current_file.as_deref() {
        // The path carries the file stem too, so the path channel already matches on the symbol.
        add(file, &mut candidates);
    }
    bound_rewrites(candidates)
}

/// Trims, de-duplicates and caps a rewrite list. Also the funnel a model rewrite goes through, so a
/// verbose model cannot turn one query into an unbounded fan-out.
pub fn bound_rewrites(values: Vec<String>) -> Vec<String> {
    let mut bounded = Vec::new();
    let mut seen = HashSet::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        let bounded_value = if trimmed.chars().count() > MAX_REWRITTEN_QUERY_CHARS {
            continue;
        } else {
            trimmed.to_owned()
        };
        let key = bounded_value.to_lowercase();
        if seen.insert(key) {
            bounded.push(bounded_value);
        }
        if bounded.len() > MAX_REWRITTEN_QUERIES {
            break;
        }
    }
    bounded
}

/// One bounded, single-shot Provider call that may propose extra queries.
///
/// The trait lives here (not in `agent`) so `knowledge` never depends on the runtime, and so tests
/// can inject a fake without a Provider. Implementations must honour the design's "failure means
/// use the original query" rule by returning `Err` rather than panicking.
#[async_trait]
pub trait QueryRewriter: Send + Sync {
    async fn rewrite(&self, query: &str, hints: &RewriteHints) -> Result<Vec<String>, String>;
}

/// The knowledge slice of the working context, in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnowledgeBudget {
    percent: usize,
    working_context_tokens: usize,
}

impl Default for KnowledgeBudget {
    fn default() -> Self {
        Self::new(
            DEFAULT_KNOWLEDGE_BUDGET_PERCENT,
            DEFAULT_WORKING_CONTEXT_TOKENS,
        )
    }
}

impl KnowledgeBudget {
    pub fn new(percent: usize, working_context_tokens: usize) -> Self {
        Self {
            percent: percent.clamp(MIN_KNOWLEDGE_BUDGET_PERCENT, MAX_KNOWLEDGE_BUDGET_PERCENT),
            working_context_tokens: working_context_tokens.max(1),
        }
    }

    pub fn percent(&self) -> usize {
        self.percent
    }

    pub fn working_context_tokens(&self) -> usize {
        self.working_context_tokens
    }

    /// Total characters the knowledge fragments may occupy.
    pub fn max_chars(&self) -> usize {
        self.working_context_tokens
            .saturating_mul(self.percent)
            .saturating_div(100)
            .saturating_mul(CHARS_PER_TOKEN)
    }

    pub fn max_chunks(&self) -> usize {
        MAX_KNOWLEDGE_CHUNKS
    }
}

/// Options for one retrieval pass. `Default` keeps the tool path (deterministic rewrite, persisted
/// budget, no model rewrite) working unchanged.
#[derive(Default, Clone)]
pub struct SearchOptions {
    pub hints: RewriteHints,
    /// Overrides `knowledge.budget_percent`; `None` reads the persisted setting.
    pub budget_percent: Option<usize>,
    /// Overrides the working-context size; `None` uses `DEFAULT_WORKING_CONTEXT_TOKENS`.
    pub working_context_tokens: Option<usize>,
    /// The optional bounded model rewrite. `None` means deterministic rewrite only.
    pub rewriter: Option<Arc<dyn QueryRewriter>>,
}

impl std::fmt::Debug for SearchOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SearchOptions")
            .field("hints", &self.hints)
            .field("budget_percent", &self.budget_percent)
            .field("working_context_tokens", &self.working_context_tokens)
            .field("rewriter", &self.rewriter.is_some())
            .finish()
    }
}

/// Greedy selection of the candidates that fit the budget, in the order they were scored.
///
/// Best-first and skip-what-does-not-fit rather than stop-at-first-miss: a single oversized chunk
/// must not hide every smaller candidate behind it. The chunk cap is applied independently of the
/// byte budget, because the design caps both.
pub fn select_within_budget(expanded_chars: &[usize], budget_chars: usize) -> Vec<usize> {
    let mut selected = Vec::new();
    let mut used = 0usize;
    for (index, chars) in expanded_chars.iter().enumerate() {
        if selected.len() >= MAX_KNOWLEDGE_CHUNKS {
            break;
        }
        if used.saturating_add(*chars) > budget_chars {
            continue;
        }
        used = used.saturating_add(*chars);
        selected.push(index);
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn signals() -> RetrievalSignals {
        RetrievalSignals::default()
    }

    #[test]
    fn the_weights_are_fixed_and_sum_to_one() {
        let total = WEIGHT_LEXICAL
            + WEIGHT_SEMANTIC
            + WEIGHT_TITLE_OR_SYMBOL
            + WEIGHT_PATH_MATCH
            + WEIGHT_FRESHNESS
            + WEIGHT_USER_FEEDBACK;
        assert!(
            (total - 1.0).abs() < 1e-9,
            "weights must sum to 1.0: {total}"
        );
    }

    #[test]
    fn lexical_and_semantic_dominate_the_fixed_weights() {
        let lexical_only = RetrievalSignals {
            lexical: 1.0,
            ..signals()
        };
        let semantic_only = RetrievalSignals {
            semantic: 1.0,
            ..signals()
        };
        let title_only = RetrievalSignals {
            title_or_symbol: 1.0,
            ..signals()
        };
        let path_only = RetrievalSignals {
            path_match: 1.0,
            ..signals()
        };
        let freshness_only = RetrievalSignals {
            freshness: 1.0,
            ..signals()
        };
        let feedback_only = RetrievalSignals {
            user_feedback: 1.0,
            ..signals()
        };
        let both_recall = RetrievalSignals {
            lexical: 1.0,
            semantic: 1.0,
            ..signals()
        };

        assert!((lexical_only.score() - WEIGHT_LEXICAL).abs() < 1e-9);
        assert!((semantic_only.score() - WEIGHT_SEMANTIC).abs() < 1e-9);
        assert!((title_only.score() - WEIGHT_TITLE_OR_SYMBOL).abs() < 1e-9);
        assert!((path_only.score() - WEIGHT_PATH_MATCH).abs() < 1e-9);
        assert!((freshness_only.score() - WEIGHT_FRESHNESS).abs() < 1e-9);
        assert!((feedback_only.score() - WEIGHT_USER_FEEDBACK).abs() < 1e-9);
        // Both recall channels hit: strictly better than either alone.
        assert!(both_recall.score() > lexical_only.score());
        assert!((both_recall.score() - 0.70).abs() < 1e-9);
        // A recency or feedback nudge can never outrank a recall hit.
        assert!(freshness_only.score() < lexical_only.score());
        assert!(feedback_only.score() < title_only.score());
    }

    #[test]
    fn every_signal_is_ordered_by_its_own_evidence() {
        // lexical / semantic: earlier rank wins, absent channel contributes nothing.
        assert!(rank_signal(Some(1)) > rank_signal(Some(2)));
        assert!(rank_signal(Some(2)) > rank_signal(Some(23)));
        assert_eq!(rank_signal(Some(0)), 0.0);
        assert_eq!(rank_signal(None), 0.0);
        assert!((rank_signal(Some(1)) - 1.0).abs() < 1e-9);

        // title / symbol: a full-title hit saturates, a partial hit does not, a miss is zero.
        assert!((title_or_symbol_signal("bootstrap", "bootstrap.ps1") - 1.0).abs() < 1e-9);
        assert_eq!(
            title_or_symbol_signal("bootstrap", "unrelated heading"),
            0.0
        );
        assert!(
            title_or_symbol_signal("知识 检索", "知识库检索设计") > 0.0,
            "CJK bigrams must match"
        );
        assert_eq!(title_or_symbol_signal("   ", "anything"), 0.0);

        // path: a hit in the file name beats one in a parent directory.
        let in_name = path_match_signal("bootstrap", "scripts/bootstrap.ps1");
        let in_parent = path_match_signal("bootstrap", "bootstrap/notes.md");
        assert!(in_name > 0.0);
        assert!(in_parent > 0.0);
        assert!(in_name > in_parent);
        assert_eq!(path_match_signal("bootstrap", "src/lib.rs"), 0.0);

        // freshness: newer wins, and the decay is a half-life rather than a cliff.
        assert!((freshness_signal(NOW, NOW) - 1.0).abs() < 1e-9);
        assert!((freshness_signal(NOW - FRESHNESS_HALF_LIFE_MS, NOW) - 0.5).abs() < 1e-9);
        assert!(
            freshness_signal(NOW - 10, NOW) > freshness_signal(NOW - FRESHNESS_HALF_LIFE_MS, NOW)
        );
        // A clock skew that puts the revision in the future must not score above 1.0.
        assert!((freshness_signal(NOW + 10_000, NOW) - 1.0).abs() < 1e-9);
        // An unknown timestamp stays neutral instead of reading as "ancient".
        assert!((freshness_signal(0, NOW) - NEUTRAL_FEEDBACK).abs() < 1e-9);

        // feedback: unrated is neutral, useful outranks negative.
        assert!((feedback_signal(0, 0) - NEUTRAL_FEEDBACK).abs() < 1e-9);
        assert!(feedback_signal(3, 0) > feedback_signal(0, 0));
        assert!(feedback_signal(0, 3) < feedback_signal(0, 0));
        assert!((feedback_signal(1, 1) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_negative_feedback_row_demotes_a_chunk_below_an_unrated_one() {
        let base = RetrievalSignals {
            lexical: rank_signal(Some(1)),
            semantic: rank_signal(Some(1)),
            title_or_symbol: title_or_symbol_signal("schema", "schema 迁移"),
            path_match: path_match_signal("schema", "src/persistence.rs"),
            freshness: freshness_signal(NOW, NOW),
            user_feedback: feedback_signal(0, 0),
        };
        let demoted = RetrievalSignals {
            user_feedback: feedback_signal(0, 3),
            ..base
        };
        let promoted = RetrievalSignals {
            user_feedback: feedback_signal(3, 0),
            ..base
        };
        assert!(demoted.score() < base.score());
        assert!(promoted.score() > base.score());
        // The feedback weight is small on purpose: it reorders near-ties, it does not override recall.
        assert!(base.score() - demoted.score() <= WEIGHT_USER_FEEDBACK + 1e-9);
    }

    #[test]
    fn out_of_range_signals_are_clamped_instead_of_inflating_the_score() {
        let inflated = RetrievalSignals {
            lexical: 9.0,
            semantic: f64::NAN,
            title_or_symbol: f64::INFINITY,
            path_match: -3.0,
            freshness: 1.0,
            user_feedback: 1.0,
        };
        let score = inflated.score();
        assert!(score.is_finite());
        assert!(score <= 1.0, "score must stay bounded: {score}");
        // A non-finite signal is a caller bug, so it counts as *no evidence* rather than as a perfect
        // hit: NaN and +INF both collapse to 0, negatives to 0, and only the honest 1.0s survive.
        let expected = WEIGHT_LEXICAL + WEIGHT_FRESHNESS + WEIGHT_USER_FEEDBACK;
        assert!((score - expected).abs() < 1e-9, "unexpected score {score}");
        // A value above 1.0 is clamped rather than rejected, so a slightly noisy signal still counts.
        assert!(
            (RetrievalSignals {
                lexical: 1.5,
                ..signals()
            }
            .score()
                - WEIGHT_LEXICAL)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn deterministic_rewrite_keeps_the_original_first_and_adds_only_host_context() {
        let hints = RewriteHints {
            project_name: Some("k-coder".into()),
            current_file: Some("src/persistence.rs".into()),
            recent_entities: vec!["memory_repository".into()],
        };
        let rewrites = deterministic_rewrite("schema 迁移", &hints);
        assert_eq!(
            rewrites[0], "schema 迁移",
            "the original query always leads"
        );
        assert_eq!(
            rewrites.len(),
            MAX_REWRITTEN_QUERIES + 1,
            "project, entity and file fill the bounded rewrite list: {rewrites:?}"
        );
        assert!(rewrites.iter().any(|value| value.contains("k-coder")));
        assert!(
            rewrites
                .iter()
                .any(|value| value.contains("memory_repository"))
        );
        assert!(
            rewrites
                .iter()
                .any(|value| value.contains("persistence.rs"))
        );
        // Every rewrite stays a short query.
        for value in &rewrites {
            assert!(value.chars().count() <= MAX_REWRITTEN_QUERY_CHARS);
        }
        // Deterministic: the same input produces the same list.
        assert_eq!(rewrites, deterministic_rewrite("schema 迁移", &hints));
    }

    #[test]
    fn deterministic_rewrite_never_repeats_context_already_in_the_query() {
        let hints = RewriteHints {
            project_name: Some("k-coder".into()),
            current_file: None,
            recent_entities: vec!["k-coder".into()],
        };
        let rewrites = deterministic_rewrite("k-coder 的 schema", &hints);
        assert_eq!(rewrites, vec!["k-coder 的 schema".to_owned()]);
    }

    #[test]
    fn an_empty_query_produces_no_rewrite_at_all() {
        assert!(deterministic_rewrite("   ", &RewriteHints::default()).is_empty());
    }

    #[test]
    fn bound_rewrites_trims_deduplicates_and_caps_a_model_reply() {
        let bounded = bound_rewrites(vec![
            "  schema 迁移 ".into(),
            "SCHEMA 迁移".into(),
            "".into(),
            "   ".into(),
            "a".into(),
            "b".into(),
            "c".into(),
            "d".into(),
            "x".repeat(MAX_REWRITTEN_QUERY_CHARS + 1),
        ]);
        assert_eq!(bounded[0], "schema 迁移");
        assert_eq!(bounded.len(), MAX_REWRITTEN_QUERIES + 1);
        assert!(
            !bounded
                .iter()
                .any(|value| value.chars().count() > MAX_REWRITTEN_QUERY_CHARS)
        );
    }

    #[test]
    fn the_knowledge_budget_is_a_share_of_the_working_context() {
        let budget = KnowledgeBudget::default();
        assert_eq!(budget.percent(), DEFAULT_KNOWLEDGE_BUDGET_PERCENT);
        assert_eq!(budget.max_chunks(), MAX_KNOWLEDGE_CHUNKS);
        // 8% of 96,000 tokens, four characters per token.
        assert_eq!(budget.max_chars(), 96_000 * 8 / 100 * 4);

        // A larger context buys proportionally more room, never less.
        let larger = KnowledgeBudget::new(8, 192_000);
        assert!(larger.max_chars() > budget.max_chars());

        // A hand-edited setting cannot escape the documented range.
        assert_eq!(
            KnowledgeBudget::new(0, 96_000).percent(),
            MIN_KNOWLEDGE_BUDGET_PERCENT
        );
        assert_eq!(
            KnowledgeBudget::new(99, 96_000).percent(),
            MAX_KNOWLEDGE_BUDGET_PERCENT
        );
        // A degenerate context size still yields a usable budget object rather than a panic; the
        // honest consequence is a budget too small for any chunk.
        let tiny = KnowledgeBudget::new(8, 0);
        assert_eq!(tiny.working_context_tokens(), 1);
        assert!(tiny.max_chars() < 100);
        assert!(select_within_budget(&[1], tiny.max_chars()).is_empty());
    }

    #[test]
    fn budget_selection_keeps_the_best_first_and_skips_what_does_not_fit() {
        let sizes = [1_000, 9_000, 500, 40_000, 100];
        let budget = 12_000;
        let selected = select_within_budget(&sizes, budget);
        // The oversized third candidate is skipped rather than truncating the list.
        assert_eq!(selected, vec![0, 1, 2, 4]);
        let used: usize = selected.iter().map(|index| sizes[*index]).sum();
        assert!(used <= budget);

        // The chunk cap holds even when the byte budget is generous.
        let many = vec![1usize; MAX_KNOWLEDGE_CHUNKS + 4];
        assert_eq!(
            select_within_budget(&many, usize::MAX).len(),
            MAX_KNOWLEDGE_CHUNKS
        );
        // An empty budget selects nothing rather than panicking.
        assert!(select_within_budget(&sizes, 0).is_empty());
    }
}
