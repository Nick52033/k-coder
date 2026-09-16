//! Deterministic context assembly: priority ordering, provenance and budget trimming.
//!
//! Design §5.1 fixes the order in which host-owned context may enter a Provider request:
//!
//! ```text
//! 安全策略 > 用户明确规则 > 项目 AGENTS.md > 用户确认的约束记忆
//!          > 项目事实记忆 > 当前工作记忆 > 知识库引用 > 模型推断
//! ```
//!
//! Two properties matter more than the wording of that list:
//!
//! * The order is *deterministic*. The same candidates always produce the same request, because the
//!   sort is stable and the within-tier order comes from the caller's recency-ordered input rather
//!   than from a hash container.
//! * Every candidate is *accounted for*. A fragment is injected, truncated to its per-fragment cap,
//!   dropped for budget, or withheld because the host graded it secret-bearing. Each outcome is
//!   recorded with the fragment's id, revision and scope, so the design's "每次注入都要记录
//!   memoryId、revision、scope 和是否因预算被裁剪" produces an auditable result instead of a log
//!   line someone has to grep for.
//!
//! This module owns no storage and no model protocol. It reads `MemoryRecord` projections handed to
//! it by the caller and returns plain sections, so `memory` keeps owning persistence and
//! `AgentRuntime` keeps owning the request shape.

use crate::execution::redact;
use crate::memory::entity::{MemoryStatus, MemoryType, Sensitivity};
use crate::memory::policy::{detect_sensitivity, memory_is_expired};
use crate::storage::memory_repository::MemoryRecord;

/// Design §5.1 priority tiers, most authoritative first.
///
/// The discriminants are the sort keys, so the enum's declaration order *is* the priority contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContextTier {
    /// Host safety policy. Never trimmed and never dropped: the assembler must not be able to
    /// weaken a boundary by running out of budget.
    SafetyPolicy,
    /// Rules the user wrote explicitly.
    UserRules,
    /// Project `AGENTS.md`.
    ProjectRules,
    /// Memory the user confirmed as a constraint.
    ConstraintMemory,
    /// Durable project facts, including experience bound to a task result.
    ProjectFactMemory,
    /// Working memory for the current task.
    WorkStateMemory,
    /// Knowledge-base citations.
    KnowledgeCitation,
    /// Model inference. Lowest authority: it may never displace anything above it.
    ModelInference,
}

/// Every tier in priority order. Used for exhaustive iteration and by tests that assert the design
/// order has not been reshuffled.
pub const CONTEXT_TIERS: [ContextTier; 8] = [
    ContextTier::SafetyPolicy,
    ContextTier::UserRules,
    ContextTier::ProjectRules,
    ContextTier::ConstraintMemory,
    ContextTier::ProjectFactMemory,
    ContextTier::WorkStateMemory,
    ContextTier::KnowledgeCitation,
    ContextTier::ModelInference,
];

impl ContextTier {
    pub fn rank(self) -> u8 {
        self as u8
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SafetyPolicy => "safety_policy",
            Self::UserRules => "user_rules",
            Self::ProjectRules => "project_rules",
            Self::ConstraintMemory => "constraint_memory",
            Self::ProjectFactMemory => "project_fact_memory",
            Self::WorkStateMemory => "work_state_memory",
            Self::KnowledgeCitation => "knowledge_citation",
            Self::ModelInference => "model_inference",
        }
    }

    /// Safety policy and rules are host-owned boundaries. Trimming them would silently change what
    /// the model was told, which is exactly what the design forbids.
    pub fn is_required(self) -> bool {
        matches!(
            self,
            Self::SafetyPolicy | Self::UserRules | Self::ProjectRules
        )
    }
}

/// Where a fragment came from. Kept separate from the tier because a tier answers "how much
/// authority" while this answers "which row".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FragmentSourceKind {
    SafetyPolicy,
    Rule,
    Memory,
    TaskSummary,
    Knowledge,
    Inference,
}

