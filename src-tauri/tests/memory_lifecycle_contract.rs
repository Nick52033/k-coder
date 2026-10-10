use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k_coder_lib::agent::EventPublisher;
use k_coder_lib::app_state::AppState;
use k_coder_lib::memory::capture::{
    AUTO_EXTRACTION_CONSENT_VERSION, CAPTURE_TIMEOUT_SECS, CAPTURE_TOKEN_BUDGET,
    MemoryCaptureService, build_capture_summary, capture_outcome_is_parseable, capture_skip_reason,
    parse_capture_proposals,
};
use k_coder_lib::memory::{MemoryScope, MemoryScopeKind, MemorySettings};
use k_coder_lib::persistence::ProjectionDb;
use k_coder_lib::protocol::{AgentEventEnvelope, TokenUsage, TurnState};
use k_coder_lib::providers::testing::FakeProvider;
use k_coder_lib::providers::{CredentialError, CredentialStore, ProviderError, ProviderEvent};
use k_coder_lib::storage::memory_capture_repository::{
    CaptureCounts, CaptureJobStatus, MAX_CAPTURE_QUEUED_JOBS, MemoryCaptureJob,
    MemoryCaptureRepository,
};
use tempfile::TempDir;
use uuid::Uuid;

const NOW: u64 = 1_700_000_000_000;

fn scope(id: &str) -> MemoryScope {
    MemoryScope::new(MemoryScopeKind::Project, Some(id.to_owned()))
}

fn settings() -> MemorySettings {
    MemorySettings {
        enabled: true,
        auto_extraction_consent_version: AUTO_EXTRACTION_CONSENT_VERSION,
        capture_after_ms: Some(NOW),
        ..MemorySettings::default()
    }
}

fn job(id: &str, project: &str) -> MemoryCaptureJob {
    MemoryCaptureJob {
        turn_id: id.to_owned(),
        thread_id: Uuid::new_v4().to_string(),
        scope: scope(project),
        summary: "任务：修复构建\n结果：已验证使用 pnpm".into(),
        started_at_ms: NOW,
        created_at_ms: NOW,
        updated_at_ms: NOW,
        status: CaptureJobStatus::Queued,
        attempts: 0,
        counts: CaptureCounts::default(),
        reason: None,
        dream_processed: false,
    }
}

fn repository() -> (TempDir, MemoryCaptureRepository) {
    let directory = TempDir::new().unwrap();
    let repository = MemoryCaptureRepository::new(ProjectionDb::open(directory.path()).unwrap());
    repository.initialize().unwrap();
    (directory, repository)
}

fn complete(repository: &MemoryCaptureRepository, id: &str) {
    repository.claim(id).unwrap().unwrap();
    repository
        .finish(
            id,
            CaptureJobStatus::Completed,
            CaptureCounts::default(),
            Some("no_proposals"),
        )
        .unwrap();
}

#[test]
fn capture_requires_current_consent_and_a_post_enable_start() {
    assert_eq!(AUTO_EXTRACTION_CONSENT_VERSION, 2);
    let mut settings = settings();
    assert_eq!(
        capture_skip_reason(&settings, NOW - 1),
        Some("turn_started_before_enable")
    );
    assert_eq!(capture_skip_reason(&settings, NOW), None);
    settings.enabled = false;
    assert_eq!(capture_skip_reason(&settings, NOW), Some("memory_disabled"));
    settings.enabled = true;
    settings.auto_extraction_consent_version = 0;
    assert_eq!(
        capture_skip_reason(&settings, NOW),
        Some("extraction_consent_required")
    );
    settings.auto_extraction_consent_version = 1;
    assert_eq!(
        capture_skip_reason(&settings, NOW),
        Some("extraction_consent_required")
    );
    settings.auto_extraction_consent_version = AUTO_EXTRACTION_CONSENT_VERSION + 1;
    assert_eq!(
        capture_skip_reason(&settings, NOW),
        Some("extraction_consent_required")
    );
    settings.auto_extraction_consent_version = AUTO_EXTRACTION_CONSENT_VERSION;
    settings.capture_after_ms = None;
    assert!(capture_skip_reason(&settings, NOW).is_some());
}

