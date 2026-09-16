//! Memory maintenance: scheduling, the Dream maintenance turn's contract, and offline maintenance.
//!
//! Design §5.2 defines Dream as a maintenance Turn that reuses the one `AgentRuntime`: no tools,
//! single instance, cancellable, with its own budget. Its input is bounded, and its output can only
//! be candidate operations — the model never names a memory id, a scope, a time or a path.
//!
//! This module owns everything about that contract that can be decided without a Provider:
//!
//! * **When** a run may start ([`MemoryMaintenanceService::automatic_trigger`]): the 24-hour interval
//!   *and* an idle window, both host-computed.
//! * **Whether** a run may start ([`MaintenanceGate`]): one in-process lease, so a manual command and
//!   the scheduler can never overlap, plus a persisted `running_since_ms` marker so a crash is
//!   recovered as `interrupted` instead of being reported as a success.
//! * **What** the model may propose ([`parse_proposals`]): a strict, bounded, `deny_unknown_fields`
//!   schema. A proposal carrying `id`, `targetMemoryId`, `scope`, `path`, `permissions` or a
//!   timestamp is rejected outright rather than silently ignored.
//! * **What happens without a model** ([`run_offline_maintenance`]): TTL expiry and duplicate-key
//!   merging, both deterministic and both expressed as status changes so they stay auditable.
//!
//! It holds no SQL, no HTTP and no prompt-to-wire code: persistence goes through `MemoryService` and
//! `ProjectionDb`, and the turn itself is driven by `commands`.

use std::sync::{Arc, Mutex as StdMutex};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::execution::redact;
use crate::memory::MemoryError;
use crate::memory::candidate::CandidateDraft;
use crate::memory::entity::{MemoryOperation, MemoryScope, MemoryType};
use crate::memory::service::{MemoryService, MergedKeyGroup};
use crate::persistence::ProjectionDb;
use crate::storage::memory_repository::MemoryRecord;

/// Settings row key inside the shared `settings` table.
const MAINTENANCE_SETTINGS_KEY: &str = "memory.maintenance";
const MAINTENANCE_SETTINGS_SCHEMA_VERSION: u32 = 1;

/// Design §5.2 / ADR 0053: Dream is a daily job.
pub const DEFAULT_MAINTENANCE_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
/// The app must have been idle this long before an automatic run starts.
pub const DEFAULT_IDLE_AFTER_MS: u64 = 10 * 60 * 1_000;
pub const MIN_IDLE_AFTER_MS: u64 = 60 * 1_000;
pub const MAX_IDLE_AFTER_MS: u64 = DEFAULT_MAINTENANCE_INTERVAL_MS;
/// Default budget for the maintenance Turn. Dream is a background job, so it gets a small slice of
/// what a normal Turn may spend, and the budget is enforced by `AgentRuntime` itself.
pub const DEFAULT_DREAM_TOKEN_BUDGET: u64 = 20_000;
pub const MIN_DREAM_TOKEN_BUDGET: u64 = 1_000;
pub const MAX_DREAM_TOKEN_BUDGET: u64 = 200_000;
/// Upper bound on proposals accepted from one maintenance Turn.
pub const MAX_MAINTENANCE_PROPOSALS: usize = 8;
/// Upper bound on memories quoted into the prompt.
pub const MAX_MAINTENANCE_INPUT_MEMORIES: usize = 40;
/// Upper bound on task summaries quoted into the prompt.
pub const MAX_MAINTENANCE_INPUT_SUMMARIES: usize = 8;
/// Per-memory content bound inside the prompt.
const PROMPT_MEMORY_CHARS: usize = 300;
/// Per-summary bound inside the prompt.
const PROMPT_SUMMARY_CHARS: usize = 400;
/// Raw model reply bound, checked before parsing so a runaway response cannot be deserialized.
const MAX_PROPOSAL_PAYLOAD_BYTES: usize = 64 * 1_024;
/// Confidence used when the model omits the field. Deliberately below the auto-accept threshold, so
/// a silent model cannot get a proposal applied without review.
const DEFAULT_PROPOSAL_CONFIDENCE: f64 = 0.5;
/// Bound for a stored failure message.
const MAX_ERROR_CHARS: usize = 500;

/// How a maintenance run ended. Persisted, so the UI can show the last outcome across restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceOutcome {
    /// No run has been recorded yet.
    Never,
    Completed,
    Failed,
    Cancelled,
    /// The process stopped while a run was in flight. Recovered on the next start; never reported as
    /// a success, because the run never produced a result.
    Interrupted,
}

impl MaintenanceOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            _ => Self::Never,
        }
    }

    /// True when the run reached a terminal state the interval clock may advance on.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Never)
    }
}

/// Why a run started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceTrigger {
    /// The user asked for it.
    Manual,
    /// The 24-hour interval elapsed and the app was idle.
    Scheduled,
}

impl MaintenanceTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Scheduled => "scheduled",
        }
    }
}

/// Versioned maintenance settings. A stored row with an unknown schema closes the read instead of
/// being coerced, matching the memory settings contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceSettings {
    pub schema_version: u32,
    /// Master switch for automatic maintenance (offline and Dream).
    pub enabled: bool,
    /// Gates the maintenance Turn that calls a Provider. Offline maintenance still runs when this is
    /// false, which is the "no local model" path the design requires.
    pub dream_enabled: bool,
    /// Design §5.2: remote Dream must be disclosed before it can run. The host requires the
    /// acknowledgement before Dream can be switched on at all, so the disclosure can never be
    /// skipped by enabling the flag first.
    pub remote_disclosure_accepted: bool,
    /// Token budget for one maintenance Turn, enforced by `AgentRuntime`.
    pub token_budget: u64,
    pub interval_ms: u64,
    pub idle_after_ms: u64,
    /// Start of the most recent completed run; the interval clock.
    pub last_run_at_ms: Option<u64>,
    pub last_outcome: MaintenanceOutcome,
    /// Set while a run is in flight. A non-null value found at startup is a crash, not a success.
    pub running_since_ms: Option<u64>,
    /// The dedicated background thread the maintenance Turn writes to, so history is auditable and a
    /// new thread is not created on every run.
    pub thread_id: Option<String>,
}

