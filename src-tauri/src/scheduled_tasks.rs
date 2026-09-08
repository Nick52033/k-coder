//! Persistent, local scheduled tasks.
//!
//! The scheduler owns only the trigger and lifecycle state.  Actual work is
//! handed back to the existing command/AgentRuntime path so scheduled runs
//! keep the same workspace, approval, cancellation and audit boundaries as a
//! user-started turn.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Datelike, Duration as ChronoDuration, Local, LocalResult, TimeZone};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio::time::sleep;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::storage::now_ms;

const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;
const MAX_NAME_CHARS: usize = 100;
const MAX_PROMPT_CHARS: usize = 16_000;
const MAX_SCHEDULED_TASKS: usize = 256;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledTaskKind {
    Once,
    Daily,
    Weekly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTaskSchedule {
    pub kind: ScheduledTaskKind,
    #[serde(default)]
    pub at_ms: Option<u64>,
    #[serde(default)]
    pub hour: Option<u8>,
    #[serde(default)]
    pub minute: Option<u8>,
    #[serde(default)]
    pub weekday: Option<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledTaskMode {
    Background,
    Thread,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTaskView {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub schedule: ScheduledTaskSchedule,
    pub prompt: String,
    pub mode: ScheduledTaskMode,
    pub thread_id: Option<String>,
    pub workspace_path: String,
    pub enabled: bool,
    pub next_run_at_ms: Option<u64>,
    pub last_run_at_ms: Option<u64>,
    pub last_run_state: Option<String>,
    pub last_error: Option<String>,
    pub run_count: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub revision: u64,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertScheduledTaskRequest {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub schedule: ScheduledTaskSchedule,
    pub prompt: String,
    pub mode: ScheduledTaskMode,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, thiserror::Error)]
pub enum ScheduledTaskError {
    #[error("scheduled task storage failed: {0}")]
    Storage(String),
    #[error("scheduled task is invalid: {0}")]
    Invalid(String),
    #[error("scheduled task was not found")]
    NotFound,
}

#[derive(Clone)]
pub struct ScheduledTaskStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
    in_flight: Arc<Mutex<HashSet<String>>>,
}

impl ScheduledTaskStore {
    pub fn new(data_root: &Path) -> Result<Self, ScheduledTaskError> {
        fs::create_dir_all(data_root)
            .map_err(|error| ScheduledTaskError::Storage(error.to_string()))?;
        Ok(Self {
            path: data_root.join("scheduled-tasks.jsonl"),
            lock: Arc::new(Mutex::new(())),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    fn latest_unlocked(&self) -> Result<Vec<ScheduledTaskView>, ScheduledTaskError> {
        let values = read_json_lines(&self.path)?;
        let mut latest = HashMap::<String, ScheduledTaskView>::new();
        for task in values {
            if latest
                .get(&task.id)
                .is_none_or(|current| current.revision < task.revision)
            {
                latest.insert(task.id.clone(), task);
            }
        }
        let mut tasks = latest
            .into_values()
            .filter(|task| !task.deleted)
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| {
            left.next_run_at_ms
                .unwrap_or(u64::MAX)
                .cmp(&right.next_run_at_ms.unwrap_or(u64::MAX))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(tasks)
    }

    pub fn list(&self) -> Result<Vec<ScheduledTaskView>, ScheduledTaskError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ScheduledTaskError::Storage("scheduled task lock poisoned".into()))?;
        self.latest_unlocked()
    }

    pub fn upsert(
        &self,
        request: UpsertScheduledTaskRequest,
        current_workspace: &Path,
    ) -> Result<ScheduledTaskView, ScheduledTaskError> {
        let name = request.name.trim();
        let prompt = request.prompt.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
            return Err(ScheduledTaskError::Invalid(format!(
                "name must contain 1 to {MAX_NAME_CHARS} characters"
            )));
        }
        if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
            return Err(ScheduledTaskError::Invalid(format!(
                "prompt must contain 1 to {MAX_PROMPT_CHARS} characters"
            )));
        }
        validate_schedule(&request.schedule, now_ms())?;
        if matches!(request.mode, ScheduledTaskMode::Thread)
            && request
                .thread_id
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(ScheduledTaskError::Invalid(
                "thread mode requires a conversation".into(),
            ));
        }
        let workspace_input = request
            .workspace_path
            .clone()
            .unwrap_or_else(|| current_workspace.to_string_lossy().into_owned());
        let workspace = workspace_input.trim();
        if workspace.is_empty() {
            return Err(ScheduledTaskError::Invalid(
                "a workspace is required for scheduled tasks".into(),
            ));
        }
        let workspace = Path::new(workspace).canonicalize().map_err(|error| {
            ScheduledTaskError::Invalid(format!("workspace is unavailable: {error}"))
        })?;
        if !workspace.is_dir() {
            return Err(ScheduledTaskError::Invalid(
                "workspace must be a directory".into(),
            ));
        }

        let _guard = self
            .lock
            .lock()
            .map_err(|_| ScheduledTaskError::Storage("scheduled task lock poisoned".into()))?;
        let mut current = if let Some(id) = request
            .id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            self.latest_unlocked()?
                .into_iter()
                .find(|task| task.id == id)
                .ok_or(ScheduledTaskError::NotFound)?
        } else {
            if self.latest_unlocked()?.len() >= MAX_SCHEDULED_TASKS {
                return Err(ScheduledTaskError::Invalid(format!(
                    "at most {MAX_SCHEDULED_TASKS} scheduled tasks are supported"
                )));
            }
            let now = now_ms();
            ScheduledTaskView {
                schema_version: crate::protocol::PROTOCOL_VERSION,
                id: Uuid::new_v4().to_string(),
                name: name.into(),
                schedule: request.schedule.clone(),
                prompt: prompt.into(),
                mode: request.mode,
                thread_id: normalize_optional(request.thread_id.clone()),
                workspace_path: workspace.to_string_lossy().into_owned(),
                enabled: request.enabled,
                next_run_at_ms: None,
                last_run_at_ms: None,
                last_run_state: None,
                last_error: None,
                run_count: 0,
                created_at_ms: now,
                updated_at_ms: now,
                revision: 1,
                deleted: false,
            }
        };
        current.name = name.into();
        current.schedule = request.schedule;
        current.prompt = prompt.into();
        current.mode = request.mode;
        current.thread_id = normalize_optional(request.thread_id);
        current.workspace_path = workspace.to_string_lossy().into_owned();
        current.enabled = request.enabled;
        current.next_run_at_ms = if current.enabled {
            next_occurrence(&current.schedule, now_ms())
        } else {
            None
        };
        current.updated_at_ms = now_ms();
        current.revision = current.revision.saturating_add(1);
        current.deleted = false;
        append_json_line(&self.path, &current)?;
        Ok(current)
    }

    pub fn delete(&self, id: &str) -> Result<(), ScheduledTaskError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ScheduledTaskError::Storage("scheduled task lock poisoned".into()))?;
        let mut task = self
            .latest_unlocked()?
            .into_iter()
            .find(|task| task.id == id.trim())
            .ok_or(ScheduledTaskError::NotFound)?;
        task.deleted = true;
        task.enabled = false;
        task.next_run_at_ms = None;
        task.updated_at_ms = now_ms();
        task.revision = task.revision.saturating_add(1);
        append_json_line(&self.path, &task)?;
        self.in_flight
            .lock()
            .map_err(|_| ScheduledTaskError::Storage("scheduled task lock poisoned".into()))?
            .remove(id);
        Ok(())
    }

    pub fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<ScheduledTaskView, ScheduledTaskError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ScheduledTaskError::Storage("scheduled task lock poisoned".into()))?;
        let mut task = self
            .latest_unlocked()?
            .into_iter()
            .find(|task| task.id == id.trim())
            .ok_or(ScheduledTaskError::NotFound)?;
        let next_run_at_ms = if enabled {
            next_occurrence(&task.schedule, now_ms()).ok_or_else(|| {
                ScheduledTaskError::Invalid(
                    "this task has no future occurrence; edit its schedule before enabling it"
                        .into(),
                )
            })?
        } else {
            0
        };
        task.enabled = enabled;
        task.next_run_at_ms = enabled.then_some(next_run_at_ms);
        task.updated_at_ms = now_ms();
        task.revision = task.revision.saturating_add(1);
        append_json_line(&self.path, &task)?;
        Ok(task)
    }

    pub fn trigger_now(&self, id: &str) -> Result<ScheduledTaskView, ScheduledTaskError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ScheduledTaskError::Storage("scheduled task lock poisoned".into()))?;
        let mut task = self
            .latest_unlocked()?
            .into_iter()
            .find(|task| task.id == id.trim())
            .ok_or(ScheduledTaskError::NotFound)?;
        task.enabled = true;
        task.next_run_at_ms = Some(now_ms());
        task.last_error = None;
        task.updated_at_ms = now_ms();
        task.revision = task.revision.saturating_add(1);
        append_json_line(&self.path, &task)?;
        Ok(task)
    }

    pub fn claim_due(&self, now: u64) -> Result<Vec<ScheduledTaskView>, ScheduledTaskError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ScheduledTaskError::Storage("scheduled task lock poisoned".into()))?;
        let tasks = self.latest_unlocked()?;
        let mut in_flight = self
            .in_flight
            .lock()
            .map_err(|_| ScheduledTaskError::Storage("scheduled task lock poisoned".into()))?;
        let mut claimed = Vec::new();
        for mut task in tasks {
            let recovering_running_task = task.last_run_state.as_deref() == Some("running");
            let due = task.next_run_at_ms.is_some_and(|next| next <= now);
            if !task.enabled || (!due && !recovering_running_task) || in_flight.contains(&task.id) {
                continue;
            }
            in_flight.insert(task.id.clone());
            task.last_run_at_ms = Some(now);
            task.last_run_state = Some("running".into());
            task.last_error = None;
            task.updated_at_ms = now;
            task.revision = task.revision.saturating_add(1);
            append_json_line(&self.path, &task)?;
            claimed.push(task);
        }
        Ok(claimed)
    }

    pub fn finish_run(
        &self,
        task_id: &str,
        success: bool,
        error: Option<String>,
    ) -> Result<ScheduledTaskView, ScheduledTaskError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| ScheduledTaskError::Storage("scheduled task lock poisoned".into()))?;
        let mut task = self
            .latest_unlocked()?
            .into_iter()
            .find(|task| task.id == task_id)
            .ok_or(ScheduledTaskError::NotFound)?;
        let now = now_ms();
        task.last_run_state = Some(if success { "completed" } else { "failed" }.into());
        task.last_error = error.map(|value| value.chars().take(2_000).collect());
        task.run_count = task.run_count.saturating_add(1);
        task.updated_at_ms = now;
        if task.enabled {
            task.next_run_at_ms = next_occurrence(&task.schedule, now);
            if matches!(task.schedule.kind, ScheduledTaskKind::Once) {
                task.enabled = false;
            }
        }
        task.revision = task.revision.saturating_add(1);
        append_json_line(&self.path, &task)?;
        self.in_flight
            .lock()
            .map_err(|_| ScheduledTaskError::Storage("scheduled task lock poisoned".into()))?
            .remove(task_id);
        Ok(task)
    }
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn validate_schedule(
    schedule: &ScheduledTaskSchedule,
    now: u64,
) -> Result<(), ScheduledTaskError> {
    match schedule.kind {
        ScheduledTaskKind::Once => {
            let at = schedule.at_ms.ok_or_else(|| {
                ScheduledTaskError::Invalid("one-time tasks require a date and time".into())
            })?;
            if at <= now {
                return Err(ScheduledTaskError::Invalid(
                    "one-time task must be scheduled in the future".into(),
                ));
            }
        }
        ScheduledTaskKind::Daily => validate_clock(schedule)?,
        ScheduledTaskKind::Weekly => {
            validate_clock(schedule)?;
            if schedule.weekday.is_none_or(|weekday| weekday > 6) {
                return Err(ScheduledTaskError::Invalid(
                    "weekday must be between 0 (Sunday) and 6 (Saturday)".into(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_clock(schedule: &ScheduledTaskSchedule) -> Result<(), ScheduledTaskError> {
    if schedule.hour.is_none_or(|hour| hour > 23)
        || schedule.minute.is_none_or(|minute| minute > 59)
    {
        return Err(ScheduledTaskError::Invalid(
            "time must use a valid 24-hour clock value".into(),
        ));
    }
    Ok(())
}

pub fn next_occurrence(schedule: &ScheduledTaskSchedule, after_ms: u64) -> Option<u64> {
    match schedule.kind {
        ScheduledTaskKind::Once => schedule.at_ms.filter(|value| *value > after_ms),
        ScheduledTaskKind::Daily | ScheduledTaskKind::Weekly => {
            let hour = schedule.hour? as u32;
            let minute = schedule.minute? as u32;
            let after = Local.timestamp_millis_opt(after_ms as i64).single()?;
            for offset in 0..=7 {
                let date = after.date_naive() + ChronoDuration::days(offset);
                if matches!(schedule.kind, ScheduledTaskKind::Weekly)
                    && date.weekday().num_days_from_sunday() != schedule.weekday? as u32
                {
                    continue;
                }
                let candidate = local_at(date, hour, minute)?;
                let candidate_ms = candidate.timestamp_millis();
                if candidate_ms > after_ms as i64 {
                    return u64::try_from(candidate_ms).ok();
                }
            }
            None
        }
    }
}

fn local_at(date: chrono::NaiveDate, hour: u32, minute: u32) -> Option<chrono::DateTime<Local>> {
    match Local.with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0) {
        LocalResult::Single(value) | LocalResult::Ambiguous(value, _) => Some(value),
        LocalResult::None => None,
    }
}

fn append_json_line<T: Serialize>(path: &Path, value: &T) -> Result<(), ScheduledTaskError> {
    if path
        .metadata()
        .is_ok_and(|metadata| metadata.len() >= MAX_LOG_BYTES)
    {
        return Err(ScheduledTaskError::Storage(
            "scheduled task log reached its 10 MiB limit".into(),
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ScheduledTaskError::Storage("scheduled task log has no parent".into()))?;
    fs::create_dir_all(parent).map_err(|error| ScheduledTaskError::Storage(error.to_string()))?;
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| ScheduledTaskError::Storage(error.to_string()))?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| ScheduledTaskError::Storage(error.to_string()))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_data())
        .map_err(|error| ScheduledTaskError::Storage(error.to_string()))
}

fn read_json_lines(path: &Path) -> Result<Vec<ScheduledTaskView>, ScheduledTaskError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ScheduledTaskError::Storage(error.to_string())),
    };
    if bytes.len() as u64 > MAX_LOG_BYTES {
        return Err(ScheduledTaskError::Storage(
            "scheduled task log exceeds its 10 MiB limit".into(),
        ));
    }
    let mut values = Vec::new();
    let lines = bytes.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(value) => values.push(value),
            Err(_) if index == lines.len() - 1 && !bytes.ends_with(&[b'\n']) => break,
            Err(error) => {
                return Err(ScheduledTaskError::Storage(format!(
                    "invalid scheduled task record: {error}"
                )));
            }
        }
    }
    Ok(values)
}

