use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::store::{append_json_line, read_json_lines};
use crate::storage::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum MetricEvent {
    Provider {
        timestamp_ms: u64,
        latency_ms: u64,
        success: bool,
        input_tokens: u64,
        output_tokens: u64,
    },
    Compaction {
        timestamp_ms: u64,
        before_tokens: u64,
        after_tokens: u64,
        compacted_messages: u64,
        automatic: bool,
    },
    Tool {
        timestamp_ms: u64,
        success: bool,
    },
    Fallback {
        timestamp_ms: u64,
    },
    Task {
        timestamp_ms: u64,
        completed: bool,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub provider_calls: u64,
    pub provider_failures: u64,
    pub average_provider_latency_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub compaction_count: u64,
    pub compacted_messages: u64,
    pub estimated_context_tokens_saved: u64,
    pub tool_calls: u64,
    pub tool_success_rate: f64,
    pub fallback_count: u64,
    pub completed_tasks: u64,
    pub failed_tasks: u64,
    pub estimated_cost_usd: Option<f64>,
}

#[derive(Clone)]
pub struct RuntimeMetrics {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl RuntimeMetrics {
    pub fn new(data_root: &std::path::Path) -> Result<Self, String> {
        Ok(Self {
            path: data_root.join("advanced/metrics.jsonl"),
            lock: Arc::new(Mutex::new(())),
        })
    }
    fn record(&self, event: MetricEvent) {
        if let Ok(_guard) = self.lock.lock() {
            let _ = append_json_line(&self.path, &event);
        }
    }
    pub fn provider(&self, latency_ms: u64, success: bool, input_tokens: u64, output_tokens: u64) {
        self.record(MetricEvent::Provider {
            timestamp_ms: now_ms(),
            latency_ms,
            success,
            input_tokens,
            output_tokens,
        });
    }
    pub fn compaction(
        &self,
        before_tokens: usize,
        after_tokens: usize,
        compacted_messages: usize,
        automatic: bool,
    ) {
        self.record(MetricEvent::Compaction {
            timestamp_ms: now_ms(),
            before_tokens: before_tokens as u64,
            after_tokens: after_tokens as u64,
            compacted_messages: compacted_messages as u64,
            automatic,
        });
    }
    pub fn tool(&self, success: bool) {
        self.record(MetricEvent::Tool {
            timestamp_ms: now_ms(),
            success,
        });
    }
    pub fn fallback(&self) {
        self.record(MetricEvent::Fallback {
            timestamp_ms: now_ms(),
        });
    }
    pub fn task(&self, completed: bool) {
        self.record(MetricEvent::Task {
            timestamp_ms: now_ms(),
            completed,
        });
    }
    pub fn snapshot(&self) -> Result<MetricsSnapshot, String> {
        let _guard = self.lock.lock().map_err(|_| "metrics lock poisoned")?;
        let mut result = MetricsSnapshot::default();
        let mut latency = 0;
        let mut successful_tools = 0;
        for event in read_json_lines::<MetricEvent>(&self.path)? {
            match event {
                MetricEvent::Provider {
                    latency_ms,
                    success,
                    input_tokens,
                    output_tokens,
                    ..
                } => {
                    result.provider_calls += 1;
                    latency += latency_ms;
                    if !success {
                        result.provider_failures += 1;
                    }
                    result.input_tokens += input_tokens;
                    result.output_tokens += output_tokens;
                }
                MetricEvent::Compaction {
                    before_tokens,
                    after_tokens,
                    compacted_messages,
                    automatic: _,
                    ..
                } => {
                    result.compaction_count += 1;
                    result.compacted_messages += compacted_messages;
                    result.estimated_context_tokens_saved +=
                        before_tokens.saturating_sub(after_tokens);
                }
                MetricEvent::Tool { success, .. } => {
                    result.tool_calls += 1;
                    if success {
                        successful_tools += 1;
                    }
                }
                MetricEvent::Fallback { .. } => result.fallback_count += 1,
                MetricEvent::Task { completed, .. } => {
                    if completed {
                        result.completed_tasks += 1
                    } else {
                        result.failed_tasks += 1
                    }
                }
            }
        }
        if result.provider_calls > 0 {
            result.average_provider_latency_ms = latency / result.provider_calls;
        }
        if result.tool_calls > 0 {
            result.tool_success_rate = successful_tools as f64 / result.tool_calls as f64;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aggregates_persisted_runtime_metrics() {
        let dir = tempfile::tempdir().unwrap();
        let metrics = RuntimeMetrics::new(dir.path()).unwrap();
        metrics.provider(100, true, 10, 4);
        metrics.provider(300, false, 2, 0);
        metrics.compaction(10_000, 2_500, 12, true);
        metrics.tool(true);
        metrics.tool(false);
        metrics.fallback();
        metrics.task(true);
        let snapshot = metrics.snapshot().unwrap();
        assert_eq!(snapshot.average_provider_latency_ms, 200);
        assert_eq!(snapshot.provider_failures, 1);
        assert_eq!(snapshot.tool_success_rate, 0.5);
        assert_eq!(snapshot.fallback_count, 1);
        assert_eq!(snapshot.compaction_count, 1);
        assert_eq!(snapshot.compacted_messages, 12);
        assert_eq!(snapshot.estimated_context_tokens_saved, 7_500);
    }
}