impl Default for MaintenanceSettings {
    fn default() -> Self {
        Self {
            schema_version: MAINTENANCE_SETTINGS_SCHEMA_VERSION,
            enabled: false,
            dream_enabled: false,
            remote_disclosure_accepted: false,
            token_budget: DEFAULT_DREAM_TOKEN_BUDGET,
            interval_ms: DEFAULT_MAINTENANCE_INTERVAL_MS,
            idle_after_ms: DEFAULT_IDLE_AFTER_MS,
            last_run_at_ms: None,
            last_outcome: MaintenanceOutcome::Never,
            running_since_ms: None,
            thread_id: None,
        }
    }
}

impl MaintenanceSettings {
    /// True when a maintenance Turn may call a Provider.
    pub fn dream_runnable(&self) -> bool {
        self.dream_enabled && self.remote_disclosure_accepted
    }
}

/// One in-flight run. Held only while a lease is alive.
#[derive(Debug)]
struct ActiveRun {
    started_at_ms: u64,
    cancellation: CancellationToken,
}

/// Process-wide single-instance gate for maintenance runs.
///
/// The lease is released by `Drop`, so no error path — a failed Provider call, a cancelled turn, a
/// panic unwinding — can leave the gate stuck and silently disable maintenance forever.
#[derive(Debug, Default)]
pub struct MaintenanceGate {
    active: StdMutex<Option<ActiveRun>>,
}

impl MaintenanceGate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Claims the gate, or reports `MEM_MAINTENANCE_RUNNING` when a run is already in flight.
    pub fn try_begin(self: &Arc<Self>, now_ms: u64) -> Result<MaintenanceLease, MemoryError> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| MemoryError::Storage("maintenance gate lock poisoned".into()))?;
        if let Some(run) = active.as_ref() {
            return Err(MemoryError::coded(
                "MEM_MAINTENANCE_RUNNING",
                format!(
                    "a memory maintenance run started at {} is still in flight",
                    run.started_at_ms
                ),
            ));
        }
        let cancellation = CancellationToken::new();
        *active = Some(ActiveRun {
            started_at_ms: now_ms,
            cancellation: cancellation.clone(),
        });
        Ok(MaintenanceLease {
            gate: Arc::clone(self),
            started_at_ms: now_ms,
            cancellation,
        })
    }

    /// Cancels the in-flight run. Returns false when nothing was running.
    pub fn cancel(&self) -> bool {
        let Ok(active) = self.active.lock() else {
            return false;
        };
        match active.as_ref() {
            Some(run) => {
                run.cancellation.cancel();
                true
            }
            None => false,
        }
    }

    pub fn is_running(&self) -> bool {
        self.active
            .lock()
            .map(|active| active.is_some())
            .unwrap_or(false)
    }

    fn release(&self) {
        if let Ok(mut active) = self.active.lock() {
            *active = None;
        }
    }
}

/// Proof that a run owns the gate. Dropping it releases the gate.
#[derive(Debug)]
pub struct MaintenanceLease {
    gate: Arc<MaintenanceGate>,
    started_at_ms: u64,
    cancellation: CancellationToken,
}

impl MaintenanceLease {
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn started_at_ms(&self) -> u64 {
        self.started_at_ms
    }
}

impl Drop for MaintenanceLease {
    fn drop(&mut self) {
        self.gate.release();
    }
}

/// Settings, scheduling and the single-instance gate. Cloneable, like every other app service.
#[derive(Clone)]
pub struct MemoryMaintenanceService {
    db: ProjectionDb,
    gate: Arc<MaintenanceGate>,
}

impl MemoryMaintenanceService {
    /// Opens the service and recovers a run that was in flight when the process stopped.
    ///
    /// Recovery never reports success: the stored `running_since_ms` becomes an `interrupted`
    /// outcome, and the interval clock is advanced so a crash loop cannot retry immediately. The user
    /// can always force a retry with the manual command.
    pub fn new(db: ProjectionDb) -> Self {
        let service = Self {
            db,
            gate: MaintenanceGate::new(),
        };
        let _ = service.recover_interrupted_run();
        service
    }

    pub fn gate(&self) -> &Arc<MaintenanceGate> {
        &self.gate
    }

    fn recover_interrupted_run(&self) -> Result<(), MemoryError> {
        let mut settings = self.settings()?;
        let Some(started_at_ms) = settings.running_since_ms else {
            return Ok(());
        };
        settings.running_since_ms = None;
        settings.last_outcome = MaintenanceOutcome::Interrupted;
        settings.last_run_at_ms = Some(crate::storage::now_ms().max(started_at_ms));
        self.persist_settings(&settings)
    }

    pub fn settings(&self) -> Result<MaintenanceSettings, MemoryError> {
        let Some(raw) = self.db.setting(MAINTENANCE_SETTINGS_KEY)? else {
            return Ok(MaintenanceSettings::default());
        };
        let settings = serde_json::from_str::<MaintenanceSettings>(&raw).map_err(|_| {
            MemoryError::coded(
                "MEM_INVALID_DATA",
                "stored memory maintenance settings are not readable",
            )
        })?;
        if settings.schema_version != MAINTENANCE_SETTINGS_SCHEMA_VERSION {
            return Err(MemoryError::coded(
                "MEM_INVALID_DATA",
                format!(
                    "unsupported memory maintenance settings schema {}",
                    settings.schema_version
                ),
            ));
        }
        Ok(settings)
    }