#[test]
fn summaries_are_redacted_bounded_and_never_quote_tool_output() {
    let summary = build_capture_summary(
        &format!("实现修复 {}", "中".repeat(10_000)),
        "构建通过 API_KEY=sk-live-abcdefghijklmnop data:image/png;base64,aabbccddeeff",
        &[
            "read_file success=true".into(),
            "run_command (ok)".into(),
            "run_command success=true: PRIVATE FULL OUTPUT".into(),
            "write_file:success".into(),
        ],
    )
    .unwrap();
    assert!(summary.chars().count() <= 900);
    assert!(!summary.contains("sk-live"));
    assert!(!summary.contains("aabbccddeeff"));
    assert!(!summary.contains("PRIVATE FULL OUTPUT"));
    assert!(summary.contains("read_file (ok)"));
    assert!(summary.contains("run_command (ok)"));
    assert!(summary.contains("write_file (ok)"));
    let private = build_capture_summary(
        "实现修复",
        "保存在 D:\\private\\repo user@example.com 13800138000",
        &[],
    )
    .unwrap();
    assert!(!private.contains("private"));
    assert!(!private.contains("example.com"));
    assert!(!private.contains("13800138000"));
}

#[test]
fn capture_is_turn_idempotent_without_scanning_old_rounds() {
    let directory = TempDir::new().unwrap();
    let service = MemoryCaptureService::new(ProjectionDb::open(directory.path()).unwrap());
    let thread = Uuid::new_v4().to_string();
    let turn = Uuid::new_v4().to_string();
    assert!(
        !service
            .capture_success(
                &thread,
                &turn,
                scope("p1"),
                "任务",
                "完成",
                &[],
                NOW - 1,
                &settings()
            )
            .unwrap()
    );
    assert!(
        service
            .capture_success(
                &thread,
                &turn,
                scope("p1"),
                "任务",
                "完成",
                &[],
                NOW,
                &settings()
            )
            .unwrap()
    );
    assert!(
        !service
            .capture_success(
                &thread,
                &turn,
                scope("p1"),
                "不同内容",
                "完成",
                &[],
                NOW,
                &settings()
            )
            .unwrap()
    );
    assert_eq!(service.diagnostics().unwrap().queued_jobs, 1);
    assert!(
        service
            .capture_success(
                &thread,
                &Uuid::new_v4().to_string(),
                MemoryScope::user(),
                "任务",
                "完成",
                &[],
                NOW,
                &settings()
            )
            .is_err()
    );
    let recovered = MemoryCaptureService::new(ProjectionDb::open(directory.path()).unwrap());
    assert_eq!(recovered.diagnostics().unwrap().queued_jobs, 1);
}

#[test]
fn record_skip_only_updates_bounded_redacted_diagnostics_and_deduplicates() {
    let directory = TempDir::new().unwrap();
    let service = MemoryCaptureService::new(ProjectionDb::open(directory.path()).unwrap());
    service.record_skip("non_user_source").unwrap();
    let diagnostics = service.diagnostics().unwrap();
    assert_eq!(diagnostics.last_reason.as_deref(), Some("non_user_source"));
    assert_eq!(diagnostics.last_capture_at_ms, None);
    assert_eq!(diagnostics.last_extraction_at_ms, None);
    assert_eq!(diagnostics.queued_jobs, 0);
    assert_eq!(diagnostics.skipped_jobs, 0);
    assert_eq!(diagnostics.dream_pending_summaries, 0);
    assert_eq!(diagnostics.provider_unavailable_reason, None);
    assert!(service.summary_for_dream().unwrap().is_none());
    let path = directory.path().join("memory/capture-events.jsonl");
    let first = fs::read(&path).unwrap();
    service.record_skip("non_user_source").unwrap();
    assert_eq!(fs::read(&path).unwrap(), first);
    service.record_skip(&"中".repeat(800)).unwrap();
    assert_eq!(
        service
            .diagnostics()
            .unwrap()
            .last_reason
            .unwrap()
            .chars()
            .count(),
        500
    );
    service
        .record_skip("failure API_KEY=sk-live-abcdefghijklmnop")
        .unwrap();
    service
        .record_skip("failure D:\\private\\repo user@example.com")
        .unwrap();
    let log = fs::read_to_string(&path).unwrap();
    assert!(!log.contains("sk-live"));
    assert!(!log.contains("example.com"));
    assert!(!log.contains("private\\\\repo"));
    let recovered = MemoryCaptureService::new(ProjectionDb::open(directory.path()).unwrap());
    assert_eq!(
        recovered.diagnostics().unwrap().last_reason.as_deref(),
        Some("sensitive_detail_omitted")
    );
    assert_eq!(recovered.diagnostics().unwrap().queued_jobs, 0);
}