impl FragmentSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SafetyPolicy => "safety_policy",
            Self::Rule => "rule",
            Self::Memory => "memory",
            Self::TaskSummary => "task_summary",
            Self::Knowledge => "knowledge",
            Self::Inference => "inference",
        }
    }
}

/// Provenance for one injected fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentSource {
    pub kind: FragmentSourceKind,
    /// Memory id, citation id, rule id or a fixed host label. Never model-supplied.
    pub id: String,
    /// Memory revision or knowledge revision, when the source is versioned.
    pub revision: Option<u64>,
    /// Canonical scope string (`user`, `project:<id>`, ...), when the source is scoped.
    pub scope: Option<String>,
}

impl FragmentSource {
    pub fn new(kind: FragmentSourceKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
            revision: None,
            scope: None,
        }
    }

    pub fn with_revision(mut self, revision: u64) -> Self {
        self.revision = Some(revision);
        self
    }

    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }
}

/// A candidate context fragment before assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFragment {
    pub tier: ContextTier,
    pub source: FragmentSource,
    pub text: String,
}

impl ContextFragment {
    pub fn new(tier: ContextTier, source: FragmentSource, text: impl Into<String>) -> Self {
        Self {
            tier,
            source,
            text: text.into(),
        }
    }

    /// Host safety policy text. Required tier.
    pub fn safety_policy(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(
            ContextTier::SafetyPolicy,
            FragmentSource::new(FragmentSourceKind::SafetyPolicy, id),
            text,
        )
    }

    pub fn user_rule(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(
            ContextTier::UserRules,
            FragmentSource::new(FragmentSourceKind::Rule, id),
            text,
        )
    }

    pub fn project_rule(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(
            ContextTier::ProjectRules,
            FragmentSource::new(FragmentSourceKind::Rule, id),
            text,
        )
    }

    /// A knowledge citation. Citations are bound to a revision so the model can never be shown a
    /// chunk that no longer belongs to the active revision.
    pub fn knowledge_citation(
        citation_id: impl Into<String>,
        revision_id: Option<u64>,
        text: impl Into<String>,
    ) -> Self {
        let mut source = FragmentSource::new(FragmentSourceKind::Knowledge, citation_id);
        source.revision = revision_id;
        Self::new(ContextTier::KnowledgeCitation, source, text)
    }

    pub fn model_inference(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(
            ContextTier::ModelInference,
            FragmentSource::new(FragmentSourceKind::Inference, id),
            text,
        )
    }

    /// Builds a fragment from a persisted memory row, or `None` when the row may not be injected.
    ///
    /// Rejected rows: non-active statuses, rows already past `expires_at_ms`, and work state or
    /// experience rows past their design TTL even when no explicit expiry was stored. The TTL check
    /// is repeated here on purpose — an assembly-time rule cannot be defeated by a row written
    /// before the rule existed.
    pub fn from_memory(record: &MemoryRecord, now_ms: u64) -> Option<Self> {
        let memory_type = MemoryType::parse(&record.memory_type).ok()?;
        if MemoryStatus::parse(&record.status).ok()? != MemoryStatus::Active {
            return None;
        }
        if record.content.trim().is_empty() || memory_is_expired(record, now_ms) {
            return None;
        }
        Some(Self::new(
            tier_for_memory_type(memory_type),
            FragmentSource::new(FragmentSourceKind::Memory, record.id.clone())
                .with_revision(record.revision)
                .with_scope(scope_string(&record.scope_type, record.scope_id.as_deref())),
            record.content.clone(),
        ))
    }

    /// Builds a fragment from a bounded task-end summary (see [`super::task_summary`]).
    pub fn task_summary(
        memory_id: impl Into<String>,
        scope: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self::new(
            ContextTier::WorkStateMemory,
            FragmentSource::new(FragmentSourceKind::TaskSummary, memory_id).with_scope(scope),
            text,
        )
    }
}

