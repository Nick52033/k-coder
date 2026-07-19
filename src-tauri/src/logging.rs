use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
