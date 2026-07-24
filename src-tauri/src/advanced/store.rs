use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

pub fn append_json_line<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if path
        .metadata()
        .is_ok_and(|metadata| metadata.len() >= MAX_LOG_BYTES)
    {
        return Err("advanced runtime log reached its 10 MiB limit".into());
    }
    let parent = path.parent().ok_or("advanced runtime log has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_data().map_err(|error| error.to_string())
}

pub fn read_json_lines<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    if bytes.len() as u64 > MAX_LOG_BYTES {
        return Err("advanced runtime log exceeds its 10 MiB limit".into());
    }
    let mut values = Vec::new();
    let lines = bytes.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(value) => values.push(value),
            Err(_) if index == lines.len() - 1 && !bytes.ends_with(b"\n") => break,
            Err(error) => return Err(format!("invalid advanced runtime log record: {error}")),
        }
    }
    Ok(values)
}