fn tier_for_memory_type(memory_type: MemoryType) -> ContextTier {
    match memory_type {
        MemoryType::Instruction | MemoryType::Constraint | MemoryType::Preference => {
            ContextTier::ConstraintMemory
        }
        // Design §5.1 has no dedicated experience tier. Experience is durable like a fact, so it
        // shares the project-fact tier and is ordered inside it by recency.
        MemoryType::Fact | MemoryType::Experience => ContextTier::ProjectFactMemory,
        MemoryType::WorkState => ContextTier::WorkStateMemory,
    }
}

fn scope_string(scope_type: &str, scope_id: Option<&str>) -> String {
    match scope_id {
        Some(id) => format!("{scope_type}:{id}"),
        None => scope_type.to_owned(),
    }
}

/// What happened to one candidate fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionState {
    /// Included in full.
    Included,
    /// Included after being cut down to the per-fragment cap.
    Truncated,
    /// Dropped because the budget was already spent by higher-priority fragments.
    Trimmed,
    /// Withheld because the host graded the content secret-bearing.
    Filtered,
}

impl InjectionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Included => "included",
            Self::Truncated => "truncated",
            Self::Trimmed => "trimmed",
            Self::Filtered => "filtered",
        }
    }

    pub fn is_injected(self) -> bool {
        matches!(self, Self::Included | Self::Truncated)
    }
}

/// The audit record for one candidate fragment. Design §5.1 requires the memory id, revision, scope
/// and whether the fragment was trimmed by budget; the tier and source kind are added so one record
/// is enough to reconstruct the whole decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextInjection {
    pub tier: ContextTier,
    pub source: FragmentSource,
    pub state: InjectionState,
    /// Characters actually injected (0 for trimmed and filtered fragments).
    pub injected_chars: usize,
}

impl ContextInjection {
    pub fn is_trimmed(&self) -> bool {
        matches!(self.state, InjectionState::Trimmed)
    }
}

/// One rendered tier block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledSection {
    pub tier: ContextTier,
    pub text: String,
}

/// The assembled request fragment plus its audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssembledContext {
    pub sections: Vec<AssembledSection>,
    /// Every candidate, in priority order, whether or not it was injected.
    pub injections: Vec<ContextInjection>,
    /// True when the required tiers alone exceeded the budget. The required tiers are still
    /// injected — safety policy and rules are not negotiable — but the caller can see that the
    /// budget was not honoured and can raise it.
    pub over_budget: bool,
    pub injected_chars: usize,
    pub estimated_tokens: usize,
}

impl AssembledContext {
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Renders the injected fragments in priority order, one `[tier] text` block per tier.
    pub fn render(&self) -> String {
        self.sections
            .iter()
            .map(|section| format!("[{}] {}", section.tier.as_str(), section.text))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn trimmed(&self) -> Vec<&ContextInjection> {
        self.injections
            .iter()
            .filter(|injection| injection.is_trimmed())
            .collect()
    }

    /// Bounded audit summary for the runtime log: counts plus the ids that were dropped or
    /// withheld, so a request can be reconstructed without writing every injected body to disk.
    pub fn audit_summary(&self) -> String {
        let included = self
            .injections
            .iter()
            .filter(|injection| injection.state.is_injected())
            .count();
        let trimmed = self
            .injections
            .iter()
            .filter(|injection| injection.is_trimmed())
            .map(|injection| injection.source.id.as_str())
            .collect::<Vec<_>>();
        let filtered = self
            .injections
            .iter()
            .filter(|injection| injection.state == InjectionState::Filtered)
            .map(|injection| injection.source.id.as_str())
            .collect::<Vec<_>>();
        format!(
            "included={included} trimmed=[{}] filtered=[{}] over_budget={} tokens={}",
            trimmed.join(","),
            filtered.join(","),
            self.over_budget,
            self.estimated_tokens
        )
    }
}

/// Per-fragment cap. Memory rows may hold up to 4,000 characters, but a single row must not be able
/// to spend a large share of the request, so injection caps it well below the storage limit.
pub const MAX_FRAGMENT_CHARS: usize = 1_200;
/// Default assembly budget: about 3,000 tokens, roughly 3% of the 96,000-token working context.
pub const DEFAULT_ASSEMBLY_BUDGET_CHARS: usize = 12_000;
const CHARS_PER_TOKEN: usize = 4;

/// Host-owned assembler. It holds only the budget, so it is cheap to build per request and has no
/// interior state that could make two identical requests differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextAssembler {
    budget_chars: usize,
}

