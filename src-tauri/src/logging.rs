use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const LOG_GENERATIONS: usize = 3;

/// 本地运行日志按「主要信息」收敛：日志面板一行要能读完，所以单个字符串字段、数组与
/// 嵌套对象的广度、以及整条 fields 的序列化长度都有界，超长正文以省略号结尾，完整内容
/// 以会话事实事件为准。调用标识只在会话里追得回来，不写进运行日志。
pub(crate) const MAX_FIELD_CHARS: usize = 160;
/// 失败原因与失败指令本体要同屏读完，因此整条 fields 的预算与前端面板上限
/// （`src/components/LogViewerDialog.tsx` 的 480）对齐：后台再松一档只会让面板重新
/// 截断，紧一档又会把其中一半挤出可见区域。
pub(crate) const MAX_FIELDS_CHARS: usize = 480;
const MAX_ARRAY_ITEMS: usize = 8;
const TRUNCATION_MARK: &str = "…";
/// 只有会话事实事件才追得回来的纯标识，运行日志不记录。
const TRACE_ONLY_KEYS: &[&str] = &["callid", "turnid", "itemid", "requestid", "jobid", "spanid"];
/// 预算超支时优先保留的「发生了什么」字段，其余字段按序排在其后取舍。
/// `arguments` 与 `output` 同列：事后查因必须同时看到「哪条指令失败」和「失败原因」，
/// 只留一半仍然无法定位。两者都在前端 480 字符面板预算内可读。
const PRIMARY_DETAIL_KEYS: &[&str] = &[
    "message",
    "reason",
    "output",
    "arguments",
    "error",
    "detail",
    "code",
];
/// 来源对话标记：`read_logs` 据此关联对话名称，任何预算下都不丢弃。
const SOURCE_KEYS: &[&str] = &["threadid"];

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
            "event": event, "fields": compact(redact(fields)) });
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
                // 旧记录按写入时的原样落盘，读取端用同一策略收敛，面板才不会只看到半行 JSON。
                let fields = compact(value.get("fields").cloned().unwrap_or(Value::Null));
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
                    thread_id: fields
                        .pointer("/threadId")
                        .and_then(Value::as_str)
                        .filter(|id| !id.trim().is_empty())
                        .map(str::to_owned),
                    thread_title: None,
                    fields,
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

/// 把一条记录的 fields 收敛成日志面板读得完的主要信息。写入与读取共用同一策略，因此
/// 旧记录和新记录在面板里是一致的有界形态。
fn compact(value: Value) -> Value {
    match value {
        Value::Object(map) => bound_total(compact_object(map)),
        Value::Array(items) => {
            let total = items.len();
            let mut kept: Vec<Value> = items
                .into_iter()
                .take(MAX_ARRAY_ITEMS)
                .map(compact)
                .collect();
            if total > MAX_ARRAY_ITEMS {
                kept.push(Value::String(TRUNCATION_MARK.into()));
            }
            Value::Array(kept)
        }
        Value::String(text) => Value::String(truncate_chars(&text, MAX_FIELD_CHARS)),
        other => other,
    }
}

fn compact_object(map: serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    map.into_iter()
        .filter(|(key, _)| !is_trace_only(key))
        .map(|(key, value)| (key, compact(value)))
        .collect()
}

/// 按优先级保留字段，直到整条 fields 的序列化长度落回预算内。排序稳定，因此丢弃顺序
/// 是确定性的；只剩单个超大字段时也要留痕，而不是把记录收敛成空对象。
fn bound_total(map: serde_json::Map<String, Value>) -> Value {
    let mut ordered: Vec<(String, Value)> = map.into_iter().collect();
    ordered.sort_by_key(|(key, _)| field_priority(key));
    let fallback = ordered.first().map(|(key, _)| key.clone());
    let mut kept = serde_json::Map::new();
    for (key, value) in ordered {
        kept.insert(key.clone(), value);
        if serialized_chars(&kept) > MAX_FIELDS_CHARS {
            kept.remove(&key);
        }
    }
    if let Some(key) = fallback.filter(|_| kept.is_empty()) {
        kept.insert(key, Value::String(TRUNCATION_MARK.into()));
    }
    Value::Object(kept)
}

fn field_priority(key: &str) -> usize {
    let lower = key.to_ascii_lowercase();
    if SOURCE_KEYS.contains(&lower.as_str()) {
        return 0;
    }
    PRIMARY_DETAIL_KEYS
        .iter()
        .position(|candidate| *candidate == lower)
        .map(|index| index + 1)
        .unwrap_or(PRIMARY_DETAIL_KEYS.len() + 1)
}