    /// Applies user-visible settings. The disclosure gate lives here rather than in the UI, so a
    /// caller cannot switch Dream on without the acknowledgement.
    pub fn set_settings(
        &self,
        enabled: bool,
        dream_enabled: bool,
        remote_disclosure_accepted: bool,
        token_budget: u64,
        idle_after_ms: u64,
    ) -> Result<MaintenanceSettings, MemoryError> {
        if !(MIN_DREAM_TOKEN_BUDGET..=MAX_DREAM_TOKEN_BUDGET).contains(&token_budget) {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                format!(
                    "tokenBudget must be between {MIN_DREAM_TOKEN_BUDGET} and {MAX_DREAM_TOKEN_BUDGET}"
                ),
            ));
        }
        if !(MIN_IDLE_AFTER_MS..=MAX_IDLE_AFTER_MS).contains(&idle_after_ms) {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                format!("idleAfterMs must be between {MIN_IDLE_AFTER_MS} and {MAX_IDLE_AFTER_MS}"),
            ));
        }
        if dream_enabled && !remote_disclosure_accepted {
            return Err(MemoryError::coded(
                "MEM_DREAM_DISCLOSURE_REQUIRED",
                "Dream sends bounded material to the configured provider; accept the disclosure before enabling it",
            ));
        }
        let mut settings = self.settings()?;
        settings.enabled = enabled;
        settings.dream_enabled = dream_enabled;
        settings.remote_disclosure_accepted = remote_disclosure_accepted;
        settings.token_budget = token_budget;
        settings.idle_after_ms = idle_after_ms;
        self.persist_settings(&settings)?;
        Ok(settings)
    }

    fn persist_settings(&self, settings: &MaintenanceSettings) -> Result<(), MemoryError> {
        let raw = serde_json::to_string(settings)
            .map_err(|error| MemoryError::Storage(error.to_string()))?;
        Ok(self.db.set_setting(MAINTENANCE_SETTINGS_KEY, &raw)?)
    }

    /// Decides whether an automatic run may start now.
    ///
    /// Both conditions must hold: the 24-hour interval has elapsed since the last run, and the app
    /// has been idle for `idle_after_ms`. `idle_since_ms` is `None` while any Turn or subagent is
    /// active, which is what keeps maintenance out of the user's way.
    pub fn automatic_trigger(
        &self,
        settings: &MaintenanceSettings,
        now_ms: u64,
        idle_since_ms: Option<u64>,
    ) -> Option<MaintenanceTrigger> {
        if !settings.enabled {
            return None;
        }
        let interval_due = settings
            .last_run_at_ms
            .is_none_or(|last| now_ms >= last.saturating_add(settings.interval_ms));
        if !interval_due {
            return None;
        }
        let idle_since_ms = idle_since_ms?;
        if now_ms < idle_since_ms.saturating_add(settings.idle_after_ms) {
            return None;
        }
        Some(MaintenanceTrigger::Scheduled)
    }

    /// Marks a run as started, so a crash during it is recoverable.
    pub fn record_run_started(&self, now_ms: u64) -> Result<(), MemoryError> {
        let mut settings = self.settings()?;
        settings.running_since_ms = Some(now_ms);
        self.persist_settings(&settings)
    }

    /// Marks a run as finished and advances the interval clock.
    pub fn record_run_finished(
        &self,
        outcome: MaintenanceOutcome,
        now_ms: u64,
    ) -> Result<(), MemoryError> {
        let mut settings = self.settings()?;
        settings.running_since_ms = None;
        settings.last_outcome = outcome;
        settings.last_run_at_ms = Some(now_ms);
        self.persist_settings(&settings)
    }

    /// Remembers the background thread the maintenance Turn writes to.
    pub fn set_thread_id(&self, thread_id: &str) -> Result<(), MemoryError> {
        let mut settings = self.settings()?;
        settings.thread_id = Some(thread_id.to_owned());
        self.persist_settings(&settings)
    }

    /// Cancels the in-flight run, if any. Returns false when nothing was running.
    pub fn cancel(&self) -> bool {
        self.gate.cancel()
    }

    pub fn is_running(&self) -> bool {
        self.gate.is_running()
    }
}

/// Result of the deterministic half of a maintenance run.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineMaintenanceReport {
    pub expired_ids: Vec<String>,
    pub merged_groups: Vec<MergedKeyGroup>,
}

impl OfflineMaintenanceReport {
    pub fn is_empty(&self) -> bool {
        self.expired_ids.is_empty() && self.merged_groups.is_empty()
    }
}

/// Runs the deterministic maintenance steps: TTL expiry, then duplicate-key merging.
///
/// This is the path the design requires when no local model is available, and it also runs before the
/// Dream Turn so the model sees a projection that is already consistent.
pub fn run_offline_maintenance(
    service: &MemoryService,
    now_ms: u64,
) -> Result<OfflineMaintenanceReport, MemoryError> {
    let expired_ids = service.expire_due(now_ms)?;
    let merged_groups = service.merge_duplicate_keys()?;
    Ok(OfflineMaintenanceReport {
        expired_ids,
        merged_groups,
    })
}