impl Default for ContextAssembler {
    fn default() -> Self {
        Self::new(DEFAULT_ASSEMBLY_BUDGET_CHARS)
    }
}

impl ContextAssembler {
    pub fn new(budget_chars: usize) -> Self {
        Self { budget_chars }
    }

    pub fn budget_chars(&self) -> usize {
        self.budget_chars
    }

    /// Orders, filters and budgets the candidate fragments.
    ///
    /// Ordering is a stable sort on the tier rank, so within a tier the caller's order wins. Callers
    /// that want recency ordering inside a tier must supply it — see [`memory_fragments`], which
    /// sorts by `(tier, updated_at desc, id)` before handing fragments over.
    pub fn assemble(&self, fragments: Vec<ContextFragment>) -> AssembledContext {
        let mut ordered = fragments;
        ordered.sort_by_key(|fragment| fragment.tier.rank());

        let mut sections = Vec::<AssembledSection>::new();
        let mut injections = Vec::<ContextInjection>::new();
        let mut used = 0usize;
        let mut over_budget = false;

        for fragment in ordered {
            let tier = fragment.tier;
            // Credential-shaped content never reaches a Provider. The host decides this; a model
            // cannot mark its own memory as safe.
            if detect_sensitivity(&fragment.text) == Sensitivity::SecretCandidate {
                injections.push(ContextInjection {
                    tier,
                    source: fragment.source,
                    state: InjectionState::Filtered,
                    injected_chars: 0,
                });
                continue;
            }
            // Redaction runs even on retained content, so a partially-secret string keeps its
            // readable parts without the credential.
            let redacted = redact(&fragment.text);
            let truncated = redacted.chars().count() > MAX_FRAGMENT_CHARS;
            let text = if truncated {
                bound_chars(&redacted, MAX_FRAGMENT_CHARS)
            } else {
                redacted
            };
            let chars = text.chars().count();
            let required = tier.is_required();
            if !required && used + chars > self.budget_chars {
                injections.push(ContextInjection {
                    tier,
                    source: fragment.source,
                    state: InjectionState::Trimmed,
                    injected_chars: 0,
                });
                continue;
            }
            if required && used + chars > self.budget_chars {
                over_budget = true;
            }
            used += chars;
            injections.push(ContextInjection {
                tier,
                source: fragment.source,
                state: if truncated {
                    InjectionState::Truncated
                } else {
                    InjectionState::Included
                },
                injected_chars: chars,
            });
            match sections.last_mut() {
                Some(section) if section.tier == tier => {
                    section.text.push('\n');
                    section.text.push_str(&text);
                }
                _ => sections.push(AssembledSection { tier, text }),
            }
        }

        AssembledContext {
            sections,
            injections,
            over_budget,
            injected_chars: used,
            estimated_tokens: used.div_ceil(CHARS_PER_TOKEN),
        }
    }
}

