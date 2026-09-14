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
        fs::create_dir_all(data_root)?;
        let data_root = data_root.canonicalize()?;
        let directory = data_root.join("logs");
        reject_link(&directory)?;
        fs::create_dir_all(&directory)?;
        Ok(Self {
            path: directory.join("runtime.jsonl"),
            lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn log(&self, level: &str, event: &str, fields: Value) -> std::io::Result<()> {
        let level = level.to_ascii_lowercase();
        if !matches!(level.as_str(), "info" | "error") {
            return Ok(());
        }
        let _guard = self
            .lock
            .lock()
            .map_err(|_| std::io::Error::other("log lock poisoned"))?;
        self.validate_paths()?;
        self.append(&level, event, fields)
    }

    fn append(&self, level: &str, event: &str, fields: Value) -> std::io::Result<()> {
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

    fn paths(&self) -> Vec<PathBuf> {
        std::iter::once(self.path.clone())
            .chain((1..=LOG_GENERATIONS).map(|n| self.path.with_extension(format!("jsonl.{n}"))))
            .collect()
    }

    fn validate_paths(&self) -> std::io::Result<()> {
        let directory = self.path.parent().expect("logger has a directory");
        reject_link(directory)?;
        if directory.canonicalize()? != directory {
            return Err(std::io::Error::other("log directory changed"));
        }
        for path in self.paths() {
            reject_link(&path)?;
            if path.exists() && (!path.is_file() || path.canonicalize()? != path) {
                return Err(std::io::Error::other("invalid log file"));
            }
        }
        Ok(())
    }

    /// A user-confirmed clear shares the writer lock, and leaves one audit record.
    pub fn clear_logs(&self, confirmed: bool) -> std::io::Result<()> {
        if !confirmed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "clearing runtime logs requires confirmation",
            ));
        }
        let _guard = self
            .lock
            .lock()
            .map_err(|_| std::io::Error::other("log lock poisoned"))?;
        self.validate_paths()?;
        for path in self.paths() {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        self.append(
            "info",
            "logs_cleared",
            json!({"source": "user", "scope": "runtime_logs"}),
        )
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
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    #[serde(alias = "timestamp_ms")]
    pub timestamp_ms: u64,
    pub level: String,
    pub event: String,
    pub fields: Value,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub thread_title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogQueryResult {
    pub records: Vec<LogRecord>,
    pub total: usize,
}

impl StructuredLogger {
    pub fn read_logs(&self, query: LogQuery) -> std::io::Result<LogQueryResult> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| std::io::Error::other("log lock poisoned"))?;
        self.validate_paths()?;
        let mut paths: Vec<PathBuf> = Vec::new();
        for generation in (1..=LOG_GENERATIONS).rev() {
            let rotated = self.path.with_extension(format!("jsonl.{generation}"));
            if rotated.exists() {
                paths.push(rotated);
            }
        }
        paths.push(self.path.clone());

        let level_filter = query.level.as_ref().map(|value| value.to_lowercase());
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
                let record_level = value
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if !matches!(record_level.as_str(), "info" | "error") {
                    continue;
                }
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
                    level: record_level,
                    event: value
                        .get("event")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    fields: value.get("fields").cloned().unwrap_or(Value::Null),
                    thread_id: value
                        .pointer("/fields/threadId")
                        .and_then(Value::as_str)
                        .filter(|id| !id.trim().is_empty())
                        .map(str::to_owned),
                    thread_title: None,
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

fn reject_link(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            let linked = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let linked = metadata.file_type().is_symlink();
            if linked {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "linked log paths are not allowed",
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
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

    fn query() -> LogQuery {
        LogQuery {
            limit: None,
            level: None,
            event: None,
            after_timestamp_ms: None,
        }
    }

    #[test]
    fn retains_only_info_error_and_reads_legacy_sources_with_camel_case() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        for level in ["trace", "debug", "warn", "invalid", "INFO", "ERROR"] {
            logger
                .log(
                    level,
                    "test",
                    json!({"threadId":"conversation-a", "apiKey":"secret"}),
                )
                .unwrap();
        }
        assert_eq!(fs::read_to_string(&logger.path).unwrap().lines().count(), 2);
        fs::write(logger.path.with_extension("jsonl.1"), concat!(
            "{\"timestampMs\":1,\"level\":\"warn\",\"event\":\"legacy\"}\n",
            "{\"timestampMs\":2,\"level\":\"error\",\"event\":\"legacy\",\"fields\":{\"threadId\":\"old-thread\"}}\n",
            "invalid partial line\n"
        )).unwrap();
        let result = logger.read_logs(query()).unwrap();
        assert_eq!(result.total, 3);
        assert_eq!(result.records[0].thread_id.as_deref(), Some("old-thread"));
        assert_eq!(result.records[1].fields["apiKey"], "[REDACTED]");
        let wire = serde_json::to_value(&result).unwrap();
        assert_eq!(wire["records"][0]["timestampMs"], 2);
        assert!(wire["records"][0].get("timestamp_ms").is_none());
        assert_eq!(wire["records"][0]["threadId"], "old-thread");
        let filtered = logger
            .read_logs(LogQuery {
                level: Some("ERROR".into()),
                limit: Some(1),
                ..query()
            })
            .unwrap();
        assert_eq!(filtered.total, 2);
        assert_eq!(filtered.records.len(), 1);
        assert_eq!(
            logger
                .read_logs(LogQuery {
                    level: Some("warn".into()),
                    ..query()
                })
                .unwrap()
                .total,
            0
        );
    }

    #[test]
    fn clear_requires_confirmation_removes_rotations_and_preserves_other_data() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        for path in logger.paths() {
            fs::write(path, "old logs").unwrap();
        }
        let unrelated = data.path().join("logs/other.jsonl");
        fs::write(&unrelated, "keep").unwrap();
        let history = data.path().join("conversation.jsonl");
        fs::write(&history, "history").unwrap();
        assert_eq!(
            logger.clear_logs(false).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(fs::read_to_string(&logger.path).unwrap(), "old logs");
        logger.clear_logs(true).unwrap();
        let result = logger.read_logs(query()).unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.records[0].event, "logs_cleared");
        assert_eq!(result.records[0].fields["source"], "user");
        assert!(logger.paths().iter().skip(1).all(|path| !path.exists()));
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "keep");
        assert_eq!(fs::read_to_string(history).unwrap(), "history");
        logger
            .clone()
            .log("error", "after_clear", json!({}))
            .unwrap();
        assert_eq!(logger.read_logs(query()).unwrap().total, 2);
    }

    #[test]
    fn concurrent_clear_and_writes_leave_complete_records() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..3 {
                let logger = logger.clone();
                scope.spawn(move || {
                    for _ in 0..30 {
                        logger.log("info", "concurrent", json!({})).unwrap();
                    }
                });
            }
            for _ in 0..10 {
                logger.clear_logs(true).unwrap();
            }
        });
        let lines = fs::read_to_string(&logger.path).unwrap();
        assert!(
            lines
                .lines()
                .all(|line| serde_json::from_str::<Value>(line).is_ok())
        );
        assert!(
            logger
                .read_logs(query())
                .unwrap()
                .records
                .iter()
                .any(|r| r.event == "logs_cleared")
        );
    }

    #[test]
    fn refuses_linked_log_directory_before_read_write_or_clear() {
        let data = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let directory = data.path().join("logs");
        fs::remove_dir(&directory).unwrap();
        fs::write(outside.path().join("runtime.jsonl"), "untouched").unwrap();
        #[cfg(windows)]
        assert!(
            std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&directory)
                .arg(outside.path())
                .output()
                .unwrap()
                .status
                .success()
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &directory).unwrap();
        assert!(logger.read_logs(query()).is_err());
        assert!(logger.log("info", "escape", json!({})).is_err());
        assert!(logger.clear_logs(true).is_err());
        assert!(StructuredLogger::new(data.path()).is_err());
        assert_eq!(
            fs::read_to_string(outside.path().join("runtime.jsonl")).unwrap(),
            "untouched"
        );
        #[cfg(windows)]
        fs::remove_dir(directory).unwrap();
        #[cfg(unix)]
        fs::remove_file(directory).unwrap();
    }

    #[test]
    fn invalid_rotation_blocks_clear_before_any_log_is_removed() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        logger.log("error", "keep", json!({})).unwrap();
        fs::create_dir(logger.path.with_extension("jsonl.2")).unwrap();
        assert!(logger.clear_logs(true).is_err());
        assert!(fs::read_to_string(&logger.path).unwrap().contains("keep"));
    }
    #[test]
    fn redacts_nested_secrets() {
        let value = redact(json!({"apiKey":"abc", "nested":{"accessToken":"def", "ok":1}}));
        assert_eq!(value["apiKey"], "[REDACTED]");
        assert_eq!(value["nested"]["accessToken"], "[REDACTED]");
    }
}