/// Keep the scheduler alive with the app window hidden or visible.  Polling is
/// intentionally bounded; persistence remains the source of truth across
/// restarts, so a missed tick is picked up on the next iteration.
pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let service = app.state::<AppState>().scheduled_tasks();
            match service.claim_due(now_ms()) {
                Ok(tasks) => {
                    for task in tasks {
                        let task_id = task.id.clone();
                        let task_service = service.clone();
                        let task_app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let result =
                                crate::commands::execute_scheduled_task(task_app, task).await;
                            let (success, error) = match result {
                                Ok(_) => (true, None),
                                Err(error) => (false, Some(error)),
                            };
                            let _ = task_service.finish_run(&task_id, success, error);
                        });
                    }
                }
                Err(error) => {
                    eprintln!("scheduled task poll failed: {error}");
                }
            }
            sleep(Duration::from_secs(5)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn daily(hour: u8, minute: u8) -> ScheduledTaskSchedule {
        ScheduledTaskSchedule {
            kind: ScheduledTaskKind::Daily,
            at_ms: None,
            hour: Some(hour),
            minute: Some(minute),
            weekday: None,
        }
    }

    #[test]
    fn validates_schedule_shape_and_future_one_time_runs() {
        let now = now_ms();
        let invalid = validate_schedule(&daily(24, 0), now);
        assert!(invalid.is_err());
        let once = ScheduledTaskSchedule {
            kind: ScheduledTaskKind::Once,
            at_ms: Some(now + 10_000),
            hour: None,
            minute: None,
            weekday: None,
        };
        assert_eq!(next_occurrence(&once, now), Some(now + 10_000));
        assert!(validate_schedule(&once, now).is_ok());
    }

    #[test]
    fn daily_occurrence_moves_to_tomorrow_after_the_configured_time() {
        let now = Local::now();
        let before = local_at(
            now.date_naive(),
            now.hour(),
            now.minute().saturating_add(1).min(59),
        )
        .unwrap()
        .timestamp_millis() as u64;
        let after = next_occurrence(&daily(now.hour() as u8, now.minute() as u8), before).unwrap();
        assert!(after > before);
    }

    #[test]
    fn store_persists_updates_and_one_time_completion_disables_task() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let task = store
            .upsert(
                UpsertScheduledTaskRequest {
                    id: None,
                    name: "Nightly check".into(),
                    schedule: ScheduledTaskSchedule {
                        kind: ScheduledTaskKind::Once,
                        at_ms: Some(now_ms() + 60_000),
                        hour: None,
                        minute: None,
                        weekday: None,
                    },
                    prompt: "Check the repository".into(),
                    mode: ScheduledTaskMode::Background,
                    thread_id: None,
                    workspace_path: Some(dir.path().to_string_lossy().into_owned()),
                    enabled: true,
                },
                dir.path(),
            )
            .unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        let claimed = store.claim_due(now_ms() + 60_000).unwrap();
        assert_eq!(claimed.len(), 1);
        let completed = store.finish_run(&task.id, true, None).unwrap();
        assert!(!completed.enabled);
        assert_eq!(completed.last_run_state.as_deref(), Some("completed"));
    }

    #[test]
    fn a_restarted_store_reclaims_a_running_task_from_persisted_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let task = store
            .upsert(
                UpsertScheduledTaskRequest {
                    id: None,
                    name: "Recoverable task".into(),
                    schedule: daily(9, 0),
                    prompt: "Resume the interrupted check".into(),
                    mode: ScheduledTaskMode::Background,
                    thread_id: None,
                    workspace_path: Some(dir.path().to_string_lossy().into_owned()),
                    enabled: true,
                },
                dir.path(),
            )
            .unwrap();

        let first_claim = store.claim_due(now_ms() + 86_400_000).unwrap();
        assert_eq!(first_claim.len(), 1);
        assert_eq!(first_claim[0].id, task.id);

        let restarted_store = ScheduledTaskStore::new(dir.path()).unwrap();
        let recovered = restarted_store.claim_due(now_ms()).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, task.id);
        assert_eq!(recovered[0].last_run_state.as_deref(), Some("running"));
    }
}
