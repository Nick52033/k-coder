//! Reproducible retrieval baseline: a fixed corpus, a fixed query set and the metrics the design
//! asks for (Recall@k, MRR, citation correctness, degradation rate and latency).
//!
//! Design §11 Phase E: "用固定评测集验证权重，禁止直接根据单次反馈自动改变生产权重". A fixed
//! fixture is what makes that rule checkable: the corpus and the queries live in
//! `evals/knowledge-retrieval-baseline.json`, every run replays exactly those, and the thresholds
//! are part of the fixture rather than of the code that enforces them.
//!
//! Two deliberate choices:
//!
//! * **It runs without an embedding key.** That pins the lexical floor — the path that has to work
//!   with no credentials, no network and no quota — and keeps the numbers reproducible instead of
//!   depending on a third-party service and its latency. The degradation rate is therefore recorded
//!   rather than suppressed: every query is expected to report `lexical_only`, and *availability*
//!   (the fraction of queries whose relevant source still comes back) is what is asserted.
//! * **It replays through the real service**, not through the scoring functions directly, so the
//!   numbers include the recall channels, the fusion, the deduplication, the neighbour expansion and
//!   the budget — the parts that can actually regress.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::knowledge::{AddSourceRequest, KnowledgeService, UpsertCollectionRequest};
use crate::persistence::ProjectionDb;
use crate::providers::{CredentialError, CredentialStore};

pub const RETRIEVAL_BASELINE_FIXTURE: &str =
    include_str!("../../../evals/knowledge-retrieval-baseline.json");
pub const RETRIEVAL_BASELINE_SCHEMA_VERSION: u32 = 1;
/// The chunk cap the baseline is measured against (design §6.1 step 7).
pub const BASELINE_RESULT_LIMIT: usize = 6;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetrievalBaseline {
    schema_version: u32,
    corpus: Vec<CorpusFile>,
    queries: Vec<QueryCase>,
    thresholds: BaselineThresholds,
}

#[derive(Debug, Clone, Deserialize)]
struct CorpusFile {
    path: String,
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
struct QueryCase {
    id: String,
    query: String,
    relevant: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaselineThresholds {
    recall_at_3: f64,
    recall_at_5: f64,
    mrr: f64,
    citation_correctness: f64,
    availability: f64,
    max_p95_latency_ms: u64,
}

/// One replayed query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalQueryOutcome {
    pub id: String,
    pub query: String,
    pub retrieval_mode: String,
    pub fallback_code: Option<String>,
    pub returned: usize,
    /// 1-based rank of the first relevant source, or `None` when it never came back.
    pub first_relevant_rank: Option<usize>,
    pub latency_ms: u64,
}

/// The recorded baseline. `failures` is empty exactly when every threshold held.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalBaselineReport {
    pub schema_version: u32,
    pub corpus_file_count: usize,
    pub query_count: usize,
    pub recall_at_3: f64,
    pub recall_at_5: f64,
    pub mrr: f64,
    pub citation_correctness: f64,
    /// Fraction of queries whose retrieval mode was not `hybrid`. Without an embedding key this is
    /// expected to be `1.0`; it is recorded so a run that *has* a key can be compared against it.
    pub degradation_rate: f64,
    /// Fraction of queries whose relevant source still came back while degraded.
    pub availability: f64,
    pub p95_latency_ms: u64,
    pub total_latency_ms: u64,
    pub outcomes: Vec<RetrievalQueryOutcome>,
    pub failures: Vec<String>,
}

/// The baseline runs with no embedding key on purpose.
#[derive(Debug, Default)]
struct NoEmbeddingKey;

impl CredentialStore for NoEmbeddingKey {
    fn get_api_key(&self, _provider_id: &str) -> Result<Option<String>, CredentialError> {
        Ok(None)
    }
    fn set_api_key(&self, _provider_id: &str, _api_key: &str) -> Result<(), CredentialError> {
        Ok(())
    }
    fn delete_api_key(&self, _provider_id: &str) -> Result<(), CredentialError> {
        Ok(())
    }
}

/// Replays the fixture and reports the metrics. The temp workspace is always removed.
pub async fn run_retrieval_baseline() -> Result<RetrievalBaselineReport, String> {
    let fixture: RetrievalBaseline = serde_json::from_str(RETRIEVAL_BASELINE_FIXTURE)
        .map_err(|error| format!("the retrieval baseline fixture is invalid: {error}"))?;
    if fixture.schema_version != RETRIEVAL_BASELINE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported retrieval baseline schema {}, expected {}",
            fixture.schema_version, RETRIEVAL_BASELINE_SCHEMA_VERSION
        ));
    }
    let root = temporary_root()?;
    let workspace = root.join("workspace");
    let data = root.join("data");
    let replayed = replay(&fixture, &workspace, &data).await;
    // Best-effort cleanup: a failed replay must not leave a corpus behind.
    let _ = std::fs::remove_dir_all(&root);
    replayed
}