/// Renders the bounded maintenance prompt.
///
/// Every field is bounded before it reaches the prompt, so a 4,000-character memory contributes a
/// fixed slice and a large projection cannot inflate the request. `task_summaries` carries
/// already-compressed summaries; the producer of those summaries is deliberately not part of this
/// task, so callers may pass an empty slice.
pub fn build_maintenance_prompt(
    memories: &[MemoryRecord],
    task_summaries: &[String],
    now_ms: u64,
) -> String {
    let mut listed = Vec::<String>::new();
    for record in memories.iter().take(MAX_MAINTENANCE_INPUT_MEMORIES) {
        let scope = match record.scope_id.as_deref() {
            Some(id) => format!("{}:{id}", record.scope_type),
            None => record.scope_type.clone(),
        };
        listed.push(format!(
            "- [{scope}] {} | 去重键：{} | 更新于 {} | 内容：{}",
            record.memory_type,
            // The dedup key is derived from the content, so it can carry a credential too.
            bound_chars(&redact(&record.normalized_key), 80),
            record.updated_at_ms,
            bound_chars(&redact(&record.content), PROMPT_MEMORY_CHARS)
        ));
    }
    let memories_block = if listed.is_empty() {
        "（当前没有任何记忆）".to_owned()
    } else {
        listed.join("\n")
    };
    let summaries_block = if task_summaries.is_empty() {
        "（本次没有可用的任务摘要）".to_owned()
    } else {
        task_summaries
            .iter()
            .take(MAX_MAINTENANCE_INPUT_SUMMARIES)
            .map(|summary| format!("- {}", bound_chars(&redact(summary), PROMPT_SUMMARY_CHARS)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        r#"<memory_maintenance>
你是 k-Coder 的后台记忆维护组件，当前时间戳为 {now_ms} 毫秒。
你没有工具：不能读取文件、不能执行命令、不能访问网络，也不能要求宿主执行任何操作。
你只能基于下面给出的信息，提出最多 {max_proposals} 条候选记忆操作。

只输出一个 JSON 对象，不要输出其他文字（可以包裹在 ```json 代码块中）：
{{"proposals":[{{"operation":"create","memoryType":"fact","content":"……","reason":"……","confidence":0.8}}]}}

硬性约束：
- operation 只能是 create、update、merge、delete；memoryType 只能是 preference、fact、instruction、constraint、work_state、experience。
- 任何 ID、targetMemoryId、scope、路径、权限、时间戳字段都是无效输出，出现即整份作废。
- content 必须自洽、可独立理解，1 到 4000 字符；reason 不超过 2000 字符。
- 不要重复下面已列出的记忆；完全重复的提案会被宿主丢弃。
- 只提出长期有价值、非敏感的内容。凭据、密钥、令牌、绝对路径、邮箱和手机号一律不要提出。
- 没有值得记住的内容时输出 {{"proposals":[]}}。

现有记忆（scope | 类型 | 去重键 | 更新时间 | 内容）：
{memories_block}

最近的任务摘要：
{summaries_block}
</memory_maintenance>"#,
        now_ms = now_ms,
        max_proposals = MAX_MAINTENANCE_PROPOSALS,
        memories_block = memories_block,
        summaries_block = summaries_block,
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProposalEnvelope {
    proposals: Vec<ProposalWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProposalWire {
    operation: String,
    memory_type: String,
    content: String,
    reason: String,
    #[serde(default = "default_proposal_confidence")]
    confidence: f64,
}

fn default_proposal_confidence() -> f64 {
    DEFAULT_PROPOSAL_CONFIDENCE
}

/// Parses one maintenance Turn's reply into candidate drafts.
///
/// The schema denies unknown fields, so a model that tries to smuggle `targetMemoryId`, `scope`,
/// `path`, `permissions` or a timestamp produces a hard failure rather than a quietly dropped field.
/// The host resolves the scope and the target from the deduplication key, exactly as
/// `MemoryService::record_candidate` expects.
pub fn parse_proposals(
    raw: &str,
    host_scope: &MemoryScope,
    source_turn_id: &str,
) -> Result<Vec<CandidateDraft>, MemoryError> {
    if raw.len() > MAX_PROPOSAL_PAYLOAD_BYTES {
        return Err(MemoryError::coded(
            "MEM_DREAM_INVALID_PROPOSAL",
            format!("maintenance reply exceeds {MAX_PROPOSAL_PAYLOAD_BYTES} bytes"),
        ));
    }
    let envelope = parse_envelope(raw).map_err(|detail| {
        MemoryError::coded(
            "MEM_DREAM_INVALID_PROPOSAL",
            format!("maintenance reply is not a valid proposal object: {detail}"),
        )
    })?;
    if envelope.proposals.len() > MAX_MAINTENANCE_PROPOSALS {
        return Err(MemoryError::coded(
            "MEM_DREAM_TOO_MANY_PROPOSALS",
            format!(
                "maintenance reply proposed {} operations; at most {MAX_MAINTENANCE_PROPOSALS} are accepted",
                envelope.proposals.len()
            ),
        ));
    }
    let mut drafts = Vec::with_capacity(envelope.proposals.len());
    for (index, wire) in envelope.proposals.into_iter().enumerate() {
        let operation = MemoryOperation::parse(&wire.operation).map_err(|error| {
            MemoryError::coded(
                "MEM_DREAM_INVALID_PROPOSAL",
                format!("proposal {index}: {}", error),
            )
        })?;
        let memory_type = MemoryType::parse(&wire.memory_type).map_err(|error| {
            MemoryError::coded(
                "MEM_DREAM_INVALID_PROPOSAL",
                format!("proposal {index}: {}", error),
            )
        })?;
        let mut draft = CandidateDraft::from_model(
            operation,
            memory_type,
            wire.content,
            wire.reason,
            wire.confidence,
            host_scope.clone(),
        );
        draft.source_turn_id = Some(source_turn_id.to_owned());
        draft.validate().map_err(|error| {
            MemoryError::coded(
                "MEM_DREAM_INVALID_PROPOSAL",
                format!("proposal {index}: {}", error),
            )
        })?;
        drafts.push(draft);
    }
    Ok(drafts)
}

/// Parses the reply, tolerating a fenced code block or surrounding prose.
///
/// Two attempts only, both deterministic: the trimmed reply as-is, then the first `{` through the
/// last `}`. A reply with no JSON object at all still closes the run with a failure.
fn parse_envelope(raw: &str) -> Result<ProposalEnvelope, String> {
    let trimmed = raw.trim();
    let unfenced = strip_code_fence(trimmed);
    if let Ok(envelope) = serde_json::from_str::<ProposalEnvelope>(unfenced) {
        return Ok(envelope);
    }
    let (Some(start), Some(end)) = (unfenced.find('{'), unfenced.rfind('}')) else {
        return Err("no JSON object was found".to_owned());
    };
    if end <= start {
        return Err("no JSON object was found".to_owned());
    }
    serde_json::from_str::<ProposalEnvelope>(&unfenced[start..=end])
        .map_err(|error| error.to_string())
}

/// Unwraps one ```json ... ``` fence, or returns the input unchanged.
fn strip_code_fence(value: &str) -> &str {
    let Some(rest) = value.strip_prefix("```") else {
        return value;
    };
    let rest = match rest.find('\n') {
        Some(index) => &rest[index + 1..],
        None => rest,
    };
    match rest.rfind("```") {
        Some(index) => rest[..index].trim_end(),
        None => rest,
    }
}

/// Character-bounded truncation that respects UTF-8 boundaries.
fn bound_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    let mut result = value.chars().take(max).collect::<String>();
    result.push('…');
    result
}

/// Bounded, redacted failure text safe to persist and show.
pub fn bound_failure(error: &str) -> String {
    bound_chars(&redact(error), MAX_ERROR_CHARS)
}

/// The Dream half of a maintenance run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamReport {
    pub status: DreamStatus,
    /// Proposals parsed from the model reply.
    pub proposals: usize,
    /// Candidates applied immediately (only possible for high-confidence, non-sensitive creates and
    /// updates when the user opted in).
    pub accepted: usize,
    /// Candidates queued for human review.
    pub pending: usize,
    pub error: Option<String>,
}