#[test]
fn running_jobs_recover_queued_and_failures_do_not_retry() {
    let (directory, repository) = repository();
    let id = Uuid::new_v4().to_string();
    repository.enqueue(job(&id, "p1")).unwrap();
    repository.claim(&id).unwrap().unwrap();
    let recovered = MemoryCaptureRepository::new(ProjectionDb::open(directory.path()).unwrap());
    recovered.initialize().unwrap();
    let queued = recovered.next_queued().unwrap().unwrap();
    assert_eq!(queued.turn_id, id);
    assert_eq!(queued.attempts, 1);
    assert_eq!(queued.reason.as_deref(), Some("interrupted_requeued"));
    recovered.claim(&id).unwrap().unwrap();
    recovered
        .finish(
            &id,
            CaptureJobStatus::Queued,
            CaptureCounts::default(),
            Some("cancelled_requeued"),
        )
        .unwrap();
    recovered.claim(&id).unwrap().unwrap();
    recovered
        .finish(
            &id,
            CaptureJobStatus::Failed,
            CaptureCounts::default(),
            Some("model_failed"),
        )
        .unwrap();
    assert!(recovered.next_queued().unwrap().is_none());
    assert_eq!(recovered.diagnostics().unwrap().failed_jobs, 1);
}

#[test]
fn queue_is_bounded_and_capacity_tombstones_preserve_idempotency() {
    let (_directory, repository) = repository();
    for index in 0..MAX_CAPTURE_QUEUED_JOBS + 1 {
        repository
            .enqueue(job(&format!("turn-{index:04}"), "p1"))
            .unwrap();
    }
    assert_eq!(
        repository.diagnostics().unwrap().queued_jobs,
        MAX_CAPTURE_QUEUED_JOBS as u64
    );
    assert_eq!(repository.diagnostics().unwrap().skipped_jobs, 1);
    assert_eq!(
        repository.get("turn-0000").unwrap().unwrap().status,
        CaptureJobStatus::Skipped
    );
    assert!(!repository.enqueue(job("turn-0000", "p1")).unwrap());
}

#[test]
fn dream_batches_share_scope_are_bounded_and_processed_never_revives() {
    let (directory, repository) = repository();
    for index in 0..10 {
        let id = format!("a-{index:02}");
        repository.enqueue(job(&id, "project-a")).unwrap();
        complete(&repository, &id);
    }
    repository.enqueue(job("b-00", "project-b")).unwrap();
    complete(&repository, "b-00");
    let (host_scope, summaries, ids) = repository.summary_for_dream().unwrap().unwrap();
    assert_eq!(host_scope, scope("project-a"));
    assert_eq!(summaries.len(), 8);
    assert!(
        repository
            .mark_dream_processed(&[ids[0].clone(), "b-00".into()])
            .is_err()
    );
    repository.mark_dream_processed(&ids).unwrap();
    repository.mark_dream_processed(&ids).unwrap();
    let recovered = MemoryCaptureRepository::new(ProjectionDb::open(directory.path()).unwrap());
    recovered.initialize().unwrap();
    let (_, summaries, next_ids) = recovered.summary_for_dream().unwrap().unwrap();
    assert_eq!(summaries.len(), 2);
    assert!(next_ids.iter().all(|id| !ids.contains(id)));
    recovered.mark_dream_processed(&next_ids).unwrap();
    let (host_scope, _, ids) = recovered.summary_for_dream().unwrap().unwrap();
    assert_eq!(host_scope, scope("project-b"));
    recovered.mark_dream_processed(&ids).unwrap();
    assert!(recovered.summary_for_dream().unwrap().is_none());
}

