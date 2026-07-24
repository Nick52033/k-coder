use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordedEvaluation {
    name: String,
    task: String,
    provider_events: Vec<RecordedEvent>,
    expected_tools: Vec<String>,
    expected_completion: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RecordedEvent {
    ToolCall { name: String },
    Text { value: String },
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationReport {
    pub total: usize,
    pub passed: usize,
    pub pass_rate: f64,
    pub failures: Vec<String>,
}

pub fn run_recorded_evaluation() -> Result<EvaluationReport, String> {
    let fixtures: Vec<RecordedEvaluation> =
        serde_json::from_str(include_str!("../../../evals/phase9-recorded.json"))
            .map_err(|error| error.to_string())?;
    let mut failures = Vec::new();
    for fixture in &fixtures {
        if fixture.task.trim().is_empty() {
            failures.push(format!("{}: task is empty", fixture.name));
            continue;
        }
        let actual_tools = fixture
            .provider_events
            .iter()
            .filter_map(|event| match event {
                RecordedEvent::ToolCall { name } => Some(name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let completion = fixture
            .provider_events
            .iter()
            .filter_map(|event| match event {
                RecordedEvent::Text { value } => Some(value.as_str()),
                _ => None,
            })
            .collect::<String>();
        let completed = fixture
            .provider_events
            .iter()
            .any(|event| matches!(event, RecordedEvent::Completed));
        if actual_tools != fixture.expected_tools
            || completion != fixture.expected_completion
            || !completed
        {
            failures.push(format!(
                "{}: recorded provider contract changed",
                fixture.name
            ));
        }
    }
    let total = fixtures.len();
    let passed = total.saturating_sub(failures.len());
    Ok(EvaluationReport {
        total,
        passed,
        pass_rate: if total == 0 {
            0.0
        } else {
            passed as f64 / total as f64
        },
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recorded_programming_evaluations_pass() {
        let report = run_recorded_evaluation().unwrap();
        assert!(report.total >= 3);
        assert_eq!(report.pass_rate, 1.0, "{:?}", report.failures);
    }
}
