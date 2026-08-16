use std::collections::{HashMap, VecDeque};
use std::env;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod shell;

const DEFAULT_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const MAX_TIMEOUT_MS: u64 = 60 * 60 * 1000;
const OUTPUT_DRAIN_GRACE_MS: u64 = 500;
const COMMAND_WAIT_STATUS_POLL_MS: u64 = 100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandMode {
    Foreground,
    Background,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCommandRequest {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub mode: CommandMode,
    pub timeout_ms: Option<u64>,
    pub buffer_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CommandState {
    Running,
    Exited { code: i32 },
    TimedOut,
    Cancelled,
    Failed { message: String },
}

impl CommandState {
    pub fn finished(&self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSessionView {
    pub id: String,
    pub mode: CommandMode,
    pub state: CommandState,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub next_cursor: u64,
    pub oldest_cursor: u64,
    pub output_truncated: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputChunk {
    pub cursor: u64,
    pub stream: OutputStream,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputPage {
    pub chunks: Vec<OutputChunk>,
    pub next_cursor: u64,
    pub oldest_cursor: u64,
    pub truncated_before_cursor: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandRisk {
    ReadOnly,
    BuildOrTest,
    Network,
    Write,
    Destructive,
    Privileged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandAssessment {
    pub risk: CommandRisk,
    pub requires_approval: bool,
    pub reason: String,
}

pub fn assess_command(program: &str, args: &[String]) -> CommandAssessment {
    let executable = Path::new(program)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    let lowered = args
        .iter()
        .map(|arg| arg.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let joined = lowered.join(" ");
    let destructive = [
        "rm",
        "rmdir",
        "del",
        "erase",
        "remove-item",
        "format",
        "diskpart",
    ];
    let shell_payload = matches!(
        executable.as_str(),
        "powershell" | "pwsh" | "cmd" | "sh" | "bash" | "zsh"
    ) && lowered
        .iter()
        .any(|arg| matches!(arg.as_str(), "-c" | "-command" | "/c"));
    if destructive.contains(&executable.as_str())
        || (executable == "git"
            && lowered
                .first()
                .is_some_and(|arg| matches!(arg.as_str(), "clean" | "reset")))
        || (shell_payload
            && ["remove-item", " rm ", " del ", "rmdir", "format "]
                .iter()
                .any(|token| joined.contains(token)))
    {
        return CommandAssessment { risk: CommandRisk::Destructive, requires_approval: true, reason: "command can delete files or storage; the runtime classified the executable and arguments as destructive".into() };
    }
    if matches!(executable.as_str(), "sudo" | "su" | "runas") {
        return CommandAssessment {
            risk: CommandRisk::Privileged,
            requires_approval: true,
            reason: "command requests elevated operating-system privileges".into(),
        };
    }
    if shell_payload {
        return CommandAssessment {
            risk: CommandRisk::Write,
            requires_approval: true,
            reason: "free-form shell payload can perform workspace or system changes".into(),
        };
    }
    if matches!(executable.as_str(), "curl" | "wget" | "ssh" | "scp") {
        return CommandAssessment {
            risk: CommandRisk::Network,
            requires_approval: true,
            reason: "command can communicate with an external system".into(),
        };
    }
    let package_manager_build_or_test = |command: Option<&String>| {
        command
            .is_some_and(|value| matches!(value.as_str(), "test" | "build" | "lint" | "typecheck"))
    };
    let build_or_test = match executable.as_str() {
        "cargo" => lowered.first().is_some_and(|arg| {
            matches!(arg.as_str(), "build" | "check" | "test" | "fmt" | "clippy")
        }),
        "npm" | "pnpm" | "yarn" => package_manager_build_or_test(lowered.first()),
        "corepack" => {
            lowered
                .first()
                .is_some_and(|value| matches!(value.as_str(), "npm" | "pnpm" | "yarn"))
                && package_manager_build_or_test(lowered.get(1))
        }
        "pytest" => true,
        "go" => lowered
            .first()
            .is_some_and(|arg| matches!(arg.as_str(), "build" | "test" | "vet")),
        "dotnet" => lowered
            .first()
            .is_some_and(|arg| matches!(arg.as_str(), "build" | "test")),
        "mvn" | "gradle" => lowered
            .iter()
            .any(|arg| matches!(arg.as_str(), "test" | "check" | "build")),
        _ => false,
    };
    if build_or_test {
        return CommandAssessment {
            risk: CommandRisk::BuildOrTest,
            requires_approval: false,
            reason: "recognized project build or test command".into(),
        };
    }
    let ripgrep_is_read_only = executable == "rg"
        && !lowered.iter().any(|arg| {
            matches!(arg.as_str(), "--pre" | "--pre-glob" | "--search-zip")
                || arg.starts_with("--pre=")
                || arg.starts_with("--pre-glob=")
        });
    if matches!(
        executable.as_str(),
        "ls" | "dir"
            | "pwd"
            | "where"
            | "which"
            | "cat"
            | "head"
            | "tail"
            | "echo"
            | "printf"
            | "get-childitem"
            | "get-command"
            | "get-content"
            | "get-item"
            | "get-location"
            | "get-process"
            | "measure-object"
            | "select-string"
            | "test-path"
            | "write-output"
    ) || ripgrep_is_read_only
        || (executable == "git"
            && lowered
                .first()
                .is_some_and(|arg| matches!(arg.as_str(), "status" | "diff" | "log" | "show")))
    {
        return CommandAssessment {
            risk: CommandRisk::ReadOnly,
            requires_approval: false,
            reason: "recognized read-only inspection command".into(),
        };
    }
    CommandAssessment { risk: CommandRisk::Write, requires_approval: true, reason: "unrecognized executable requires approval because its effects cannot be proven read-only".into() }
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("invalid command request: {0}")]
    Invalid(String),
    #[error("command session was not found: {0}")]
    NotFound(String),
    #[error("command session is already closed: {0}")]
    Closed(String),
    #[error("command runtime failed: {0}")]
    Io(String),
}

#[derive(Debug, Clone)]
pub(crate) struct BundledTools {
    directory: PathBuf,
}

impl BundledTools {
    pub(crate) fn new(directory: impl AsRef<Path>) -> Result<Self, ExecutionError> {
        let directory = directory.as_ref().canonicalize().map_err(|error| {
            ExecutionError::Invalid(format!("bundled tools directory is unavailable: {error}"))
        })?;
        if !directory.is_dir() {
            return Err(ExecutionError::Invalid(
                "bundled tools path is not a directory".into(),
            ));
        }
        let ripgrep = directory.join(if cfg!(windows) { "rg.exe" } else { "rg" });
        if !ripgrep.is_file() {
            return Err(ExecutionError::Invalid(format!(
                "bundled ripgrep executable is unavailable: {}",
                ripgrep.display()
            )));
        }
        Ok(Self {
            directory: strip_verbatim_prefix(&directory),
        })
    }

    fn path_for(&self, request_env: &HashMap<String, String>) -> OsString {
        let existing_path = request_env
            .iter()
            .find(|(key, _)| is_path_key(key))
            .map(|(_, value)| OsString::from(value))
            .or_else(|| env::var_os("PATH"));
        prepend_path_entry(&self.directory, existing_path)
    }
}

fn prepend_path_entry(path_entry: &Path, existing_path: Option<OsString>) -> OsString {
    #[cfg(unix)]
    const PATH_SEPARATOR: &str = ":";
    #[cfg(windows)]
    const PATH_SEPARATOR: &str = ";";

    let mut path = path_entry.as_os_str().to_os_string();
    if let Some(existing_path) = existing_path.filter(|value| !value.is_empty()) {
        path.push(PATH_SEPARATOR);
        path.push(existing_path);
    }
    path
}

fn is_path_key(key: &str) -> bool {
    #[cfg(windows)]
    {
        key.eq_ignore_ascii_case("PATH")
    }
    #[cfg(not(windows))]
    {
        key == "PATH"
    }
}

#[derive(Clone)]
pub struct CommandRuntime {
    workspace_root: PathBuf,
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
    grants: Arc<RwLock<Vec<ReusableAuthorization>>>,
    recovery_dir: Option<PathBuf>,
    default_shell: shell::DetectedShell,
    bundled_tools: Option<BundledTools>,
}

/// Host-owned reusable authorization. It is intentionally not deserializable, so a
/// model tool call cannot manufacture or widen an authorization scope.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReusableAuthorization {
    program: String,
    args_prefix: Vec<String>,
    cwd_prefix: String,
}

impl ReusableAuthorization {
    pub fn host_configured(
        program: impl Into<String>,
        args_prefix: Vec<String>,
        cwd_prefix: impl Into<String>,
    ) -> Self {
        Self {
            program: program.into(),
            args_prefix,
            cwd_prefix: cwd_prefix.into(),
        }
    }

    fn matches(&self, request: &StartCommandRequest) -> bool {
        request.program.eq_ignore_ascii_case(&self.program)
            && request.args.starts_with(&self.args_prefix)
            && (self.cwd_prefix.is_empty()
                || request.cwd == self.cwd_prefix
                || request.cwd.starts_with(&(self.cwd_prefix.clone() + "/")))
    }
}

struct Session {
    id: String,
    mode: CommandMode,
    started_at_ms: u64,
    state: Mutex<CommandState>,
    finished_at_ms: Mutex<Option<u64>>,
    stdin: Mutex<Option<ChildStdin>>,
    output: Mutex<OutputBuffer>,
    cancel: CancellationToken,
    changed: Notify,
}

struct OutputBuffer {
    chunks: VecDeque<OutputChunk>,
    bytes: usize,
    limit: usize,
    next_cursor: u64,
    truncated: bool,
}

impl CommandRuntime {
    #[cfg(test)]
    pub(crate) fn root(&self) -> PathBuf {
        self.workspace_root.clone()
    }

    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, ExecutionError> {
        Self::new_with_bundled_tools(workspace_root, None)
    }

    pub(crate) fn new_with_bundled_tools(
        workspace_root: impl AsRef<Path>,
        bundled_tools: Option<BundledTools>,
    ) -> Result<Self, ExecutionError> {
        let workspace_root = workspace_root
            .as_ref()
            .canonicalize()
            .map_err(|e| ExecutionError::Invalid(e.to_string()))?;
        if !workspace_root.is_dir() {
            return Err(ExecutionError::Invalid(
                "workspace root is not a directory".into(),
            ));
        }
        Ok(Self {
            workspace_root,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            grants: Arc::new(RwLock::new(Vec::new())),
            recovery_dir: None,
            default_shell: shell::default_user_shell(),
            bundled_tools,
        })
    }

    pub fn with_recovery(
        workspace_root: impl AsRef<Path>,
        data_root: impl AsRef<Path>,
    ) -> Result<Self, ExecutionError> {
        Self::with_recovery_and_bundled_tools(workspace_root, data_root, None)
    }

    pub(crate) fn with_recovery_and_bundled_tools(
        workspace_root: impl AsRef<Path>,
        data_root: impl AsRef<Path>,
        bundled_tools: Option<BundledTools>,
    ) -> Result<Self, ExecutionError> {
        let mut runtime = Self::new_with_bundled_tools(workspace_root, bundled_tools)?;
        let recovery_dir = data_root.as_ref().join("command-sessions");
        std::fs::create_dir_all(&recovery_dir)
            .map_err(|error| ExecutionError::Io(error.to_string()))?;
        let mut recovered = HashMap::new();
        for entry in std::fs::read_dir(&recovery_dir)
            .map_err(|error| ExecutionError::Io(error.to_string()))?
        {
            let path = entry
                .map_err(|error| ExecutionError::Io(error.to_string()))?
                .path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(mut view) = serde_json::from_slice::<CommandSessionView>(&bytes) else {
                continue;
            };
            if matches!(view.state, CommandState::Running) {
                view.state = CommandState::Failed {
                    message: "application exited while command was running".into(),
                };
                view.finished_at_ms = Some(now_ms());
                let _ = write_recovery_view(&recovery_dir, &view);
            }
            recovered.insert(
                view.id.clone(),
                Arc::new(Session {
                    id: view.id,
                    mode: view.mode,
                    started_at_ms: view.started_at_ms,
                    state: Mutex::new(view.state),
                    finished_at_ms: Mutex::new(view.finished_at_ms),
                    stdin: Mutex::new(None),
                    output: Mutex::new(OutputBuffer {
                        chunks: VecDeque::new(),
                        bytes: 0,
                        limit: DEFAULT_BUFFER_BYTES,
                        next_cursor: view.next_cursor,
                        truncated: view.output_truncated,
                    }),
                    cancel: CancellationToken::new(),
                    changed: Notify::new(),
                }),
            );
        }
        runtime.sessions = Arc::new(Mutex::new(recovered));
        runtime.recovery_dir = Some(recovery_dir);
        Ok(runtime)
    }

    pub fn configure_reusable_authorizations(&self, grants: Vec<ReusableAuthorization>) {
        *self
            .grants
            .write()
            .expect("authorization lock should not be poisoned") = grants;
    }

    pub fn assess(&self, request: &StartCommandRequest) -> CommandAssessment {
        let assessment = assess_command(&request.program, &request.args);
        self.apply_reusable_authorization(request, assessment)
    }

    pub fn shell_request(
        &self,
        command: &str,
        cwd: String,
        timeout_ms: u64,
    ) -> StartCommandRequest {
        self.default_shell.request(command, cwd, timeout_ms)
    }

    pub fn default_shell_name(&self) -> &'static str {
        self.default_shell.name()
    }

    pub(crate) fn uses_windows_powershell_native_pipeline(&self) -> bool {
        self.default_shell.uses_windows_powershell_native_pipeline()
    }

    pub(crate) fn has_bundled_ripgrep(&self) -> bool {
        self.bundled_tools.is_some()
    }

    pub fn assess_shell_command(
        &self,
        command: &str,
        cwd: String,
        timeout_ms: u64,
    ) -> CommandAssessment {
        let request = self.shell_request(command, cwd, timeout_ms);
        self.apply_reusable_authorization(&request, self.default_shell.assess(command))
    }

    fn apply_reusable_authorization(
        &self,
        request: &StartCommandRequest,
        mut assessment: CommandAssessment,
    ) -> CommandAssessment {
        if assessment.requires_approval
            && self
                .grants
                .read()
                .expect("authorization lock should not be poisoned")
                .iter()
                .any(|grant| grant.matches(request))
        {
            assessment.requires_approval = false;
            assessment.reason = "command matches a host-configured reusable authorization".into();
        }
        assessment
    }

    pub async fn start(
        &self,
        request: StartCommandRequest,
    ) -> Result<CommandSessionView, ExecutionError> {
        if request.program.trim().is_empty() {
            return Err(ExecutionError::Invalid("program must not be empty".into()));
        }
        if request
            .timeout_ms
            .is_some_and(|v| v == 0 || v > MAX_TIMEOUT_MS)
        {
            return Err(ExecutionError::Invalid(format!(
                "timeout must be between 1 and {MAX_TIMEOUT_MS} ms"
            )));
        }
        let cwd = resolve_cwd(&self.workspace_root, &request.cwd)?;
        // P10-035: canonicalize 注入的 \\?\ 前缀仅用于内部路径比对，
        // 传给子进程（包括 Windows 终端子进程）会原样出现在提示符或
        // `cd %CD%` 输出中。CommandBuilder 接受 user-facing 路径，
        // 因此这里在交给 PTY 之前剥掉 verbatim 前缀。
        let cwd_for_shell = strip_verbatim_prefix(&cwd);
        let limit = request
            .buffer_bytes
            .unwrap_or(DEFAULT_BUFFER_BYTES)
            .clamp(1024, MAX_BUFFER_BYTES);
        let id = Uuid::new_v4().to_string();
        let session = Arc::new(Session {
            id: id.clone(),
            mode: request.mode,
            started_at_ms: now_ms(),
            state: Mutex::new(CommandState::Running),
            finished_at_ms: Mutex::new(None),
            stdin: Mutex::new(None),
            output: Mutex::new(OutputBuffer {
                chunks: VecDeque::new(),
                bytes: 0,
                limit,
                next_cursor: 0,
                truncated: false,
            }),
            cancel: CancellationToken::new(),
            changed: Notify::new(),
        });
        let mut command = Command::new(&request.program);
        let bundled_path = self
            .bundled_tools
            .as_ref()
            .map(|tools| tools.path_for(&request.env));
        command
            .args(&request.args)
            .current_dir(&cwd_for_shell)
            .envs(request.env.iter().filter(|(key, _)| {
                !is_sensitive_key(key) && (bundled_path.is_none() || !is_path_key(key))
            }))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(bundled_path) = bundled_path {
            command.env("PATH", bundled_path);
        }
        configure_process_group(&mut command);
        let mut child = command
            .spawn()
            .map_err(|e| ExecutionError::Io(e.to_string()))?;
        let pid = child
            .id()
            .ok_or_else(|| ExecutionError::Io("spawned process has no pid".into()))?;
        *session.stdin.lock().await = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ExecutionError::Io("stdout was not captured".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ExecutionError::Io("stderr was not captured".into()))?;
        self.sessions.lock().await.insert(id, session.clone());
        if let Some(directory) = &self.recovery_dir {
            write_recovery_view(directory, &self.status(&session.id).await?)?;
        }
        let stdout_task = tokio::spawn(read_stream(stdout, OutputStream::Stdout, session.clone()));
        let stderr_task = tokio::spawn(read_stream(stderr, OutputStream::Stderr, session.clone()));
        let timeout = request.timeout_ms.map(Duration::from_millis);
        let watcher_session = session.clone();
        let recovery_dir = self.recovery_dir.clone();
        tokio::spawn(async move {
            let outcome = match timeout {
                Some(duration) => tokio::select! {
                    result = child.wait() => wait_state(result),
                    _ = watcher_session.cancel.cancelled() => { terminate_tree(pid).await; let _ = child.wait().await; CommandState::Cancelled },
                    _ = tokio::time::sleep(duration) => { terminate_tree(pid).await; let _ = child.wait().await; CommandState::TimedOut },
                },
                None => tokio::select! {
                    result = child.wait() => wait_state(result),
                    _ = watcher_session.cancel.cancelled() => { terminate_tree(pid).await; let _ = child.wait().await; CommandState::Cancelled },
                },
            };
            drain_output_tasks(stdout_task, stderr_task).await;
            *watcher_session.stdin.lock().await = None;
            *watcher_session.state.lock().await = outcome;
            *watcher_session.finished_at_ms.lock().await = Some(now_ms());
            if let Some(directory) = recovery_dir {
                if let Ok(view) = session_view(&watcher_session).await {
                    let _ = write_recovery_view(&directory, &view);
                }
            }
            watcher_session.changed.notify_waiters();
        });
        self.status(&session.id).await
    }

    async fn get(&self, id: &str) -> Result<Arc<Session>, ExecutionError> {
        self.sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| ExecutionError::NotFound(id.into()))
    }

    pub async fn status(&self, id: &str) -> Result<CommandSessionView, ExecutionError> {
        let session = self.get(id).await?;
        session_view(&session).await
    }

    pub async fn read(
        &self,
        id: &str,
        cursor: u64,
        limit: usize,
    ) -> Result<OutputPage, ExecutionError> {
        let session = self.get(id).await?;
        let output = session.output.lock().await;
        let oldest = output
            .chunks
            .front()
            .map_or(output.next_cursor, |c| c.cursor);
        let chunks = output
            .chunks
            .iter()
            .filter(|c| c.cursor >= cursor)
            .take(limit.clamp(1, 1000))
            .cloned()
            .collect::<Vec<_>>();
        let next_cursor = chunks
            .last()
            .map_or(cursor.max(oldest).min(output.next_cursor), |c| c.cursor + 1);
        Ok(OutputPage {
            chunks,
            next_cursor,
            oldest_cursor: oldest,
            truncated_before_cursor: cursor < oldest,
        })
    }

    pub async fn wait(&self, id: &str) -> Result<CommandSessionView, ExecutionError> {
        let session = self.get(id).await?;
        loop {
            let changed = session.changed.notified();
            if session.state.lock().await.finished() {
                return self.status(id).await;
            }
            tokio::select! {
                _ = changed => {}
                _ = tokio::time::sleep(Duration::from_millis(COMMAND_WAIT_STATUS_POLL_MS)) => {}
            }
        }
    }

    pub async fn write_stdin(&self, id: &str, input: &str) -> Result<(), ExecutionError> {
        let session = self.get(id).await?;
        let mut stdin = session.stdin.lock().await;
        let writer = stdin
            .as_mut()
            .ok_or_else(|| ExecutionError::Closed(id.into()))?;
        writer
            .write_all(input.as_bytes())
            .await
            .map_err(|e| ExecutionError::Io(e.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|e| ExecutionError::Io(e.to_string()))
    }

    pub async fn cancel(&self, id: &str) -> Result<bool, ExecutionError> {
        let session = self.get(id).await?;
        if session.state.lock().await.finished() {
            return Ok(false);
        }
        session.cancel.cancel();
        Ok(true)
    }

    pub async fn close(&self, id: &str) -> Result<(), ExecutionError> {
        let session = self.get(id).await?;
        if !session.state.lock().await.finished() {
            session.cancel.cancel();
            let _ = self.wait(id).await?;
        }
        self.sessions.lock().await.remove(id);
        if let Some(directory) = &self.recovery_dir {
            let path = directory.join(format!("{id}.json"));
            if path.exists() {
                std::fs::remove_file(path)
                    .map_err(|error| ExecutionError::Io(error.to_string()))?;
            }
        }
        Ok(())
    }
}

async fn session_view(session: &Session) -> Result<CommandSessionView, ExecutionError> {
    let state = session.state.lock().await.clone();
    let finished_at_ms = *session.finished_at_ms.lock().await;
    let output = session.output.lock().await;
    Ok(CommandSessionView {
        id: session.id.clone(),
        mode: session.mode,
        state,
        started_at_ms: session.started_at_ms,
        finished_at_ms,
        next_cursor: output.next_cursor,
        oldest_cursor: output
            .chunks
            .front()
            .map_or(output.next_cursor, |chunk| chunk.cursor),
        output_truncated: output.truncated,
    })
}

fn write_recovery_view(directory: &Path, view: &CommandSessionView) -> Result<(), ExecutionError> {
    let target = directory.join(format!("{}.json", view.id));
    let temporary = directory.join(format!("{}.tmp", view.id));
    let bytes = serde_json::to_vec(view).map_err(|error| ExecutionError::Io(error.to_string()))?;
    std::fs::write(&temporary, bytes).map_err(|error| ExecutionError::Io(error.to_string()))?;
    std::fs::rename(temporary, target).map_err(|error| ExecutionError::Io(error.to_string()))
}

async fn read_stream<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    stream: OutputStream,
    session: Arc<Session>,
) {
    let mut lines = BufReader::new(reader).split(b'\n');
    while let Ok(Some(bytes)) = lines.next_segment().await {
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        text.push('\n');
        let text = redact(&text);
        let mut output = session.output.lock().await;
        let cursor = output.next_cursor;
        output.next_cursor += 1;
        output.bytes += text.len();
        output.chunks.push_back(OutputChunk {
            cursor,
            stream,
            text,
        });
        while output.bytes > output.limit {
            if let Some(chunk) = output.chunks.pop_front() {
                output.bytes = output.bytes.saturating_sub(chunk.text.len());
                output.truncated = true;
            } else {
                break;
            }
        }
        drop(output);
        session.changed.notify_waiters();
    }
}

async fn drain_output_tasks(
    mut stdout_task: tokio::task::JoinHandle<()>,
    mut stderr_task: tokio::task::JoinHandle<()>,
) {
    let drained = tokio::time::timeout(Duration::from_millis(OUTPUT_DRAIN_GRACE_MS), async {
        let _ = tokio::join!(&mut stdout_task, &mut stderr_task);
    })
    .await
    .is_ok();
    if !drained {
        let abort_stdout = !stdout_task.is_finished();
        let abort_stderr = !stderr_task.is_finished();
        if abort_stdout {
            stdout_task.abort();
        }
        if abort_stderr {
            stderr_task.abort();
        }
        if abort_stdout {
            let _ = stdout_task.await;
        }
        if abort_stderr {
            let _ = stderr_task.await;
        }
    }
}

fn resolve_cwd(root: &Path, cwd: &str) -> Result<PathBuf, ExecutionError> {
    let relative = Path::new(cwd);
    if relative.is_absolute()
        || relative.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ExecutionError::Invalid(
            "cwd must be a relative path inside the workspace".into(),
        ));
    }
    let target = root
        .join(relative)
        .canonicalize()
        .map_err(|e| ExecutionError::Invalid(format!("cwd cannot be resolved: {e}")))?;
    if !target.starts_with(root) || !target.is_dir() {
        return Err(ExecutionError::Invalid(
            "cwd resolves outside the workspace or is not a directory".into(),
        ));
    }
    Ok(target)
}

/// 把路径中的 Windows verbatim (`\\?\`) 前缀还原成 user-facing 形式。
///
/// Rust 的 `canonicalize` 在 Windows 上会返回带 `\\?\` 前缀的"扩展长度"路径，
/// 传给子进程（特别是 PTY 的 PowerShell/cmd）会原样回显在 `$PWD` 和
/// 提示符中。`extensions::user_facing_path` 已对诊断和文案做了同样处理，
/// 这里保持一致：仅当目标平台是 Windows 且 `cwd` 看起来是绝对路径时才剥；
/// 其他平台（包括已在 user-facing 形式下的输入）原样返回。
fn strip_verbatim_prefix(cwd: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let raw = cwd.as_os_str();
        let text = raw.to_string_lossy();
        if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{stripped}"));
        }
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    cwd.to_path_buf()
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "AUTH",
    ]
    .iter()
    .any(|part| key.contains(part))
}

pub fn redact(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            if let Some((key, _)) = word.split_once('=') {
                if is_sensitive_key(key) {
                    return format!("{key}=[REDACTED]");
                }
            }
            if word.starts_with("sk-")
                || word.starts_with("ghp_")
                || word.starts_with("github_pat_")
            {
                "[REDACTED]".into()
            } else {
                word.into()
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
        + if text.ends_with('\n') { "\n" } else { "" }
}

fn wait_state(result: std::io::Result<std::process::ExitStatus>) -> CommandState {
    match result {
        Ok(status) => CommandState::Exited {
            code: status.code().unwrap_or(-1),
        },
        Err(e) => CommandState::Failed {
            message: e.to_string(),
        },
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(unix)]
pub(crate) fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.as_std_mut().process_group(0);
}

#[cfg(windows)]
pub(crate) fn configure_process_group(command: &mut Command) {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(windows)]
pub(crate) fn hide_console_window(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(unix)]
pub(crate) fn hide_console_window(_command: &mut Command) {}

#[cfg(unix)]
pub(crate) async fn terminate_tree(pid: u32) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(windows)]
pub(crate) async fn terminate_tree(pid: u32) {
    let mut command = Command::new("taskkill");
    command
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console_window(&mut command);
    let _ = command.status().await;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartPtyRequest {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub rows: u16,
    pub cols: u16,
    pub buffer_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtySessionView {
    pub id: String,
    pub state: CommandState,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub rows: u16,
    pub cols: u16,
    pub next_cursor: u64,
    pub oldest_cursor: u64,
    pub output_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyOutputChunk {
    pub cursor: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyOutputPage {
    pub chunks: Vec<PtyOutputChunk>,
    pub next_cursor: u64,
    pub oldest_cursor: u64,
    pub truncated_before_cursor: bool,
}

#[derive(Clone)]
pub struct NativePtyRuntime {
    workspace_root: PathBuf,
    sessions: Arc<Mutex<HashMap<String, Arc<PtySession>>>>,
    bundled_tools: Option<BundledTools>,
}

struct PtySession {
    id: String,
    started_at_ms: u64,
    state: Mutex<CommandState>,
    requested_state: StdMutex<Option<CommandState>>,
    finished_at_ms: Mutex<Option<u64>>,
    size: StdMutex<PtySize>,
    master: StdMutex<Box<dyn MasterPty + Send>>,
    writer: StdMutex<Option<Box<dyn Write + Send>>>,
    killer: StdMutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    output: Mutex<PtyOutputBuffer>,
    changed: Notify,
}

struct PtyOutputBuffer {
    chunks: VecDeque<PtyOutputChunk>,
    bytes: usize,
    limit: usize,
    next_cursor: u64,
    truncated: bool,
}

impl NativePtyRuntime {
    #[cfg(test)]
    pub(crate) fn root(&self) -> PathBuf {
        self.workspace_root.clone()
    }

    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, ExecutionError> {
        Self::new_with_bundled_tools(workspace_root, None)
    }

    pub(crate) fn new_with_bundled_tools(
        workspace_root: impl AsRef<Path>,
        bundled_tools: Option<BundledTools>,
    ) -> Result<Self, ExecutionError> {
        let workspace_root = workspace_root
            .as_ref()
            .canonicalize()
            .map_err(|error| ExecutionError::Invalid(error.to_string()))?;
        if !workspace_root.is_dir() {
            return Err(ExecutionError::Invalid(
                "workspace root is not a directory".into(),
            ));
        }
        Ok(Self {
            workspace_root,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            bundled_tools,
        })
    }

    pub async fn start(&self, request: StartPtyRequest) -> Result<PtySessionView, ExecutionError> {
        if request.rows == 0 || request.cols == 0 {
            return Err(ExecutionError::Invalid(
                "PTY rows and columns must be greater than zero".into(),
            ));
        }
        // 空 program 表示界面交互终端：使用平台默认 shell 的交互式启动参数。
        let (program, args) = if request.program.trim().is_empty() {
            shell::default_user_shell().interactive_launch()
        } else {
            (request.program.clone(), request.args.clone())
        };
        let cwd = resolve_cwd(&self.workspace_root, &request.cwd)?;
        // P10-035: canonicalize 注入的 \\?\ 前缀仅用于内部路径比对，
        // 传给子进程（包括 Windows 终端子进程）会原样出现在提示符或
        // `cd %CD%` 输出中。CommandBuilder 接受 user-facing 路径，
        // 因此这里在交给 PTY 之前剥掉 verbatim 前缀。
        let cwd_for_shell = strip_verbatim_prefix(&cwd);
        let size = PtySize {
            rows: request.rows,
            cols: request.cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|error| ExecutionError::Io(error.to_string()))?;
        let mut command = CommandBuilder::new(&program);
        command.args(&args);
        command.cwd(&cwd_for_shell);
        let bundled_path = self
            .bundled_tools
            .as_ref()
            .map(|tools| tools.path_for(&request.env));
        for (key, value) in request.env.iter().filter(|(key, _)| {
            !is_sensitive_key(key) && (bundled_path.is_none() || !is_path_key(key))
        }) {
            command.env(key, value);
        }
        if let Some(bundled_path) = bundled_path {
            command.env("PATH", bundled_path);
        }
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| ExecutionError::Io(error.to_string()))?;
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| ExecutionError::Io(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| ExecutionError::Io(error.to_string()))?;
        let pid = child.process_id();
        let killer = child.clone_killer();
        let id = Uuid::new_v4().to_string();
        let limit = request
            .buffer_bytes
            .unwrap_or(DEFAULT_BUFFER_BYTES)
            .clamp(1024, MAX_BUFFER_BYTES);
        let session = Arc::new(PtySession {
            id: id.clone(),
            started_at_ms: now_ms(),
            state: Mutex::new(CommandState::Running),
            requested_state: StdMutex::new(None),
            finished_at_ms: Mutex::new(None),
            size: StdMutex::new(size),
            master: StdMutex::new(pair.master),
            writer: StdMutex::new(Some(writer)),
            killer: StdMutex::new(killer),
            pid,
            output: Mutex::new(PtyOutputBuffer {
                chunks: VecDeque::new(),
                bytes: 0,
                limit,
                next_cursor: 0,
                truncated: false,
            }),
            changed: Notify::new(),
        });
        self.sessions.lock().await.insert(id, session.clone());

        let reader_session = session.clone();
        std::thread::spawn(move || read_pty_stream(reader, reader_session));
        let wait_session = session.clone();
        std::thread::spawn(move || {
            let waited = child.wait();
            let state = wait_session
                .requested_state
                .lock()
                .expect("PTY terminal state lock should not be poisoned")
                .take()
                .unwrap_or_else(|| match waited {
                    Ok(status) => CommandState::Exited {
                        code: status.exit_code() as i32,
                    },
                    Err(error) => CommandState::Failed {
                        message: error.to_string(),
                    },
                });
            *wait_session.state.blocking_lock() = state;
            *wait_session.finished_at_ms.blocking_lock() = Some(now_ms());
            wait_session.changed.notify_waiters();
        });
        self.status(&session.id).await
    }

    async fn get(&self, id: &str) -> Result<Arc<PtySession>, ExecutionError> {
        self.sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| ExecutionError::NotFound(id.into()))
    }

    pub async fn status(&self, id: &str) -> Result<PtySessionView, ExecutionError> {
        let session = self.get(id).await?;
        let state = session.state.lock().await.clone();
        let finished_at_ms = *session.finished_at_ms.lock().await;
        let size = *session
            .size
            .lock()
            .expect("PTY size lock should not be poisoned");
        let output = session.output.lock().await;
        Ok(PtySessionView {
            id: session.id.clone(),
            state,
            started_at_ms: session.started_at_ms,
            finished_at_ms,
            rows: size.rows,
            cols: size.cols,
            next_cursor: output.next_cursor,
            oldest_cursor: output
                .chunks
                .front()
                .map_or(output.next_cursor, |chunk| chunk.cursor),
            output_truncated: output.truncated,
        })
    }

    pub async fn read(
        &self,
        id: &str,
        cursor: u64,
        limit: usize,
    ) -> Result<PtyOutputPage, ExecutionError> {
        let session = self.get(id).await?;
        let output = session.output.lock().await;
        let oldest = output
            .chunks
            .front()
            .map_or(output.next_cursor, |chunk| chunk.cursor);
        let chunks = output
            .chunks
            .iter()
            .filter(|chunk| chunk.cursor >= cursor)
            .take(limit.clamp(1, 1000))
            .cloned()
            .collect::<Vec<_>>();
        let next_cursor = chunks
            .last()
            .map_or(cursor.max(oldest).min(output.next_cursor), |chunk| {
                chunk.cursor + 1
            });
        Ok(PtyOutputPage {
            chunks,
            next_cursor,
            oldest_cursor: oldest,
            truncated_before_cursor: cursor < oldest,
        })
    }

    pub async fn write(&self, id: &str, input: &str) -> Result<(), ExecutionError> {
        let session = self.get(id).await?;
        if session.state.lock().await.finished() {
            return Err(ExecutionError::Closed(id.into()));
        }
        let mut writer = session
            .writer
            .lock()
            .map_err(|_| ExecutionError::Io("PTY writer lock was poisoned".into()))?;
        let writer = writer
            .as_mut()
            .ok_or_else(|| ExecutionError::Closed(id.into()))?;
        writer
            .write_all(input.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| ExecutionError::Io(error.to_string()))
    }

    pub async fn resize(&self, id: &str, rows: u16, cols: u16) -> Result<(), ExecutionError> {
        if rows == 0 || cols == 0 {
            return Err(ExecutionError::Invalid(
                "PTY rows and columns must be greater than zero".into(),
            ));
        }
        let session = self.get(id).await?;
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        session
            .master
            .lock()
            .map_err(|_| ExecutionError::Io("PTY master lock was poisoned".into()))?
            .resize(size)
            .map_err(|error| ExecutionError::Io(error.to_string()))?;
        *session
            .size
            .lock()
            .map_err(|_| ExecutionError::Io("PTY size lock was poisoned".into()))? = size;
        Ok(())
    }

    pub async fn wait(&self, id: &str) -> Result<PtySessionView, ExecutionError> {
        let session = self.get(id).await?;
        loop {
            let changed = session.changed.notified();
            if session.state.lock().await.finished() {
                return self.status(id).await;
            }
            changed.await;
        }
    }

    pub async fn cancel(&self, id: &str) -> Result<bool, ExecutionError> {
        let session = self.get(id).await?;
        if session.state.lock().await.finished() {
            return Ok(false);
        }
        *session
            .requested_state
            .lock()
            .map_err(|_| ExecutionError::Io("PTY terminal state lock was poisoned".into()))? =
            Some(CommandState::Cancelled);
        if let Some(pid) = session.pid {
            terminate_tree(pid).await;
        }
        let _ = session
            .killer
            .lock()
            .map_err(|_| ExecutionError::Io("PTY killer lock was poisoned".into()))?
            .kill();
        Ok(true)
    }

    pub async fn close(&self, id: &str) -> Result<(), ExecutionError> {
        let session = self.get(id).await?;
        if !session.state.lock().await.finished() {
            self.cancel(id).await?;
            let _ = self.wait(id).await?;
        }
        *session
            .writer
            .lock()
            .map_err(|_| ExecutionError::Io("PTY writer lock was poisoned".into()))? = None;
        self.sessions.lock().await.remove(id);
        Ok(())
    }
}

fn read_pty_stream(mut reader: Box<dyn Read + Send>, session: Arc<PtySession>) {
    let mut bytes = [0u8; 4096];
    loop {
        let count = match reader.read(&mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        let text = redact(&String::from_utf8_lossy(&bytes[..count]));
        let mut output = session.output.blocking_lock();
        let cursor = output.next_cursor;
        output.next_cursor += 1;
        output.bytes += text.len();
        output.chunks.push_back(PtyOutputChunk { cursor, text });
        while output.bytes > output.limit {
            if let Some(chunk) = output.chunks.pop_front() {
                output.bytes = output.bytes.saturating_sub(chunk.text.len());
                output.truncated = true;
            } else {
                break;
            }
        }
        drop(output);
        session.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_verbatim_prefix_strips_windows_extended_length_prefix() {
        // 内部 canonicalize 形式必须被还原成 user-facing 形式，
        // 才能让 PowerShell/cmd 提示符与 `cd %CD%` 输出保持整洁。
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\D:\code\k-coder")),
            PathBuf::from(r"D:\code\k-coder"),
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\UNC\server\share\repo")),
            PathBuf::from(r"\\server\share\repo"),
        );
        // 已经是 user-facing 形式时保持原样。
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"D:\code\k-coder")),
            PathBuf::from(r"D:\code\k-coder"),
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new("/tmp/k-coder")),
            PathBuf::from("/tmp/k-coder"),
        );
    }

    #[test]
    fn bundled_tools_require_the_ripgrep_executable() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            BundledTools::new(directory.path()),
            Err(ExecutionError::Invalid(message))
                if message.contains("bundled ripgrep executable is unavailable")
        ));
    }

    #[test]
    fn bundled_tools_prepend_their_directory_to_a_requested_path() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory
            .path()
            .join(if cfg!(windows) { "rg.exe" } else { "rg" });
        std::fs::write(executable, b"test executable").unwrap();
        let tools = BundledTools::new(directory.path()).unwrap();
        let existing = tempfile::tempdir().unwrap();
        let mut request_env = HashMap::new();
        request_env.insert(
            if cfg!(windows) { "Path" } else { "PATH" }.to_string(),
            existing.path().to_string_lossy().into_owned(),
        );

        let entries = env::split_paths(&tools.path_for(&request_env)).collect::<Vec<_>>();
        assert_eq!(
            entries[0],
            strip_verbatim_prefix(&directory.path().canonicalize().unwrap())
        );
        assert_eq!(entries[1], existing.path());
    }

    #[test]
    fn risk_is_derived_from_program_and_arguments() {
        assert_eq!(
            assess_command("cargo", &["test".into()]).risk,
            CommandRisk::BuildOrTest
        );
        assert_eq!(
            assess_command("corepack", &["pnpm".into(), "build".into()]).risk,
            CommandRisk::BuildOrTest
        );
        assert!(!assess_command("corepack", &["pnpm".into(), "build".into()]).requires_approval);
        assert_eq!(
            assess_command(
                "powershell",
                &["-Command".into(), "Remove-Item -Recurse target".into()]
            )
            .risk,
            CommandRisk::Destructive
        );
    }

    #[test]
    fn secrets_are_redacted() {
        assert!(!redact("API_KEY=secret sk-live-value\n").contains("secret"));
        assert!(!redact("API_KEY=secret sk-live-value\n").contains("sk-live"));
    }

    fn request(program: &str, args: &[&str], timeout_ms: u64) -> StartCommandRequest {
        StartCommandRequest {
            program: program.into(),
            args: args.iter().map(|value| (*value).into()).collect(),
            cwd: String::new(),
            env: HashMap::new(),
            mode: CommandMode::Foreground,
            timeout_ms: Some(timeout_ms),
            buffer_bytes: Some(4096),
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn bundled_ripgrep_runs_when_the_requested_path_is_empty() {
        let workspace = tempfile::tempdir().unwrap();
        let tools = BundledTools::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/resources/tools/windows-x86_64"),
        )
        .unwrap();
        let runtime =
            CommandRuntime::new_with_bundled_tools(workspace.path(), Some(tools)).unwrap();
        let mut command = runtime.shell_request("rg --version", String::new(), 5_000);
        command.env.insert("PATH".into(), String::new());

        let session = runtime.start(command).await.unwrap();
        assert_eq!(
            runtime.wait(&session.id).await.unwrap().state,
            CommandState::Exited { code: 0 }
        );
        let output = runtime
            .read(&session.id, 0, 100)
            .await
            .unwrap()
            .chunks
            .into_iter()
            .map(|chunk| chunk.text)
            .collect::<String>();
        assert!(
            output.contains("ripgrep 15.2.0"),
            "bundled rg output was {output:?}"
        );
    }

    #[cfg(windows)]
    fn shell(script: &str, timeout_ms: u64) -> StartCommandRequest {
        request("cmd", &["/D", "/S", "/C", script], timeout_ms)
    }

    #[cfg(unix)]
    fn shell(script: &str, timeout_ms: u64) -> StartCommandRequest {
        request("sh", &["-c", script], timeout_ms)
    }

    #[tokio::test]
    async fn captures_success_and_nonzero_exit() {
        let workspace = tempfile::tempdir().unwrap();
        let runtime = CommandRuntime::new(workspace.path()).unwrap();
        let success = runtime
            .start(shell("echo command-ok", 5_000))
            .await
            .unwrap();
        let status = runtime.wait(&success.id).await.unwrap();
        assert_eq!(status.state, CommandState::Exited { code: 0 });
        let output = runtime.read(&success.id, 0, 20).await.unwrap();
        assert!(
            output
                .chunks
                .iter()
                .any(|chunk| chunk.text.contains("command-ok"))
        );

        #[cfg(windows)]
        let failing = shell("exit 7", 5_000);
        #[cfg(unix)]
        let failing = shell("exit 7", 5_000);
        let failed = runtime.start(failing).await.unwrap();
        assert_eq!(
            runtime.wait(&failed.id).await.unwrap().state,
            CommandState::Exited { code: 7 }
        );
    }

    #[tokio::test]
    async fn output_drain_is_bounded_when_pipe_readers_do_not_finish() {
        let stdout_task = tokio::spawn(std::future::pending::<()>());
        let stderr_task = tokio::spawn(std::future::pending::<()>());
        let started = tokio::time::Instant::now();

        drain_output_tasks(stdout_task, stderr_task).await;

        assert!(
            started.elapsed() < Duration::from_millis(1_500),
            "output drain must not hold a finished command session open indefinitely"
        );
    }

    #[tokio::test]
    async fn wait_observes_a_finished_session_without_a_completion_notification() {
        let workspace = tempfile::tempdir().unwrap();
        let runtime = CommandRuntime::new(workspace.path()).unwrap();
        let id = Uuid::new_v4().to_string();
        let session = Arc::new(Session {
            id: id.clone(),
            mode: CommandMode::Foreground,
            started_at_ms: now_ms(),
            state: Mutex::new(CommandState::Running),
            finished_at_ms: Mutex::new(None),
            stdin: Mutex::new(None),
            output: Mutex::new(OutputBuffer {
                chunks: VecDeque::new(),
                bytes: 0,
                limit: 4096,
                next_cursor: 0,
                truncated: false,
            }),
            cancel: CancellationToken::new(),
            changed: Notify::new(),
        });
        runtime
            .sessions
            .lock()
            .await
            .insert(id.clone(), session.clone());

        let mut wait = Box::pin(runtime.wait(&id));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut wait)
                .await
                .is_err(),
            "the synthetic session should still be running"
        );
        *session.state.lock().await = CommandState::Exited { code: 0 };
        *session.finished_at_ms.lock().await = Some(now_ms());

        let status = tokio::time::timeout(Duration::from_secs(1), &mut wait)
            .await
            .expect("wait should poll terminal state even when no notification arrives")
            .unwrap();
        assert_eq!(status.state, CommandState::Exited { code: 0 });
        runtime.close(&id).await.unwrap();
    }

    #[tokio::test]
    async fn timeout_and_explicit_cancel_finish_sessions() {
        let workspace = tempfile::tempdir().unwrap();
        let runtime = CommandRuntime::new(workspace.path()).unwrap();
        #[cfg(windows)]
        let slow_script = "ping -n 31 127.0.0.1 >nul";
        #[cfg(unix)]
        let slow_script = "sleep 30";
        let timed = runtime.start(shell(slow_script, 30)).await.unwrap();
        assert_eq!(
            runtime.wait(&timed.id).await.unwrap().state,
            CommandState::TimedOut
        );

        let cancelled = runtime.start(shell(slow_script, 5_000)).await.unwrap();
        assert!(runtime.cancel(&cancelled.id).await.unwrap());
        assert_eq!(
            runtime.wait(&cancelled.id).await.unwrap().state,
            CommandState::Cancelled
        );
        runtime.close(&cancelled.id).await.unwrap();
        assert!(matches!(
            runtime.status(&cancelled.id).await,
            Err(ExecutionError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn cancellation_terminates_descendants() {
        let workspace = tempfile::tempdir().unwrap();
        let marker = workspace.path().join("descendant-survived.txt");
        let runtime = CommandRuntime::new(workspace.path()).unwrap();
        #[cfg(windows)]
        let script = format!(
            "start \"\" /B cmd /D /S /C \"ping -n 3 127.0.0.1 >nul & echo survived>\"{}\"\" & ping -n 31 127.0.0.1 >nul",
            marker.display()
        );
        #[cfg(unix)]
        let script = format!(
            "(sleep 2; echo survived > '{}') & sleep 30",
            marker.display().to_string().replace('\'', "'\\''")
        );
        let session = runtime.start(shell(&script, 10_000)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        runtime.cancel(&session.id).await.unwrap();
        assert_eq!(
            runtime.wait(&session.id).await.unwrap().state,
            CommandState::Cancelled
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            !marker.exists(),
            "a descendant survived process-tree cancellation"
        );
    }

    #[tokio::test]
    async fn pty_supports_write_resize_read_wait_and_close() {
        let workspace = tempfile::tempdir().unwrap();
        let runtime = NativePtyRuntime::new(workspace.path()).unwrap();
        #[cfg(windows)]
        let (program, args, input) = (
            "cmd",
            vec!["/D".into(), "/Q".into(), "/K".into()],
            "echo pty-ok\r\n",
        );
        #[cfg(unix)]
        let (program, args, input) = ("sh", Vec::new(), "echo pty-ok\n");
        let session = runtime
            .start(StartPtyRequest {
                program: program.into(),
                args,
                cwd: String::new(),
                env: HashMap::new(),
                rows: 24,
                cols: 80,
                buffer_bytes: Some(4096),
            })
            .await
            .unwrap();
        runtime.resize(&session.id, 40, 120).await.unwrap();
        let resized = runtime.status(&session.id).await.unwrap();
        assert_eq!((resized.rows, resized.cols), (40, 120));
        #[cfg(windows)]
        runtime.write(&session.id, "\x1b[1;1R").await.unwrap();
        runtime.write(&session.id, input).await.unwrap();
        let mut terminal_output = String::new();
        for _ in 0..120 {
            let page = runtime.read(&session.id, 0, 100).await.unwrap();
            terminal_output = page
                .chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect();
            if terminal_output.contains("pty-ok") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            terminal_output.contains("pty-ok"),
            "PTY output did not contain marker: {terminal_output:?}"
        );
        runtime.close(&session.id).await.unwrap();
        assert!(matches!(
            runtime.status(&session.id).await,
            Err(ExecutionError::NotFound(_))
        ));

        #[cfg(windows)]
        let (program, args) = (
            "cmd",
            vec!["/D".into(), "/S".into(), "/C".into(), "exit 0".into()],
        );
        #[cfg(unix)]
        let (program, args) = ("sh", vec!["-c".into(), "exit 0".into()]);
        let short = runtime
            .start(StartPtyRequest {
                program: program.into(),
                args,
                cwd: String::new(),
                env: HashMap::new(),
                rows: 24,
                cols: 80,
                buffer_bytes: Some(4096),
            })
            .await
            .unwrap();
        #[cfg(windows)]
        runtime.write(&short.id, "\x1b[1;1R").await.unwrap();
        assert!(matches!(
            runtime.wait(&short.id).await.unwrap().state,
            CommandState::Exited { code: 0 }
        ));
        runtime.close(&short.id).await.unwrap();
    }

    #[tokio::test]
    async fn pty_empty_program_starts_default_interactive_shell_in_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let runtime = NativePtyRuntime::new(workspace.path()).unwrap();
        let session = runtime
            .start(StartPtyRequest {
                program: String::new(),
                args: Vec::new(),
                cwd: String::new(),
                env: HashMap::new(),
                rows: 24,
                cols: 80,
                buffer_bytes: Some(8192),
            })
            .await
            .unwrap();
        #[cfg(windows)]
        runtime.write(&session.id, "\x1b[1;1R").await.unwrap();
        #[cfg(windows)]
        let input = "echo pty-default-ok\r\n";
        #[cfg(unix)]
        let input = "echo pty-default-ok\n";
        runtime.write(&session.id, input).await.unwrap();
        let mut terminal_output = String::new();
        for _ in 0..240 {
            let page = runtime.read(&session.id, 0, 200).await.unwrap();
            terminal_output = page
                .chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect();
            if terminal_output.contains("pty-default-ok") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            terminal_output.contains("pty-default-ok"),
            "default interactive shell did not echo marker: {terminal_output:?}"
        );
        runtime.close(&session.id).await.unwrap();
    }

    #[test]
    fn reusable_authorization_cannot_be_supplied_by_request() {
        let workspace = tempfile::tempdir().unwrap();
        let runtime = CommandRuntime::new(workspace.path()).unwrap();
        let command = shell("echo safe", 1_000);
        runtime.configure_reusable_authorizations(vec![ReusableAuthorization::host_configured(
            command.program.clone(),
            command.args.clone(),
            "",
        )]);
        assert!(!runtime.assess(&command).requires_approval);
        let serialized = serde_json::to_value(command).unwrap();
        assert!(serialized.get("authorization").is_none());
    }

    #[tokio::test]
    async fn recovers_an_interrupted_command_as_failed_and_closeable() {
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let directory = data.path().join("command-sessions");
        std::fs::create_dir_all(&directory).unwrap();
        let view = CommandSessionView {
            id: Uuid::new_v4().to_string(),
            mode: CommandMode::Background,
            state: CommandState::Running,
            started_at_ms: 1,
            finished_at_ms: None,
            next_cursor: 0,
            oldest_cursor: 0,
            output_truncated: false,
        };
        write_recovery_view(&directory, &view).unwrap();

        let runtime = CommandRuntime::with_recovery(workspace.path(), data.path()).unwrap();
        let recovered = runtime.status(&view.id).await.unwrap();
        assert!(
            matches!(recovered.state, CommandState::Failed { ref message } if message.contains("application exited"))
        );
        runtime.close(&view.id).await.unwrap();
        assert!(!directory.join(format!("{}.json", view.id)).exists());
    }
}