#[test]
fn malformed_version_retains_projection_and_torn_tail_is_repaired_before_append() {
    let (directory, repository) = repository();
    repository.enqueue(job("original", "p1")).unwrap();
    let path = directory.path().join("memory/capture-events.jsonl");
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(b"{torn-tail").unwrap();
    drop(file);
    let recovered = MemoryCaptureRepository::new(ProjectionDb::open(directory.path()).unwrap());
    recovered.initialize().unwrap();
    recovered.enqueue(job("after-tail", "p1")).unwrap();
    recovered.initialize().unwrap();
    assert_eq!(recovered.diagnostics().unwrap().queued_jobs, 2);
    let mut event: serde_json::Value =
        serde_json::from_str(fs::read_to_string(&path).unwrap().lines().next().unwrap()).unwrap();
    event["schemaVersion"] = serde_json::json!(999);
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{event}").unwrap();
    drop(file);
    assert!(recovered.initialize().is_err());
    assert_eq!(recovered.diagnostics().unwrap().queued_jobs, 2);
    let service = MemoryCaptureService::new(ProjectionDb::open(directory.path()).unwrap());
    assert_eq!(
        service.diagnostics().unwrap().recovery_error.as_deref(),
        Some("capture_recovery_failed")
    );
}

#[test]
fn proposals_bind_host_scope_and_only_creates_are_allowed() {
    let raw = r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"使用 pnpm","reason":"验证过"},{"operation":"delete","memoryType":"fact","content":"旧约定","reason":"不要自动删除"}]}"#;
    let drafts = parse_capture_proposals(raw, &scope("p1"), "source-turn").unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].scope, scope("p1"));
    assert_eq!(drafts[0].source_turn_id.as_deref(), Some("source-turn"));
    assert!(parse_capture_proposals(r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"x","reason":"y","scope":"user"}]}"#, &scope("p1"), "t").is_err());
    assert!(parse_capture_proposals(raw, &MemoryScope::user(), "t").is_err());
    assert!(capture_outcome_is_parseable(&TurnState::Completed));
    assert!(!capture_outcome_is_parseable(&TurnState::Failed));
    assert!(!capture_outcome_is_parseable(&TurnState::Cancelled));
    assert_eq!(CAPTURE_TOKEN_BUDGET, 4000);
    assert_eq!(CAPTURE_TIMEOUT_SECS, 90);
}

#[derive(Default)]
struct NoCredentials;

impl CredentialStore for NoCredentials {
    fn get_api_key(&self, _: &str) -> Result<Option<String>, CredentialError> {
        Ok(None)
    }
    fn set_api_key(&self, _: &str, _: &str) -> Result<(), CredentialError> {
        Ok(())
    }
    fn delete_api_key(&self, _: &str) -> Result<(), CredentialError> {
        Ok(())
    }
}

#[derive(Default)]
struct Publisher(Mutex<Vec<AgentEventEnvelope>>);

impl EventPublisher for Publisher {
    fn publish(&self, event: AgentEventEnvelope) {
        self.0.lock().unwrap().push(event);
    }
}