impl DreamReport {
    pub fn skipped() -> Self {
        Self {
            status: DreamStatus::Skipped,
            proposals: 0,
            accepted: 0,
            pending: 0,
            error: None,
        }
    }

    /// Dream could not run. `error` is bounded and redacted before it is stored.
    pub fn failed(error: impl std::fmt::Display) -> Self {
        Self {
            status: DreamStatus::Failed,
            error: Some(bound_failure(&error.to_string())),
            ..Self::skipped()
        }
    }

    /// The user cancelled the run before it produced a result.
    pub fn cancelled() -> Self {
        Self {
            status: DreamStatus::Cancelled,
            ..Self::skipped()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamStatus {
    /// Dream is off, or the disclosure was not accepted, or no Provider is configured.
    Skipped,
    Completed,
    Failed,
    Cancelled,
}

/// Everything one maintenance run produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceReport {
    pub trigger: MaintenanceTrigger,
    pub outcome: MaintenanceOutcome,
    pub offline: OfflineMaintenanceReport,
    pub dream: DreamReport,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
}

/// True when the reply mentions a key the host refuses to accept from a model.
///
/// Only used by tests today; kept next to the schema so the refusal list cannot drift away from the
/// field names the prompt forbids.
pub const FORBIDDEN_PROPOSAL_FIELDS: [&str; 8] = [
    "id",
    "targetMemoryId",
    "scope",
    "scopeType",
    "path",
    "permissions",
    "expiresAtMs",
    "createdAtMs",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::entity::{MemoryScopeKind, MemoryStatus};
    use crate::memory::service::UpsertMemoryCommand;
    use crate::storage::memory_repository::{
        MemoryEventKind, MemoryRepository, MemoryWrite, normalize_memory_key,
    };

    const NOW: u64 = 1_700_000_000_000;

    fn memory_record(
        id: &str,
        memory_type: MemoryType,
        content: &str,
        updated_at_ms: u64,
    ) -> MemoryRecord {
        MemoryRecord {
            id: id.to_owned(),
            scope_type: "user".to_owned(),
            scope_id: None,
            memory_type: memory_type.as_str().to_owned(),
            normalized_key: normalize_memory_key(content),
            content: content.to_owned(),
            source_type: "user".to_owned(),
            source_ref: None,
            confidence: 1.0,
            sensitivity: "normal".to_owned(),
            status: MemoryStatus::Active.as_str().to_owned(),
            revision: 1,
            expires_at_ms: None,
            created_at_ms: updated_at_ms,
            updated_at_ms,
        }
    }

    fn service() -> MemoryMaintenanceService {
        MemoryMaintenanceService::new(ProjectionDb::memory().unwrap())
    }

    fn memory_service() -> MemoryService {
        MemoryService::new(ProjectionDb::memory().unwrap(), true)
    }

    // ---- scheduling ----

    #[test]
    fn automatic_runs_need_the_interval_and_an_idle_window() {
        let service = service();
        let mut settings = MaintenanceSettings {
            enabled: true,
            ..MaintenanceSettings::default()
        };

        // Never run before, but the app is busy.
        assert_eq!(service.automatic_trigger(&settings, NOW, None), None);
        // Never run before, and idle long enough.
        assert_eq!(
            service.automatic_trigger(&settings, NOW, Some(NOW - settings.idle_after_ms)),
            Some(MaintenanceTrigger::Scheduled)
        );
        // Idle, but not idle long enough yet.
        assert_eq!(
            service.automatic_trigger(&settings, NOW, Some(NOW - settings.idle_after_ms + 1)),
            None
        );

        // A run 23 hours ago is not due; exactly 24 hours ago is.
        settings.last_run_at_ms = Some(NOW - DEFAULT_MAINTENANCE_INTERVAL_MS + 1);
        assert_eq!(
            service.automatic_trigger(&settings, NOW, Some(NOW - settings.idle_after_ms)),
            None
        );
        settings.last_run_at_ms = Some(NOW - DEFAULT_MAINTENANCE_INTERVAL_MS);
        assert_eq!(
            service.automatic_trigger(&settings, NOW, Some(NOW - settings.idle_after_ms)),
            Some(MaintenanceTrigger::Scheduled)
        );

        // The master switch wins over everything else.
        settings.enabled = false;
        assert_eq!(
            service.automatic_trigger(&settings, NOW, Some(NOW - settings.idle_after_ms)),
            None
        );
    }

    #[test]
    fn interrupted_runs_advance_the_interval_clock_instead_of_retrying_immediately() {
        let db = ProjectionDb::memory().unwrap();
        let service = MemoryMaintenanceService::new(db.clone());
        service
            .set_settings(true, false, false, 5_000, 120_000)
            .unwrap();
        service.record_run_started(NOW).unwrap();
        // A fresh process finds the marker and must not report a success.
        let recovered = MemoryMaintenanceService::new(db);
        let settings = recovered.settings().unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.last_outcome, MaintenanceOutcome::Interrupted);
        assert_eq!(settings.running_since_ms, None);
        assert!(
            settings.last_run_at_ms.is_some_and(|last| last >= NOW),
            "the interval clock must advance so a crash cannot loop"
        );
        assert_eq!(
            recovered.automatic_trigger(&settings, NOW + 1_000, Some(0)),
            None,
            "an interrupted run must not retry immediately"
        );
    }

    #[test]
    fn settings_round_trip_and_reject_out_of_range_values() {
        let service = service();
        assert_eq!(service.settings().unwrap(), MaintenanceSettings::default());

        let settings = service
            .set_settings(true, false, false, 5_000, 120_000)
            .unwrap();
        assert!(settings.enabled);
        assert!(!settings.dream_enabled);
        assert_eq!(service.settings().unwrap(), settings);

        assert_eq!(
            service
                .set_settings(true, false, false, MIN_DREAM_TOKEN_BUDGET - 1, 120_000)
                .unwrap_err()
                .code(),
            "MEM_INVALID_ARGUMENT"
        );
        assert_eq!(
            service
                .set_settings(true, false, false, 5_000, MIN_IDLE_AFTER_MS - 1)
                .unwrap_err()
                .code(),
            "MEM_INVALID_ARGUMENT"
        );
        assert_eq!(
            service
                .set_settings(true, false, false, MAX_DREAM_TOKEN_BUDGET + 1, 120_000)
                .unwrap_err()
                .code(),
            "MEM_INVALID_ARGUMENT"
        );
    }

    #[test]
    fn dream_cannot_be_enabled_without_the_disclosure_acknowledgement() {
        let service = service();
        assert_eq!(
            service
                .set_settings(true, true, false, 5_000, 120_000)
                .unwrap_err()
                .code(),
            "MEM_DREAM_DISCLOSURE_REQUIRED"
        );
        let settings = service
            .set_settings(true, true, true, 5_000, 120_000)
            .unwrap();
        assert!(settings.dream_runnable());
        // Offline maintenance stays available without any acknowledgement.
        let settings = service
            .set_settings(true, false, false, 5_000, 120_000)
            .unwrap();
        assert!(!settings.dream_runnable());
        assert!(settings.enabled);
    }

    #[test]
    fn a_second_run_is_refused_while_the_first_holds_the_gate() {
        let service = service();
        let lease = service.gate().try_begin(NOW).unwrap();
        assert!(service.is_running());
        let second = service.gate().try_begin(NOW);
        assert_eq!(second.unwrap_err().code(), "MEM_MAINTENANCE_RUNNING");
        assert!(!lease.cancellation().is_cancelled());
        assert_eq!(lease.started_at_ms(), NOW);

        // Cancellation reaches the in-flight token.
        assert!(service.cancel());
        assert!(lease.cancellation().is_cancelled());

        // Dropping the lease releases the gate, so a later run can start.
        drop(lease);
        assert!(!service.is_running());
        assert!(service.gate().try_begin(NOW).is_ok());
    }

    #[test]
    fn cancelling_without_a_run_reports_nothing_to_cancel() {
        let service = service();
        assert!(!service.cancel());
    }

    #[test]
    fn run_outcomes_are_recorded_and_clear_the_running_marker() {
        let service = service();
        service.record_run_started(NOW).unwrap();
        assert_eq!(service.settings().unwrap().running_since_ms, Some(NOW));
        service
            .record_run_finished(MaintenanceOutcome::Failed, NOW + 10)
            .unwrap();
        let settings = service.settings().unwrap();
        assert_eq!(settings.running_since_ms, None);
        assert_eq!(settings.last_outcome, MaintenanceOutcome::Failed);
        assert_eq!(settings.last_run_at_ms, Some(NOW + 10));
        assert!(settings.last_outcome.is_terminal());
        assert!(!MaintenanceOutcome::Never.is_terminal());
    }

    #[test]
    fn an_unreadable_settings_row_closes_the_read() {
        let db = ProjectionDb::memory().unwrap();
        db.set_setting(MAINTENANCE_SETTINGS_KEY, "{not json}")
            .unwrap();
        let service = MemoryMaintenanceService::new(db);
        assert_eq!(service.settings().unwrap_err().code(), "MEM_INVALID_DATA");
    }

    // ---- offline maintenance ----

    #[test]
    fn offline_maintenance_expires_due_memories_and_leaves_the_rest_alone() {
        let service = memory_service();
        let now = crate::storage::now_ms();
        service
            .upsert(UpsertMemoryCommand {
                memory_id: None,
                content: "长期有效的偏好".into(),
                memory_type: MemoryType::Preference,
                scope: MemoryScope::user(),
                expires_at_ms: None,
            })
            .unwrap();
        service
            .upsert(UpsertMemoryCommand {
                memory_id: None,
                content: "已经过期的临时状态".into(),
                memory_type: MemoryType::Fact,
                scope: MemoryScope::user(),
                expires_at_ms: Some(now + 1_000),
            })
            .unwrap();

        let report = run_offline_maintenance(&service, now + 2_000).unwrap();
        assert_eq!(report.expired_ids.len(), 1);
        assert!(report.merged_groups.is_empty());
        let expired_id = report.expired_ids[0].clone();
        assert_eq!(
            service.get(&expired_id).unwrap().unwrap().status,
            MemoryStatus::Expired.as_str()
        );

        // The sweep is idempotent: nothing is left to expire.
        let second = run_offline_maintenance(&service, now + 3_000).unwrap();
        assert!(second.is_empty());
    }

    #[test]
    fn offline_maintenance_merges_duplicate_deduplication_keys() {
        // Two legacy rows can share a normalized key because the Task 1 backfill mapped by id. The
        // projection's unique index covers `(scope, key, revision)`, so real duplicates differ in
        // revision — which is exactly the shape the merge has to break the tie on.
        let db = ProjectionDb::memory().unwrap();
        let service = MemoryService::new(db.clone(), true);
        let repository = MemoryRepository::new(db);
        for (id, content, revision, updated_at_ms) in [
            ("mem-old", "用 pnpm 安装依赖", 1, NOW - 10_000),
            ("mem-new", "用 pnpm 安装依赖", 2, NOW),
        ] {
            repository
                .append(MemoryEventKind::MemoryUpserted(MemoryWrite {
                    id: id.to_owned(),
                    scope_type: "user".to_owned(),
                    scope_id: None,
                    memory_type: MemoryType::Fact.as_str().to_owned(),
                    normalized_key: normalize_memory_key(content),
                    content: content.to_owned(),
                    source_type: "system".to_owned(),
                    source_ref: None,
                    confidence: 1.0,
                    sensitivity: "normal".to_owned(),
                    status: MemoryStatus::Active.as_str().to_owned(),
                    revision,
                    expires_at_ms: None,
                    created_at_ms: updated_at_ms,
                }))
                .unwrap();
        }

        let report = run_offline_maintenance(&service, NOW).unwrap();
        assert_eq!(report.merged_groups.len(), 1);
        let group = &report.merged_groups[0];
        assert_eq!(group.scope, "user");
        assert_eq!(group.kept_id, "mem-new");
        assert_eq!(group.archived_ids, vec!["mem-old".to_owned()]);
        assert_eq!(
            service.get("mem-old").unwrap().unwrap().status,
            MemoryStatus::Archived.as_str()
        );
        assert_eq!(
            service.get("mem-new").unwrap().unwrap().status,
            MemoryStatus::Active.as_str()
        );

        // A healthy projection reports nothing on the next pass.
        let second = run_offline_maintenance(&service, NOW).unwrap();
        assert!(second.is_empty());
    }

    // ---- prompt and proposal parsing ----

    #[test]
    fn the_prompt_is_bounded_and_lists_dedup_keys() {
        let memories = (0..MAX_MAINTENANCE_INPUT_MEMORIES + 10)
            .map(|index| {
                memory_record(
                    &format!("mem-{index}"),
                    MemoryType::Fact,
                    &format!("记忆 {index} {}", "长".repeat(1_000)),
                    NOW,
                )
            })
            .collect::<Vec<_>>();
        let summaries = (0..MAX_MAINTENANCE_INPUT_SUMMARIES + 5)
            .map(|index| format!("摘要 {index} {}", "长".repeat(1_000)))
            .collect::<Vec<_>>();
        let prompt = build_maintenance_prompt(&memories, &summaries, NOW);

        assert!(prompt.contains("mem-0") || prompt.contains("记忆 0"));
        assert_eq!(
            prompt.matches("- [user] ").count(),
            MAX_MAINTENANCE_INPUT_MEMORIES,
            "the memory list must be capped at {MAX_MAINTENANCE_INPUT_MEMORIES}"
        );
        assert!(
            !prompt.contains("记忆 49"),
            "the memory list must be capped"
        );
        assert!(
            !prompt.contains("摘要 12"),
            "the summary list must be capped"
        );
        // 40 memories x ~430 chars, plus 8 summaries and the fixed instructions.
        assert!(
            prompt.chars().count() < 30_000,
            "prompt must stay bounded, got {} chars",
            prompt.chars().count()
        );
        assert!(prompt.contains("proposals"));

        // An empty projection is still a valid prompt.
        let empty = build_maintenance_prompt(&[], &[], NOW);
        assert!(empty.contains("当前没有任何记忆"));
        assert!(empty.contains("没有可用的任务摘要"));
    }

    #[test]
    fn the_prompt_redacts_credentials_from_existing_memories() {
        let memories = vec![memory_record(
            "mem-secret",
            MemoryType::Fact,
            "部署读取 API_KEY=sk-live-abcdefghijklmnop",
            NOW,
        )];
        let prompt = build_maintenance_prompt(&memories, &[], NOW);
        assert!(!prompt.contains("sk-live"));
        assert!(prompt.contains("[REDACTED]"));
    }

    #[test]
    fn proposals_parse_from_a_plain_object_a_fenced_block_and_chatty_output() {
        let scope = MemoryScope::user();
        let body = r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"项目用 pnpm","reason":"构建命令","confidence":0.9}]}"#;

        for raw in [
            body.to_owned(),
            format!("```json\n{body}\n```"),
            format!("好的，这是我的提案：\n{body}\n希望有帮助。"),
        ] {
            let drafts = parse_proposals(&raw, &scope, "turn-1").unwrap();
            assert_eq!(drafts.len(), 1, "input: {raw}");
            assert_eq!(drafts[0].operation, MemoryOperation::Create);
            assert_eq!(drafts[0].memory_type, MemoryType::Fact);
            assert_eq!(drafts[0].source_turn_id.as_deref(), Some("turn-1"));
            assert!(drafts[0].target_memory_id.is_none());
            assert_eq!(drafts[0].scope, scope);
        }
    }

    #[test]
    fn an_empty_proposal_list_is_valid() {
        let drafts = parse_proposals(r#"{"proposals":[]}"#, &MemoryScope::user(), "t").unwrap();
        assert!(drafts.is_empty());
    }

    #[test]
    fn proposals_cannot_smuggle_ids_scopes_paths_or_permissions() {
        let scope = MemoryScope::user();
        for forbidden in [
            r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"x","reason":"y","id":"mem-1"}]}"#,
            r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"x","reason":"y","targetMemoryId":"mem-1"}]}"#,
            r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"x","reason":"y","scope":"user"}]}"#,
            r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"x","reason":"y","path":"D:\\code"}]}"#,
            r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"x","reason":"y","permissions":["full"]}]}"#,
            r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"x","reason":"y","expiresAtMs":1}]}"#,
        ] {
            let error = parse_proposals(forbidden, &scope, "t").unwrap_err();
            assert_eq!(
                error.code(),
                "MEM_DREAM_INVALID_PROPOSAL",
                "input: {forbidden}"
            );
        }
        // The refusal list in the constant matches the field names the tests exercise.
        assert!(FORBIDDEN_PROPOSAL_FIELDS.contains(&"targetMemoryId"));
    }

    #[test]
    fn malformed_oversized_or_unknown_proposals_close_the_run() {
        let scope = MemoryScope::user();
        assert_eq!(
            parse_proposals("no json here", &scope, "t")
                .unwrap_err()
                .code(),
            "MEM_DREAM_INVALID_PROPOSAL"
        );
        assert_eq!(
            parse_proposals(r#"{"proposals":[{"operation":"upsert","memoryType":"fact","content":"x","reason":"y"}]}"#, &scope, "t")
                .unwrap_err()
                .code(),
            "MEM_DREAM_INVALID_PROPOSAL"
        );
        assert_eq!(
            parse_proposals(r#"{"proposals":[{"operation":"create","memoryType":"notes","content":"x","reason":"y"}]}"#, &scope, "t")
                .unwrap_err()
                .code(),
            "MEM_DREAM_INVALID_PROPOSAL"
        );
        // Empty content and out-of-range confidence are rejected by the draft validator.
        assert_eq!(
            parse_proposals(r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"   ","reason":"y"}]}"#, &scope, "t")
                .unwrap_err()
                .code(),
            "MEM_DREAM_INVALID_PROPOSAL"
        );
        assert_eq!(
            parse_proposals(r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"x","reason":"y","confidence":7}]}"#, &scope, "t")
                .unwrap_err()
                .code(),
            "MEM_DREAM_INVALID_PROPOSAL"
        );

        let too_many = (0..MAX_MAINTENANCE_PROPOSALS + 1)
            .map(|index| {
                format!(
                    r#"{{"operation":"create","memoryType":"fact","content":"c{index}","reason":"r"}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            parse_proposals(&format!(r#"{{"proposals":[{too_many}]}}"#), &scope, "t")
                .unwrap_err()
                .code(),
            "MEM_DREAM_TOO_MANY_PROPOSALS"
        );

        let oversized = "x".repeat(MAX_PROPOSAL_PAYLOAD_BYTES + 1);
        assert_eq!(
            parse_proposals(&oversized, &scope, "t").unwrap_err().code(),
            "MEM_DREAM_INVALID_PROPOSAL"
        );
    }

    #[test]
    fn a_missing_confidence_stays_below_the_auto_accept_threshold() {
        let drafts = parse_proposals(
            r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"x","reason":"y"}]}"#,
            &MemoryScope::user(),
            "t",
        )
        .unwrap();
        assert_eq!(drafts[0].confidence, DEFAULT_PROPOSAL_CONFIDENCE);
        assert!(drafts[0].confidence < crate::memory::AUTO_ACCEPT_CONFIDENCE);
    }

    #[test]
    fn a_model_proposed_delete_lands_in_the_review_queue() {
        // Design: the model may propose a deletion, but the host must never apply one unattended.
        let service = memory_service();
        let created = service
            .upsert(UpsertMemoryCommand {
                memory_id: None,
                content: "临时约定".into(),
                memory_type: MemoryType::Fact,
                scope: MemoryScope::user(),
                expires_at_ms: None,
            })
            .unwrap();
        let memory_id = created.memory.id.clone();
        let drafts = parse_proposals(
            r#"{"proposals":[{"operation":"delete","memoryType":"fact","content":"临时约定","reason":"已过时","confidence":0.99}]}"#,
            &MemoryScope::user(),
            "turn-dream",
        )
        .unwrap();
        let outcome = service.record_candidate(drafts[0].clone()).unwrap();
        match outcome {
            crate::memory::CandidateOutcome::Pending { candidate } => {
                assert!(candidate.requires_review);
                assert_eq!(candidate.operation, "delete");
                assert_eq!(
                    candidate.target_memory_id.as_deref(),
                    Some(memory_id.as_str())
                );
                assert_eq!(candidate.source_turn_id.as_deref(), Some("turn-dream"));
            }
            other => panic!("a model-proposed delete must require review, got {other:?}"),
        }
        // Nothing is touched before a human decides.
        assert_eq!(
            service.get(&memory_id).unwrap().unwrap().status,
            MemoryStatus::Active.as_str()
        );
    }

    #[test]
    fn proposal_drafts_feed_the_review_pipeline_for_creates_too() {
        let service = memory_service();
        let drafts = parse_proposals(
            r#"{"proposals":[{"operation":"create","memoryType":"preference","content":"回复保持中文","reason":"用户偏好","confidence":0.95}]}"#,
            &MemoryScope::user(),
            "turn-dream",
        )
        .unwrap();
        // Auto-accept is off by default, so even a high-confidence create waits for the user.
        match service.record_candidate(drafts[0].clone()).unwrap() {
            crate::memory::CandidateOutcome::Pending { candidate } => {
                assert!(candidate.requires_review);
                assert_eq!(candidate.content, "回复保持中文");
                assert_eq!(candidate.scope_type, "user");
                assert_eq!(
                    candidate.normalized_key,
                    normalize_memory_key("回复保持中文")
                );
            }
            other => panic!("expected a pending candidate, got {other:?}"),
        }
    }

    #[test]
    fn scope_is_host_resolved_and_a_thread_scope_survives_the_round_trip() {
        let scope = MemoryScope::new(MemoryScopeKind::Thread, Some("thread-1".into()));
        let drafts = parse_proposals(
            r#"{"proposals":[{"operation":"create","memoryType":"work_state","content":"正在迁移 schema","reason":"任务状态"}]}"#,
            &scope,
            "turn-dream",
        )
        .unwrap();
        assert_eq!(drafts[0].scope.canonical(), "thread:thread-1");
    }

    #[test]
    fn failure_text_is_bounded_and_redacted() {
        let bounded = bound_failure(&format!(
            "provider failed API_KEY=sk-live-abcdefghijklmnop {}",
            "x".repeat(2_000)
        ));
        assert!(!bounded.contains("sk-live"));
        assert!(bounded.chars().count() <= MAX_ERROR_CHARS + 1);
    }

    #[test]
    fn fence_stripping_handles_languages_and_missing_closers() {
        assert_eq!(strip_code_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence("```\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(strip_code_fence("```json\n{\"a\":1}"), "{\"a\":1}");
    }
}