fn is_trace_only(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    TRACE_ONLY_KEYS.contains(&lower.as_str())
}

fn serialized_chars(map: &serde_json::Map<String, Value>) -> usize {
    serde_json::to_string(map)
        .map(|text| text.chars().count())
        .unwrap_or(usize::MAX)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut shortened: String = text.chars().take(max).collect();
    shortened.push_str(TRUNCATION_MARK);
    shortened
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

    #[test]
    fn writes_keep_only_the_main_information_the_panel_can_read() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let output = format!(
            "patch conflicts with the workspace: chunk for src/App.css matched 0 locations instead of exactly one\n{}",
            "冲突细节".repeat(400)
        );
        logger
            .log(
                "error",
                "tool_failed",
                json!({
                    "threadId": "4eff95a7-9acc-4357-8564-b381bc442c7e",
                    "turnId": "ac8f71a1-3650-4093-beab-dfbd7eb38cb8",
                    "tool": "apply_patch",
                    "callId": "0dbff861-61c4-59c0-4836-8f12-bb0ea0ea83f9",
                    "itemStatus": "failed",
                    "output": output,
                }),
            )
            .unwrap();
        let persisted = fs::read_to_string(&logger.path).unwrap();
        let record: Value = serde_json::from_str(persisted.lines().next().unwrap()).unwrap();
        let fields = record["fields"].as_object().unwrap();
        // 调用与 Turn 标识只在会话事实事件里追得回来，不占运行日志的版面。
        assert!(fields.get("callId").is_none());
        assert!(fields.get("turnId").is_none());
        // 来源对话、工具与失败原因保留，且整条 fields 一行能读完。
        assert_eq!(
            record["fields"]["threadId"],
            "4eff95a7-9acc-4357-8564-b381bc442c7e"
        );
        assert_eq!(record["fields"]["tool"], "apply_patch");
        assert_eq!(record["fields"]["itemStatus"], "failed");
        let output = record["fields"]["output"].as_str().unwrap();
        assert!(output.starts_with("patch conflicts with the workspace"));
        assert!(output.ends_with(TRUNCATION_MARK));
        assert!(output.chars().count() <= MAX_FIELD_CHARS + 1);
        assert!(serialized_chars(fields) <= MAX_FIELDS_CHARS);
    }

    #[test]
    fn failed_command_arguments_survive_next_to_the_failure_reason() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let command = "cd D:/code/Nick/k-coder; npx playwright test e2e/workbench.spec.ts -g \"commit button is disabled\"";
        logger
            .log(
                "error",
                "tool_failed",
                json!({
                    "threadId": "4eff95a7-9acc-4357-8564-b381bc442c7e",
                    "turnId": "ac8f71a1-3650-4093-beab-dfbd7eb38cb8",
                    "tool": "run_command",
                    "callId": "0dbff861-61c4-59c0-4836-8f12-bb0ea0ea83f9",
                    "itemStatus": "failed",
                    "arguments": {"command": command},
                    "output": "运行失败：退出码 1 (已有部分输出)",
                }),
            )
            .unwrap();
        let persisted = fs::read_to_string(&logger.path).unwrap();
        let record: Value = serde_json::from_str(persisted.lines().next().unwrap()).unwrap();
        let fields = record["fields"].as_object().unwrap();
        // 事后查因必须在同一行里同时读到「哪条指令失败」和「失败原因」。
        assert_eq!(fields["arguments"]["command"], command);
        assert_eq!(fields["output"], "运行失败：退出码 1 (已有部分输出)");
        assert_eq!(record["fields"]["tool"], "run_command");
        assert_eq!(
            record["fields"]["threadId"],
            "4eff95a7-9acc-4357-8564-b381bc442c7e"
        );
        assert!(fields.get("callId").is_none());
        assert!(fields.get("turnId").is_none());
        assert!(serialized_chars(fields) <= MAX_FIELDS_CHARS);
    }
    #[test]
    fn reads_compact_legacy_records_so_the_reason_stays_visible() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let legacy = json!({
            "timestampMs": 7,
            "level": "error",
            "event": "tool_failed",
            "fields": {
                "threadId": "old-thread",
                "turnId": "old-turn",
                "callId": "old-call",
                "output": "x".repeat(4096),
            },
        });
        fs::write(&logger.path, format!("{legacy}\n")).unwrap();
        let result = logger.read_logs(query()).unwrap();
        let record = &result.records[0];
        // 来源对话的关联不能因为收敛而失效。
        assert_eq!(record.thread_id.as_deref(), Some("old-thread"));
        assert!(record.fields.get("turnId").is_none());
        assert!(record.fields.get("callId").is_none());
        let output = record.fields["output"].as_str().unwrap();
        assert!(output.starts_with("xxx"));
        assert!(output.ends_with(TRUNCATION_MARK));
        assert!(serialized_chars(record.fields.as_object().unwrap()) <= MAX_FIELDS_CHARS);
    }

    #[test]
    fn secondary_fields_are_dropped_until_the_record_fits_the_panel() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        logger
            .log(
                "error",
                "turn_finished",
                json!({
                    "threadId": "4eff95a7-9acc-4357-8564-b381bc442c7e",
                    "message": "provider request failed: error sending request for url (https://api.stepfun.com/step_plan/v1/chat/completions) (已自动重试 3 次)",
                    "error": "e".repeat(400),
                    "detail": "d".repeat(400),
                    "success": false,
                }),
            )
            .unwrap();
        let result = logger.read_logs(query()).unwrap();
        let fields = result.records[0].fields.as_object().unwrap();
        // 先说发生了什么，再说背景：主字段（message/error）在 480 预算内留得下，
        // 更低优先级的长字段（detail）整条丢弃，而不是留半行让人猜。
        assert_eq!(fields["threadId"], "4eff95a7-9acc-4357-8564-b381bc442c7e");
        assert!(
            fields["message"]
                .as_str()
                .unwrap()
                .starts_with("provider request failed")
        );
        assert!(fields["error"].as_str().unwrap().ends_with(TRUNCATION_MARK));
        assert!(fields.get("detail").is_none());
        assert_eq!(fields["success"], false);
        assert!(serialized_chars(fields) <= MAX_FIELDS_CHARS);
    }

    #[test]
    fn nested_arrays_and_objects_stay_bounded() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        logger
            .log(
                "info",
                "batch_reported",
                json!({
                    "threadId": "thread-1",
                    "items": ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"],
                    "nested": {
                        "alpha": "a".repeat(400),
                        "beta": "b".repeat(400),
                        "gamma": "g".repeat(400),
                    },
                }),
            )
            .unwrap();
        let result = logger.read_logs(query()).unwrap();
        let fields = result.records[0].fields.as_object().unwrap();
        let items = fields["items"].as_array().unwrap();
        assert_eq!(items.len(), MAX_ARRAY_ITEMS + 1);
        assert_eq!(items[MAX_ARRAY_ITEMS], TRUNCATION_MARK);
        // 嵌套对象同样受总预算约束：预算从 320 放宽到 480 后，前两个长字段留得下，
        // 第三个仍然被丢弃，记录始终不超预算。
        let nested = fields["nested"].as_object().unwrap();
        assert_eq!(nested.len(), 2);
        assert!(nested["alpha"].as_str().unwrap().ends_with(TRUNCATION_MARK));
        assert!(nested["beta"].as_str().unwrap().ends_with(TRUNCATION_MARK));
        assert!(nested.get("gamma").is_none());
        assert!(serialized_chars(fields) <= MAX_FIELDS_CHARS);
    }

    #[test]
    fn compaction_is_idempotent_and_keeps_secret_redaction_intact() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        logger
            .log(
                "error",
                "turn_failed",
                json!({
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "apiKey": "sk-should-never-survive",
                    "message": "m".repeat(400),
                }),
            )
            .unwrap();
        let first = logger.read_logs(query()).unwrap();
        let once = first.records[0].fields.clone();
        fs::write(
            &logger.path,
            format!(
                "{}\n",
                serde_json::to_string(&json!({
                    "timestampMs": 1,
                    "level": "error",
                    "event": "turn_failed",
                    "fields": once,
                }))
                .unwrap()
            ),
        )
        .unwrap();
        let twice = logger.read_logs(query()).unwrap();
        // 再收敛一次不改变形态：面板每次刷新看到的是同一条有界记录。
        assert_eq!(once, twice.records[0].fields);
        assert_eq!(twice.records[0].fields["apiKey"], "[REDACTED]");
        assert!(twice.records[0].fields.get("turnId").is_none());
    }
}