fn runtime_state() -> (TempDir, AppState, MemoryCaptureRepository) {
    let directory = TempDir::new().unwrap();
    let data_root = directory.path().join("data");
    let state = AppState::with_workspace_and_credentials(
        &data_root,
        directory.path(),
        Arc::new(NoCredentials),
    )
    .unwrap();
    state
        .repository()
        .projection()
        .set_setting(
            "memory.settings",
            &serde_json::to_string(&settings()).unwrap(),
        )
        .unwrap();
    let repository = MemoryCaptureRepository::new(ProjectionDb::open(&data_root).unwrap());
    repository.enqueue(job("source-turn", "p1")).unwrap();
    (directory, state, repository)
}

#[test]
fn repository_rejects_private_summaries_and_redacts_persisted_diagnostics() {
    let (directory, repository) = repository();
    let mut private = job("private-turn", "p1");
    private.summary = "API_KEY=sk-live-abcdefghijklmnop D:\\private\\repo".into();
    assert!(repository.enqueue(private).is_err());
    assert_eq!(repository.diagnostics().unwrap().queued_jobs, 0);
    repository
        .set_provider_unavailable_reason(Some("failure API_KEY=sk-live-abcdefghijklmnop"))
        .unwrap();
    repository
        .note_reason("failure D:\\private\\repo user@example.com")
        .unwrap();
    let log = fs::read_to_string(directory.path().join("memory/capture-events.jsonl")).unwrap();
    assert!(!log.contains("sk-live"));
    assert!(!log.contains("example.com"));
    assert!(!log.contains("private\\\\repo"));
    repository.initialize().unwrap();
    assert_eq!(
        repository.diagnostics().unwrap().last_reason.as_deref(),
        Some("sensitive_detail_omitted")
    );
}

#[tokio::test]
async fn disabled_or_revoked_capture_never_calls_a_provider() {
    let (_directory, state, repository) = runtime_state();
    let mut current = settings();
    current.enabled = false;
    state
        .repository()
        .projection()
        .set_setting("memory.settings", &serde_json::to_string(&current).unwrap())
        .unwrap();
    let provider = Arc::new(FakeProvider::text(&["unused"]));
    assert!(
        !state
            .memory_capture()
            .run_next_with_provider(
                &state,
                Arc::new(Publisher::default()),
                provider.clone(),
                "fake".into(),
                32000,
            )
            .await
            .unwrap()
    );
    assert!(provider.requests().is_empty());
    assert_eq!(repository.get("source-turn").unwrap().unwrap().attempts, 0);
    current.enabled = true;
    current.capture_after_ms = Some(NOW + 1);
    state
        .repository()
        .projection()
        .set_setting("memory.settings", &serde_json::to_string(&current).unwrap())
        .unwrap();
    assert!(
        state
            .memory_capture()
            .run_next_with_provider(
                &state,
                Arc::new(Publisher::default()),
                provider.clone(),
                "fake".into(),
                32000,
            )
            .await
            .unwrap()
    );
    assert!(provider.requests().is_empty());
    assert_eq!(
        repository.get("source-turn").unwrap().unwrap().status,
        CaptureJobStatus::Skipped
    );
}

#[tokio::test]
async fn unauthorized_consent_versions_are_no_ops_for_both_runtime_entries() {
    let (directory, state, repository) = runtime_state();
    let service = state.memory_capture();
    let publisher = Arc::new(Publisher::default());
    let provider = Arc::new(FakeProvider::text(&["unused"]));
    let before = serde_json::to_value(service.diagnostics().unwrap()).unwrap();
    let path = directory.path().join("data/memory/capture-events.jsonl");
    let durable_before = fs::read(&path).unwrap();
    for version in [0, 1, AUTO_EXTRACTION_CONSENT_VERSION + 1] {
        let mut current = settings();
        current.auto_extraction_consent_version = version;
        state
            .repository()
            .projection()
            .set_setting("memory.settings", &serde_json::to_string(&current).unwrap())
            .unwrap();
        assert!(!service.run_next(&state, publisher.clone()).await.unwrap());
        assert!(
            !service
                .run_next_with_provider(
                    &state,
                    publisher.clone(),
                    provider.clone(),
                    "fake".into(),
                    32000,
                )
                .await
                .unwrap()
        );
        assert!(provider.requests().is_empty());
        let queued = repository.get("source-turn").unwrap().unwrap();
        assert_eq!(queued.status, CaptureJobStatus::Queued);
        assert_eq!(queued.attempts, 0);
        assert_eq!(queued.updated_at_ms, NOW);
        assert_eq!(
            serde_json::to_value(service.diagnostics().unwrap()).unwrap(),
            before
        );
        assert_eq!(fs::read(&path).unwrap(), durable_before);
    }
    assert!(publisher.0.lock().unwrap().is_empty());
    assert!(
        state
            .repository()
            .search_threads("后台记忆提取")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!state.memory_maintenance().gate().is_running());
}