/// Builds injectable fragments from persisted memory rows.
///
/// Ordering inside a tier is deterministic *and* recency-aware: newest `updated_at_ms` first, ties
/// broken by id. Callers therefore do not have to sort, and two runs over the same projection
/// produce byte-identical requests.
pub fn memory_fragments(records: &[MemoryRecord], now_ms: u64) -> Vec<ContextFragment> {
    let mut candidates = records
        .iter()
        .filter_map(|record| {
            ContextFragment::from_memory(record, now_ms)
                .map(|fragment| (record.updated_at_ms, fragment))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.1
            .tier
            .rank()
            .cmp(&right.1.tier.rank())
            .then_with(|| right.0.cmp(&left.0))
            .then_with(|| left.1.source.id.cmp(&right.1.source.id))
    });
    candidates
        .into_iter()
        .map(|(_, fragment)| fragment)
        .collect()
}

/// Character-bounded truncation that always respects UTF-8 boundaries.
pub(crate) fn bound_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    let mut result = value.chars().take(max).collect::<String>();
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::entity::{MemoryScope, MemoryScopeKind};
    use crate::memory::policy::{
        DEFAULT_EXPERIENCE_TTL_DAYS, DEFAULT_WORK_STATE_TTL_DAYS, days_to_ms,
    };

    fn record(
        id: &str,
        memory_type: MemoryType,
        content: &str,
        updated_at_ms: u64,
        created_at_ms: u64,
        expires_at_ms: Option<u64>,
    ) -> MemoryRecord {
        MemoryRecord {
            id: id.to_owned(),
            scope_type: "user".to_owned(),
            scope_id: None,
            memory_type: memory_type.as_str().to_owned(),
            normalized_key: id.to_owned(),
            content: content.to_owned(),
            source_type: "user".to_owned(),
            source_ref: None,
            confidence: 1.0,
            sensitivity: Sensitivity::Normal.as_str().to_owned(),
            status: MemoryStatus::Active.as_str().to_owned(),
            revision: 3,
            expires_at_ms,
            created_at_ms,
            updated_at_ms,
        }
    }

    #[test]
    fn tier_order_matches_the_design_priority_list() {
        assert_eq!(
            CONTEXT_TIERS.map(ContextTier::as_str),
            [
                "safety_policy",
                "user_rules",
                "project_rules",
                "constraint_memory",
                "project_fact_memory",
                "work_state_memory",
                "knowledge_citation",
                "model_inference",
            ]
        );
        for pair in CONTEXT_TIERS.windows(2) {
            assert!(
                pair[0].rank() < pair[1].rank(),
                "{} must outrank {}",
                pair[0].as_str(),
                pair[1].as_str()
            );
        }
        assert!(ContextTier::SafetyPolicy.is_required());
        assert!(ContextTier::UserRules.is_required());
        assert!(ContextTier::ProjectRules.is_required());
        assert!(!ContextTier::ConstraintMemory.is_required());
        assert!(!ContextTier::ModelInference.is_required());
    }

    #[test]
    fn every_tier_overrides_every_lower_tier() {
        // Feed the candidates in a rotated order: the assembler must restore the design order.
        let mut fragments = vec![
            ContextFragment::model_inference("inference", "inference"),
            ContextFragment::knowledge_citation("citation", Some(9), "knowledge"),
            ContextFragment::task_summary("task", "thread:t1", "work state"),
            ContextFragment::new(
                ContextTier::ProjectFactMemory,
                FragmentSource::new(FragmentSourceKind::Memory, "fact"),
                "fact",
            ),
            ContextFragment::new(
                ContextTier::ConstraintMemory,
                FragmentSource::new(FragmentSourceKind::Memory, "constraint"),
                "constraint",
            ),
            ContextFragment::project_rule("agents", "project rule"),
            ContextFragment::user_rule("rule-1", "user rule"),
            ContextFragment::safety_policy("policy", "safety policy"),
        ];
        fragments.rotate_left(3);

        let assembled = ContextAssembler::default().assemble(fragments);
        assert_eq!(
            assembled
                .sections
                .iter()
                .map(|section| section.tier)
                .collect::<Vec<_>>(),
            CONTEXT_TIERS.to_vec()
        );
        let rendered = assembled.render();
        let positions = CONTEXT_TIERS.map(|tier| {
            rendered
                .find(&format!("[{}]", tier.as_str()))
                .unwrap_or_else(|| panic!("missing {} in {rendered}", tier.as_str()))
        });
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "rendered order must follow the tier order: {rendered}"
        );
    }

    #[test]
    fn assembly_is_deterministic_for_the_same_candidates() {
        let candidates = || {
            vec![
                ContextFragment::model_inference("m2", "second"),
                ContextFragment::model_inference("m1", "first"),
                ContextFragment::user_rule("r1", "rule"),
            ]
        };
        let first = ContextAssembler::default().assemble(candidates());
        let second = ContextAssembler::default().assemble(candidates());
        assert_eq!(first, second);
        // Within one tier the caller's order is preserved, so callers keep control of recency.
        let inference = first
            .sections
            .iter()
            .find(|section| section.tier == ContextTier::ModelInference)
            .expect("inference section");
        assert_eq!(inference.text, "second\nfirst");
    }

    #[test]
    fn budget_trims_lowest_priority_first_and_records_every_candidate() {
        // Budget fits the two required fragments and the constraint memory, nothing more.
        let constraint = ContextFragment::new(
            ContextTier::ConstraintMemory,
            FragmentSource::new(FragmentSourceKind::Memory, "mem-constraint"),
            "c".repeat(50),
        );
        let fact = ContextFragment::new(
            ContextTier::ProjectFactMemory,
            FragmentSource::new(FragmentSourceKind::Memory, "mem-fact")
                .with_revision(3)
                .with_scope("user"),
            "f".repeat(50),
        );
        let knowledge = ContextFragment::knowledge_citation("cite-1", Some(4), "k".repeat(50));
        let inference = ContextFragment::model_inference("inf-1", "i".repeat(50));
        let assembler = ContextAssembler::new(100);

        let assembled = assembler.assemble(vec![
            inference,
            knowledge,
            fact,
            constraint,
            ContextFragment::safety_policy("policy", "s".repeat(20)),
            ContextFragment::user_rule("rule", "r".repeat(20)),
        ]);

        assert!(!assembled.over_budget);
        // Required tiers survive; the lowest tiers are the ones that lose.
        let states = assembled
            .injections
            .iter()
            .map(|injection| (injection.tier, injection.state))
            .collect::<Vec<_>>();
        assert_eq!(
            states,
            vec![
                (ContextTier::SafetyPolicy, InjectionState::Included),
                (ContextTier::UserRules, InjectionState::Included),
                (ContextTier::ConstraintMemory, InjectionState::Included),
                (ContextTier::ProjectFactMemory, InjectionState::Trimmed),
                (ContextTier::KnowledgeCitation, InjectionState::Trimmed),
                (ContextTier::ModelInference, InjectionState::Trimmed),
            ]
        );
        assert_eq!(assembled.trimmed().len(), 3);
        // The audit trail keeps provenance for the dropped rows too.
        let dropped = assembled.trimmed()[0].clone();
        assert_eq!(dropped.source.id, "mem-fact");
        assert_eq!(dropped.source.revision, Some(3));
        assert_eq!(dropped.source.scope.as_deref(), Some("user"));
        assert!(
            assembled
                .audit_summary()
                .contains("trimmed=[mem-fact,cite-1,inf-1]")
        );
    }

    #[test]
    fn required_tiers_are_never_trimmed_and_report_overflow() {
        let assembler = ContextAssembler::new(10);
        let assembled = assembler.assemble(vec![
            ContextFragment::safety_policy("policy", "s".repeat(40)),
            ContextFragment::model_inference("inf", "i".repeat(40)),
        ]);
        assert!(assembled.over_budget, "required tiers exceeded the budget");
        assert_eq!(assembled.sections.len(), 1);
        assert_eq!(assembled.sections[0].tier, ContextTier::SafetyPolicy);
        assert_eq!(assembled.sections[0].text.len(), 40);
        assert_eq!(
            assembled.injections[1].state,
            InjectionState::Trimmed,
            "the lowest tier still loses"
        );
    }

    #[test]
    fn per_fragment_cap_truncates_on_character_boundaries() {
        let assembler = ContextAssembler::default();
        let long = "汉".repeat(MAX_FRAGMENT_CHARS + 10);
        let assembled = assembler.assemble(vec![ContextFragment::model_inference("inf", long)]);
        let text = &assembled.sections[0].text;
        assert_eq!(text.chars().count(), MAX_FRAGMENT_CHARS + 1);
        assert!(text.ends_with('…'));
        assert_eq!(assembled.injections[0].state, InjectionState::Truncated);
    }

    #[test]
    fn secret_bearing_fragments_are_filtered_before_the_provider() {
        let assembler = ContextAssembler::default();
        let assembled = assembler.assemble(vec![
            ContextFragment::new(
                ContextTier::ConstraintMemory,
                FragmentSource::new(FragmentSourceKind::Memory, "mem-secret"),
                "API_KEY=sk-live-abcdefghijklmnop",
            ),
            ContextFragment::user_rule("rule", "keep responses in Chinese"),
        ]);
        // The user rule outranks the memory tier, so it is injected first.
        assert_eq!(assembled.injections[0].tier, ContextTier::UserRules);
        assert_eq!(assembled.injections[1].state, InjectionState::Filtered);
        assert_eq!(assembled.injections[1].injected_chars, 0);
        let rendered = assembled.render();
        assert!(!rendered.contains("sk-live"));
        assert!(!rendered.contains("API_KEY"));
        assert!(rendered.contains("keep responses in Chinese"));
    }

    #[test]
    fn memory_fragments_skip_inactive_expired_and_over_ttl_rows() {
        let now = 1_000_000_000_000u64;
        let mut deleted = record("mem-deleted", MemoryType::Fact, "deleted", now, now, None);
        deleted.status = MemoryStatus::Deleted.as_str().to_owned();
        let mut archived = record("mem-archived", MemoryType::Fact, "archived", now, now, None);
        archived.status = MemoryStatus::Archived.as_str().to_owned();

        let records = vec![
            record(
                "mem-constraint",
                MemoryType::Constraint,
                "always run fmt",
                now,
                now,
                None,
            ),
            record(
                "mem-fact",
                MemoryType::Fact,
                "schema is v10",
                now,
                now,
                None,
            ),
            record(
                "mem-work",
                MemoryType::WorkState,
                "task in progress",
                now,
                now,
                None,
            ),
            record(
                "mem-exp",
                MemoryType::Experience,
                "learned something",
                now,
                now,
                None,
            ),
            record(
                "mem-expired",
                MemoryType::Fact,
                "stale",
                now,
                now,
                Some(now - 1),
            ),
            record(
                "mem-work-stale",
                MemoryType::WorkState,
                "abandoned task",
                now,
                now - days_to_ms(DEFAULT_WORK_STATE_TTL_DAYS) - 1,
                None,
            ),
            record(
                "mem-exp-stale",
                MemoryType::Experience,
                "ancient lesson",
                now,
                now - days_to_ms(DEFAULT_EXPERIENCE_TTL_DAYS) - 1,
                None,
            ),
            deleted,
            archived,
        ];

        let fragments = memory_fragments(&records, now);
        // `mem-fact` and `mem-exp` share the project-fact tier and the same `updated_at_ms`, so the
        // id is the deterministic tie-break.
        assert_eq!(
            fragments
                .iter()
                .map(|fragment| fragment.source.id.as_str())
                .collect::<Vec<_>>(),
            vec!["mem-constraint", "mem-exp", "mem-fact", "mem-work"]
        );
        assert_eq!(fragments[0].tier, ContextTier::ConstraintMemory);
        assert_eq!(fragments[1].tier, ContextTier::ProjectFactMemory);
        assert_eq!(fragments[2].tier, ContextTier::ProjectFactMemory);
        assert_eq!(fragments[3].tier, ContextTier::WorkStateMemory);
        assert_eq!(fragments[0].source.revision, Some(3));
        assert_eq!(fragments[0].source.scope.as_deref(), Some("user"));
    }

    #[test]
    fn work_state_ttl_boundary_is_exclusive_at_fourteen_days() {
        let now = 1_000_000_000_000u64;
        let at_deadline = record(
            "mem-boundary",
            MemoryType::WorkState,
            "just inside",
            now,
            now - days_to_ms(DEFAULT_WORK_STATE_TTL_DAYS),
            None,
        );
        let past_deadline = record(
            "mem-outside",
            MemoryType::WorkState,
            "just outside",
            now,
            now - days_to_ms(DEFAULT_WORK_STATE_TTL_DAYS) - 1,
            None,
        );
        // The deadline itself already counts as expired, so a 14-day-old work state is not injected.
        assert!(ContextFragment::from_memory(&at_deadline, now).is_none());
        assert!(ContextFragment::from_memory(&past_deadline, now).is_none());
        let fresh = record("mem-fresh", MemoryType::WorkState, "fresh", now, now, None);
        assert!(ContextFragment::from_memory(&fresh, now).is_some());
    }

    #[test]
    fn memory_fragments_are_ordered_by_recency_within_a_tier() {
        let now = 1_000_000_000_000u64;
        let records = vec![
            record(
                "mem-old",
                MemoryType::Fact,
                "older",
                now - 500,
                now - 500,
                None,
            ),
            record("mem-new", MemoryType::Fact, "newer", now, now, None),
            record(
                "mem-mid",
                MemoryType::Fact,
                "middle",
                now - 100,
                now - 100,
                None,
            ),
        ];
        assert_eq!(
            memory_fragments(&records, now)
                .iter()
                .map(|fragment| fragment.source.id.as_str())
                .collect::<Vec<_>>(),
            vec!["mem-new", "mem-mid", "mem-old"]
        );
    }

    #[test]
    fn unknown_memory_type_or_status_is_skipped_rather_than_guessed() {
        let now = 1_000_000_000_000u64;
        let mut unknown_type = record("mem-x", MemoryType::Fact, "content", now, now, None);
        unknown_type.memory_type = "notes".to_owned();
        let mut unknown_status = record("mem-y", MemoryType::Fact, "content", now, now, None);
        unknown_status.status = "gone".to_owned();
        let empty = record("mem-z", MemoryType::Fact, "   ", now, now, None);
        assert!(ContextFragment::from_memory(&unknown_type, now).is_none());
        assert!(ContextFragment::from_memory(&unknown_status, now).is_none());
        assert!(ContextFragment::from_memory(&empty, now).is_none());
    }

    #[test]
    fn scope_string_matches_the_canonical_scope_vocabulary() {
        let now = 1_000_000_000_000u64;
        let mut scoped = record("mem-p", MemoryType::Fact, "content", now, now, None);
        scoped.scope_type = "project".to_owned();
        scoped.scope_id = Some("proj-1".to_owned());
        let fragment = ContextFragment::from_memory(&scoped, now).expect("fragment");
        assert_eq!(fragment.source.scope.as_deref(), Some("project:proj-1"));
        // The canonical string is shared with the confirmation-token vocabulary.
        assert_eq!(
            MemoryScope::new(MemoryScopeKind::Project, Some("proj-1".into())).canonical(),
            "project:proj-1"
        );
        assert_eq!(MemoryScope::user().canonical(), "user");
    }

    #[test]
    fn bound_chars_never_splits_a_multi_byte_character() {
        assert_eq!(bound_chars("abc", 5), "abc");
        assert_eq!(bound_chars("汉语词典", 2), "汉语…");
        assert_eq!(bound_chars("汉语", 2), "汉语");
    }
}
