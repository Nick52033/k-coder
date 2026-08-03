use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const LOG_GENERATIONS: usize = 3;

#[derive(Clone)]
pub struct StructuredLogger {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl StructuredLogger {
    pub fn new(data_root: &Path) -> std::io::Result<Self> {
        let directory = data_root.join("logs");
        fs::create_dir_all(&directory)?;
        Ok(Self {
            path: directory.join("runtime.jsonl"),
            lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn log(&self, level: &str, event: &str, fields: Value) -> std::io::Result<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| std::io::Error::other("log lock poisoned"))?;
        if self
            .path
            .metadata()
            .is_ok_and(|metadata| metadata.len() >= MAX_LOG_BYTES)
        {
            self.rotate()?;
        }
        let record = json!({ "timestampMs": crate::storage::now_ms(), "level": level,
            "event": event, "fields": redact(fields) });
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    fn rotate(&self) -> std::io::Result<()> {
        for generation in (1..LOG_GENERATIONS).rev() {
            let source = self.path.with_extension(format!("jsonl.{generation}"));
            let target = self
                .path
                .with_extension(format!("jsonl.{}", generation + 1));
            if source.exists() {
                fs::rename(source, target)?;
            }
        }
        if self.path.exists() {
            fs::rename(&self.path, self.path.with_extension("jsonl.1"))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogQuery {
    pub limit: Option<usize>,
    pub level: Option<String>,
    pub event: Option<String>,
    pub after_timestamp_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRecord {
    pub timestamp_ms: u64,
    pub level: String,
    pub event: String,
    pub fields: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogQueryResult {
    pub records: Vec<LogRecord>,
    pub total: usize,
}

impl StructuredLogger {
    pub fn read_logs(&self, query: LogQuery) -> std::io::Result<LogQueryResult> {
        let mut paths: Vec<PathBuf> = Vec::new();
        for generation in (1..=LOG_GENERATIONS).rev() {
            let rotated = self.path.with_extension(format!("jsonl.{generation}"));
            if rotated.exists() {
                paths.push(rotated);
            }
        }
        paths.push(self.path.clone());

        let level_filter = query
            .level
            .as_ref()
            .map(|value| value.to_lowercase());
        let event_filter = query.event.as_ref().map(|value| value.to_lowercase());
        let limit = query.limit.unwrap_or(200).min(2000);

        let mut records: Vec<LogRecord> = Vec::new();
        for path in &paths {
            let file = match File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for line in std::io::BufReader::new(file).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => continue,
                };
                let value: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if let Some(level) = &level_filter {
                    let record_level = value
                        .get("level")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_lowercase();
                    if record_level != *level {
                        continue;
                    }
                }
                if let Some(event) = &event_filter {
                    let record_event = value
                        .get("event")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_lowercase();
                    if record_event != *event {
                        continue;
                    }
                }
                if let Some(after) = query.after_timestamp_ms {
                    let ts = value
                        .get("timestampMs")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    if ts <= after {
                        continue;
                    }
                }
                records.push(LogRecord {
                    timestamp_ms: value
                        .get("timestampMs")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    level: value
                        .get("level")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    event: value
                        .get("event")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    fields: value.get("fields").cloned().unwrap_or(Value::Null),
                });
            }
        }

        records.sort_by(|a, b| a.timestamp_ms.cmp(&b.timestamp_ms));
        let total = records.len();
        let start = total.saturating_sub(limit);
        let records: Vec<LogRecord> = records.split_off(start);
        Ok(LogQueryResult { records, total })
    }
}

fn redact(value: Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let lower = key.to_lowercase();
                    if ["key", "token", "secret", "authorization", "password"]
                        .iter()
                        .any(|part| lower.contains(part))
                    {
                        (key, Value::String("[REDACTED]".into()))
                    } else {
                        (key, redact(value))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(redact).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_nested_secrets() {
        let value = redact(json!({"apiKey":"abc", "nested":{"accessToken":"def", "ok":1}}));
        assert_eq!(value["apiKey"], "[REDACTED]");
        assert_eq!(value["nested"]["accessToken"], "[REDACTED]");
    }
}