#[tokio::test]
async fn provider_unavailable_does_not_claim_a_job_or_create_a_thread() {
    let (_directory, state, repository) = runtime_state();
    let service = state.memory_capture();
    let publisher = Arc::new(Publisher::default());
    assert!(!service.run_next(&state, publisher.clone()).await.unwrap());
    assert_eq!(repository.get("source-turn").unwrap().unwrap().attempts, 0);
    assert_eq!(
        service
            .diagnostics()
            .unwrap()
            .provider_unavailable_reason
            .as_deref(),
        Some("provider_unavailable")
    );
    assert!(publisher.0.lock().unwrap().is_empty());
    assert!(
        state
            .repository()
            .search_threads("后台记忆提取")
            .await
            .unwrap()
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn capture_log_rejects_symlink_escape() {
    let directory = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), directory.path().join("memory")).unwrap();
    let repository = MemoryCaptureRepository::new(ProjectionDb::open(directory.path()).unwrap());
    assert!(repository.initialize().is_err());
    assert!(!outside.path().join("capture-events.jsonl").exists());
}

#[tokio::test]
async fn completed_extraction_runs_one_empty_registry_turn_and_records_candidates() {
    let (_directory, state, repository) = runtime_state();
    let provider = Arc::new(FakeProvider::text(&[
        r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"使用 pnpm","reason":"构建验证"}]}"#,
    ]));
    assert!(
        state
            .memory_capture()
            .run_next_with_provider(
                &state,
                Arc::new(Publisher::default()),
                provider.clone(),
                "fake".into(),
                32000
            )
            .await
            .unwrap()
    );
    assert_eq!(
        repository.get("source-turn").unwrap().unwrap().status,
        CaptureJobStatus::Completed
    );
    assert_eq!(
        repository
            .get("source-turn")
            .unwrap()
            .unwrap()
            .counts
            .pending,
        1
    );
    assert_eq!(provider.requests().len(), 1);
    assert!(provider.requests()[0].tools.is_empty());
    let candidates = state.memory().list_candidates("pending", Some(10)).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].scope_type, "project");
    assert_eq!(candidates[0].scope_id.as_deref(), Some("p1"));
    assert_eq!(candidates[0].source_turn_id.as_deref(), Some("source-turn"));
    let (dream_scope, summaries, _) = state.memory_capture().summary_for_dream().unwrap().unwrap();
    assert_eq!(dream_scope, scope("p1"));
    assert_eq!(summaries.len(), 1);
    let backgrounds = state
        .repository()
        .search_threads("后台记忆提取")
        .await
        .unwrap();
    assert_eq!(backgrounds.len(), 1);
    assert!(!backgrounds[0].in_project);
    assert!(backgrounds[0].workspace_path.is_none());
    assert!(state.memory_maintenance_idle_since_ms().await.is_some());
    assert!(!state.memory_maintenance().gate().is_running());
}