fn temporary_root() -> Result<PathBuf, String> {
    let root = std::env::temp_dir().join(format!("k-coder-retrieval-baseline-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    Ok(root)
}

async fn replay(
    fixture: &RetrievalBaseline,
    workspace: &Path,
    data: &Path,
) -> Result<RetrievalBaselineReport, String> {
    for file in &fixture.corpus {
        let target = workspace.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&target, &file.content).map_err(|error| error.to_string())?;
    }
    let service = KnowledgeService::new(
        ProjectionDb::open(data).map_err(|error| error.to_string())?,
        Arc::new(NoEmbeddingKey),
    );
    service
        .set_enabled(true)
        .map_err(|error| error.to_string())?;
    let collection = service
        .upsert_collection(
            workspace,
            UpsertCollectionRequest {
                id: None,
                name: "Baseline".into(),
                scope: None,
                enabled: true,
            },
        )
        .map_err(|error| error.to_string())?;
    for file in &fixture.corpus {
        let source = service
            .add_source(
                workspace,
                AddSourceRequest {
                    collection_id: collection.id.clone(),
                    workspace_relative_path: file.path.clone(),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        if let Some(job_id) = source.initial_job_id.as_deref() {
            wait_for_job(&service, job_id).await?;
        }
    }

    let mut failures = Vec::new();
    let mut outcomes = Vec::new();
    let mut latencies = Vec::new();
    let mut total_latency_ms = 0u64;
    let mut citation_checked = 0usize;
    let mut citation_correct = 0usize;
    let mut recall_at_3_hits = 0usize;
    let mut recall_at_5_hits = 0usize;
    let mut reciprocal_total = 0.0f64;
    let mut degraded = 0usize;
    let mut available = 0usize;

    for (index, case) in fixture.queries.iter().enumerate() {
        // Each query gets its own turn, exactly like a real Turn would, so citation resolution and
        // the "no citation from another turn" rule are exercised rather than bypassed.
        let thread_id = format!("baseline-thread-{index}");
        let turn_id = format!("baseline-turn-{index}");
        let started = Instant::now();
        let response = service
            .search(
                workspace,
                &thread_id,
                &turn_id,
                &case.query,
                BASELINE_RESULT_LIMIT,
            )
            .await
            .map_err(|error| error.to_string())?;
        let latency_ms = started.elapsed().as_millis() as u64;
        total_latency_ms += latency_ms;
        latencies.push(latency_ms);

        let retrieval_mode = response.metadata["retrievalMode"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned();
        let fallback_code = response.metadata["fallbackCode"]
            .as_str()
            .map(str::to_owned);
        if retrieval_mode != "hybrid" {
            degraded += 1;
        }

        let rank_of = |response: &crate::knowledge::KnowledgeSearchResponse| {
            response
                .results
                .iter()
                .position(|result| case.relevant.iter().any(|path| path == &result.path))
                .map(|position| position + 1)
        };
        let first_relevant_rank = rank_of(&response);
        if first_relevant_rank.is_some() {
            available += 1;
        } else {
            failures.push(format!(
                "{}: no relevant source in the top {BASELINE_RESULT_LIMIT} (returned: {})",
                case.id,
                response
                    .results
                    .iter()
                    .map(|result| result.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if first_relevant_rank.is_some_and(|rank| rank <= 3) {
            recall_at_3_hits += 1;
        }
        if first_relevant_rank.is_some_and(|rank| rank <= 5) {
            recall_at_5_hits += 1;
        }
        reciprocal_total += first_relevant_rank.map_or(0.0, |rank| 1.0 / rank as f64);

        // Citation correctness: every citation handed out must still resolve, in the turn that
        // received it, to the path and revision the result claims.
        for result in &response.results {
            citation_checked += 1;
            match service.read_citation(&thread_id, &turn_id, &result.citation_id, 0, 0) {
                Ok(citation)
                    if citation.path == result.path && citation.revision == result.revision =>
                {
                    citation_correct += 1;
                }
                Ok(_) => failures.push(format!(
                    "{}: citation {} resolved to a different source",
                    case.id, result.citation_id
                )),
                Err(error) => failures.push(format!(
                    "{}: citation {} did not resolve: {}",
                    case.id,
                    result.citation_id,
                    error.code()
                )),
            }
        }

        outcomes.push(RetrievalQueryOutcome {
            id: case.id.clone(),
            query: case.query.clone(),
            retrieval_mode,
            fallback_code,
            returned: response.results.len(),
            first_relevant_rank,
            latency_ms,
        });
    }

    let query_count = fixture.queries.len();
    let divisor = query_count.max(1) as f64;
    let recall_at_3 = recall_at_3_hits as f64 / divisor;
    let recall_at_5 = recall_at_5_hits as f64 / divisor;
    let mrr = reciprocal_total / divisor;
    let citation_correctness = if citation_checked == 0 {
        0.0
    } else {
        citation_correct as f64 / citation_checked as f64
    };
    let degradation_rate = degraded as f64 / divisor;
    let availability = available as f64 / divisor;

    latencies.sort_unstable();
    let p95_latency_ms = if latencies.is_empty() {
        0
    } else {
        let index = ((latencies.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(latencies.len() - 1);
        latencies[index]
    };

    // The thresholds live in the fixture, so tightening the baseline is a data change with a
    // recorded reason, not an edit buried in this function.
    let thresholds = fixture.thresholds;
    for (label, actual, minimum) in [
        ("recallAt3", recall_at_3, thresholds.recall_at_3),
        ("recallAt5", recall_at_5, thresholds.recall_at_5),
        ("mrr", mrr, thresholds.mrr),
        (
            "citationCorrectness",
            citation_correctness,
            thresholds.citation_correctness,
        ),
        ("availability", availability, thresholds.availability),
    ] {
        if actual < minimum {
            failures.push(format!(
                "{label} {actual:.3} is below the recorded threshold {minimum:.3}"
            ));
        }
    }
    if p95_latency_ms > thresholds.max_p95_latency_ms {
        failures.push(format!(
            "p95 latency {p95_latency_ms} ms exceeds the recorded bound {} ms",
            thresholds.max_p95_latency_ms
        ));
    }

    Ok(RetrievalBaselineReport {
        schema_version: RETRIEVAL_BASELINE_SCHEMA_VERSION,
        corpus_file_count: fixture.corpus.len(),
        query_count,
        recall_at_3,
        recall_at_5,
        mrr,
        citation_correctness,
        degradation_rate,
        availability,
        p95_latency_ms,
        total_latency_ms,
        outcomes,
        failures,
    })
}

async fn wait_for_job(service: &KnowledgeService, job_id: &str) -> Result<(), String> {
    for _ in 0..900 {
        match service.get_job(job_id) {
            Ok(job) if !matches!(job.state.as_str(), "queued" | "running") => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(format!("index job {job_id} never finished"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The replay is the regression gate: if a change to the recall channels, the weights, the
    /// deduplication or the budget moves any metric below the recorded threshold, this fails with
    /// the per-query outcome that explains which one.
    #[tokio::test]
    async fn the_retrieval_baseline_meets_its_recorded_thresholds() {
        let report = run_retrieval_baseline()
            .await
            .expect("the baseline must run");
        // Printed so a local run records the numbers the verification document cites, instead of
        // only reporting pass/fail.
        eprintln!(
            "retrieval baseline: recall@3={:.3} recall@5={:.3} mrr={:.3} citations={:.3} \
             degraded={:.3} availability={:.3} p95={}ms total={}ms",
            report.recall_at_3,
            report.recall_at_5,
            report.mrr,
            report.citation_correctness,
            report.degradation_rate,
            report.availability,
            report.p95_latency_ms,
            report.total_latency_ms,
        );
        assert_eq!(
            report.query_count,
            report.outcomes.len(),
            "every fixture query must be replayed"
        );
        assert_eq!(
            report.failures,
            Vec::<String>::new(),
            "the retrieval baseline regressed:\n{:#?}",
            report
        );
        assert_eq!(report.corpus_file_count, 8);
        assert_eq!(report.query_count, 8);
        // Without an embedding key every query degrades, and that is the point of the floor.
        assert_eq!(
            report.degradation_rate, 1.0,
            "the baseline is measured without an embedding key"
        );
        assert_eq!(
            report.availability, 1.0,
            "a degraded search must still answer"
        );
        assert!(
            report
                .outcomes
                .iter()
                .all(|outcome| outcome.fallback_code.as_deref()
                    == Some("KC_EMBEDDING_NOT_CONFIGURED")),
            "the reported fallback reason must be the honest one: {:#?}",
            report.outcomes
        );
    }
}