#[tokio::test]
async fn failed_turn_even_with_json_is_not_parsed_or_retried() {
    let (_directory, state, repository) = runtime_state();
    let provider = Arc::new(FakeProvider::new(vec![
        Ok(ProviderEvent::TextDelta { delta: r#"{"proposals":[{"operation":"create","memoryType":"fact","content":"不得入库","reason":"未完成"}]}"#.into() }),
        Err(ProviderError::Http { status: 400, message: "failure".into() }),
    ]));
    let service = state.memory_capture();
    assert!(
        service
            .run_next_with_provider(
                &state,
                Arc::new(Publisher::default()),
                provider.clone(),
                "fake".into(),
                32000
            )
            .await
            .is_err()
    );
    assert_eq!(
        repository.get("source-turn").unwrap().unwrap().status,
        CaptureJobStatus::Failed
    );
    assert!(service.summary_for_dream().unwrap().is_none());
    assert!(
        !service
            .run_next_with_provider(
                &state,
                Arc::new(Publisher::default()),
                provider.clone(),
                "fake".into(),
                32000
            )
            .await
            .unwrap()
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn shared_gate_blocks_extraction_and_cancellation_requeues_with_finish_turn() {
    let (_directory, state, repository) = runtime_state();
    let maintenance = state.memory_maintenance();
    let lease = maintenance.gate().try_begin(NOW).unwrap();
    let provider = Arc::new(FakeProvider::text(&["unused"]));
    assert!(
        !state
            .memory_capture()
            .run_next_with_provider(
                &state,
                Arc::new(Publisher::default()),
                provider.clone(),
                "fake".into(),
                32000,
            )
            .await
            .unwrap()
    );
    assert!(provider.requests().is_empty());
    drop(lease);
    let provider = Arc::new(FakeProvider::text(&["never"]).with_delay(Duration::from_secs(60)));
    let service = state.memory_capture();
    let run = service.run_next_with_provider(
        &state,
        Arc::new(Publisher::default()),
        provider.clone(),
        "fake".into(),
        32000,
    );
    let cancel = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if !provider.requests().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(maintenance.cancel());
    };
    let (result, ()) = tokio::join!(run, cancel);
    assert!(result.unwrap());
    let queued = repository.get("source-turn").unwrap().unwrap();
    assert_eq!(queued.status, CaptureJobStatus::Queued);
    assert_eq!(queued.reason.as_deref(), Some("cancelled_requeued"));
    assert!(state.memory_maintenance_idle_since_ms().await.is_some());
    assert!(!maintenance.gate().is_running());
}

#[tokio::test(start_paused = true)]
async fn timeout_cancels_and_leaves_a_terminal_failed_job_and_no_active_turn() {
    let (_directory, state, repository) = runtime_state();
    let provider = Arc::new(FakeProvider::text(&["never"]).with_delay(Duration::from_secs(300)));
    let result = state
        .memory_capture()
        .run_next_with_provider(
            &state,
            Arc::new(Publisher::default()),
            provider,
            "fake".into(),
            32000,
        )
        .await;
    assert_eq!(result.unwrap_err().code(), "MEM_CAPTURE_TIMEOUT");
    assert_eq!(
        repository.get("source-turn").unwrap().unwrap().status,
        CaptureJobStatus::Failed
    );
    assert!(state.memory_maintenance_idle_since_ms().await.is_some());
    assert!(!state.memory_maintenance().gate().is_running());
}

#[tokio::test]
async fn token_budget_failure_is_not_parsed() {
    let (_directory, state, repository) = runtime_state();
    let provider = Arc::new(FakeProvider::new(vec![
        Ok(ProviderEvent::Usage {
            usage: TokenUsage {
                input_tokens: 4000,
                output_tokens: 1,
                total_tokens: 4001,
            },
        }),
        Ok(ProviderEvent::Completed),
    ]));
    assert!(
        state
            .memory_capture()
            .run_next_with_provider(
                &state,
                Arc::new(Publisher::default()),
                provider,
                "fake".into(),
                32000,
            )
            .await
            .is_err()
    );
    assert_eq!(
        repository.get("source-turn").unwrap().unwrap().status,
        CaptureJobStatus::Failed
    );
    assert_eq!(
        repository
            .get("source-turn")
            .unwrap()
            .unwrap()
            .counts
            .candidates,
        0
    );
}
