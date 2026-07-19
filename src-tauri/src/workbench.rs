use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use base64::Engine;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::persistence::{ProjectRecord, ProjectionDb};
use crate::storage::now_ms;

const MAX_DIRECTORY_ENTRIES: usize = 500;
const MAX_PREVIEW_BYTES: usize = 256 * 1024;
const MAX_ATTACHMENT_BYTES: usize = 4 * 1024 * 1024;
const IGNORED: &[&str] = &[".git", "node_modules", "target", "dist", "build", ".next"];

#[derive(Debug, thiserror::Error)]
pub enum WorkbenchError {
    #[error("invalid workspace request: {0}")]
    Invalid(String),
    #[error("workspace I/O failed: {0}")]
    Io(String),
    #[error("Git operation failed: {0}")]
    Git(String),
    #[error("project registry failed: {0}")]
    Registry(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub current: ProjectRecord,
    pub recent: Vec<ProjectRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub modified_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub path: String,
    pub name: String,
    pub language: String,
    pub content: Option<String>,
    pub data_url: Option<String>,
    pub size: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentContent {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub content: String,
    pub size: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileStatus {
    pub path: String,
    pub index_status: String,
    pub worktree_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusView {
    pub is_repository: bool,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<GitFileStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchView {
    pub current: Option<String>,
    pub branches: Vec<String>,
}

pub fn register_project(
    db: &ProjectionDb,
    path: &Path,
    trusted: bool,
) -> Result<ProjectRecord, WorkbenchError> {
    let canonical = path
        .canonicalize()
        .map_err(|error| WorkbenchError::Invalid(error.to_string()))?;
    if !canonical.is_dir() {
        return Err(WorkbenchError::Invalid(
            "selected workspace is not a directory".into(),
        ));
    }
    let path_string = canonical.to_string_lossy().to_string();
    let existing = db
        .list_projects()
        .map_err(|error| WorkbenchError::Registry(error.to_string()))?
        .into_iter()
        .find(|project| project.path == path_string);
    let project = ProjectRecord {
        id: existing
            .as_ref()
            .map_or_else(|| Uuid::new_v4().to_string(), |value| value.id.clone()),
        name: canonical
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(&path_string)
            .to_string(),
        path: path_string,
        trusted: existing.as_ref().is_some_and(|value| value.trusted) || trusted,
        last_opened_at_ms: now_ms(),
    };
    db.upsert_project(&project)
        .map_err(|error| WorkbenchError::Registry(error.to_string()))?;
    Ok(project)
}

pub fn workspace_state(
    db: &ProjectionDb,
    current: &Path,
) -> Result<WorkspaceState, WorkbenchError> {
    let current = register_project(db, current, true)?;
    let recent = db
        .list_projects()
        .map_err(|error| WorkbenchError::Registry(error.to_string()))?;
    Ok(WorkspaceState { current, recent })
}

pub fn list_directory(root: &Path, relative: &str) -> Result<Vec<FileEntry>, WorkbenchError> {
    let root = canonical_workspace(root)?;
    let directory = resolve(&root, relative, true)?;
    let mut entries = std::fs::read_dir(directory)
        .map_err(|error| WorkbenchError::Io(error.to_string()))?
        .filter_map(Result::ok)
        .filter(|entry| !IGNORED.contains(&entry.file_name().to_string_lossy().as_ref()))
        .take(MAX_DIRECTORY_ENTRIES)
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let path = entry
                .path()
                .strip_prefix(&root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            Some(FileEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path,
                is_directory: metadata.is_dir(),
                size: metadata.is_file().then_some(metadata.len()),
                modified_at_ms: metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|value| value.as_millis() as u64),
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| (!entry.is_directory, entry.name.to_lowercase()));
    Ok(entries)
}

pub fn preview_file(root: &Path, relative: &str) -> Result<FilePreview, WorkbenchError> {
    let path = resolve(root, relative, false)?;
    let metadata = path
        .metadata()
        .map_err(|error| WorkbenchError::Io(error.to_string()))?;
    if !metadata.is_file() {
        return Err(WorkbenchError::Invalid(
            "preview target is not a file".into(),
        ));
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(relative)
        .to_string();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    if is_image(&extension) {
        let bytes = std::fs::read(&path).map_err(|error| WorkbenchError::Io(error.to_string()))?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(WorkbenchError::Invalid(
                "image exceeds the 4 MiB preview limit".into(),
            ));
        }
        return Ok(FilePreview {
            path: relative.into(),
            name,
            language: "image".into(),
            content: None,
            data_url: Some(format!(
                "data:{};base64,{}",
                image_mime(&extension),
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )),
            size: metadata.len(),
            truncated: false,
        });
    }
    let bytes = std::fs::read(&path).map_err(|error| WorkbenchError::Io(error.to_string()))?;
    let truncated = bytes.len() > MAX_PREVIEW_BYTES;
    let end = bytes.len().min(MAX_PREVIEW_BYTES);
    let content = String::from_utf8_lossy(&bytes[..end]).to_string();
    Ok(FilePreview {
        path: relative.into(),
        name,
        language: language(&extension).into(),
        content: Some(content),
        data_url: None,
        size: metadata.len(),
        truncated,
    })
}

pub fn extract_attachment(
    root: &Path,
    relative: &str,
) -> Result<AttachmentContent, WorkbenchError> {
    let preview = preview_file(root, relative)?;
    let (kind, content) = if let Some(data_url) = preview.data_url.clone() {
        ("image", data_url)
    } else {
        ("document", preview.content.clone().unwrap_or_default())
    };
    Ok(AttachmentContent {
        path: preview.path,
        name: preview.name,
        kind: kind.into(),
        content,
        size: preview.size,
        truncated: preview.truncated,
    })
}

pub fn open_external(root: &Path, relative: &str, reveal: bool) -> Result<(), WorkbenchError> {
    let path = resolve(root, relative, false)?;
    #[cfg(target_os = "windows")]
    let status = if reveal {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .status()
    } else {
        Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .status()
    };
    #[cfg(target_os = "macos")]
    let status = if reveal {
        Command::new("open")
            .args(["-R", &path.to_string_lossy()])
            .status()
    } else {
        Command::new("open").arg(&path).status()
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open")
        .arg(if reveal {
            path.parent().unwrap_or(root)
        } else {
            &path
        })
        .status();
    status.map_err(|error| WorkbenchError::Io(error.to_string()))?;
    Ok(())
}

pub fn git_status(root: &Path) -> Result<GitStatusView, WorkbenchError> {
    let output = git(root, &["status", "--porcelain=v1", "--branch"]);
    let Ok(output) = output else {
        return Ok(GitStatusView {
            is_repository: false,
            branch: None,
            upstream: None,
            ahead: 0,
            behind: 0,
            files: vec![],
        });
    };
    let mut lines = output.lines();
    let header = lines.next().unwrap_or("").trim_start_matches("## ");
    let (branch_part, tracking) = header.split_once("...").unwrap_or((header, ""));
    let branch = (!branch_part.is_empty()).then(|| {
        branch_part
            .split_whitespace()
            .next()
            .unwrap_or(branch_part)
            .to_string()
    });
    let upstream = (!tracking.is_empty()).then(|| {
        tracking
            .split_whitespace()
            .next()
            .unwrap_or(tracking)
            .to_string()
    });
    let ahead = parse_counter(header, "ahead ");
    let behind = parse_counter(header, "behind ");
    let files = lines
        .filter(|line| line.len() >= 3)
        .map(|line| GitFileStatus {
            index_status: line[0..1].to_string(),
            worktree_status: line[1..2].to_string(),
            path: line[3..].to_string(),
        })
        .collect();
    Ok(GitStatusView {
        is_repository: true,
        branch,
        upstream,
        ahead,
        behind,
        files,
    })
}

pub fn git_diff(root: &Path, path: Option<&str>, staged: bool) -> Result<String, WorkbenchError> {
    let mut args = vec!["diff"];
    if staged {
        args.push("--cached");
    }
    if let Some(path) = path {
        validate_relative(path)?;
        args.extend(["--", path]);
    }
    git(root, &args)
}

pub fn git_branches(root: &Path) -> Result<GitBranchView, WorkbenchError> {
    let output = git(root, &["branch", "--format=%(HEAD)%00%(refname:short)"])?;
    let mut current = None;
    let mut branches = Vec::new();
    for line in output.lines() {
        let Some((head, name)) = line.split_once('\0') else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        if head == "*" {
            current = Some(name.to_string());
        }
        branches.push(name.to_string());
    }
    branches.sort();
    Ok(GitBranchView { current, branches })
}

pub fn git_switch_branch(
    root: &Path,
    branch: &str,
    create: bool,
    confirmed: bool,
) -> Result<String, WorkbenchError> {
    if !confirmed {
        return Err(WorkbenchError::Invalid(
            "switching branches requires explicit confirmation".into(),
        ));
    }
    let branch = branch.trim();
    if branch.is_empty() || branch.len() > 255 {
        return Err(WorkbenchError::Invalid("branch name is invalid".into()));
    }
    git(root, &["check-ref-format", "--branch", branch])?;
    if create {
        git(root, &["switch", "-c", branch])
    } else {
        git(root, &["switch", branch])
    }
}

pub fn git_action(
    root: &Path,
    action: &str,
    paths: &[String],
    message: Option<&str>,
    confirmed: bool,
) -> Result<String, WorkbenchError> {
    let allowed = HashSet::from(["stage", "unstage", "commit", "pull", "push"]);
    if !allowed.contains(action) {
        return Err(WorkbenchError::Invalid(
            "destructive Git actions require a separate approval flow".into(),
        ));
    }
    for path in paths {
        validate_relative(path)?;
    }
    if matches!(action, "commit" | "pull" | "push") && !confirmed {
        return Err(WorkbenchError::Invalid(format!(
            "Git {action} requires explicit confirmation"
        )));
    }
    match action {
        "stage" => {
            let mut args = vec!["add", "--"];
            args.extend(paths.iter().map(String::as_str));
            git(root, &args)
        }
        "unstage" => {
            let mut args = vec!["restore", "--staged", "--"];
            args.extend(paths.iter().map(String::as_str));
            git(root, &args)
        }
        "commit" => {
            let message = message
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| WorkbenchError::Invalid("commit message is required".into()))?;
            git(root, &["commit", "-m", message])
        }
        "pull" => git(root, &["pull", "--ff-only"]),
        "push" => git(root, &["push"]),
        _ => unreachable!(),
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String, WorkbenchError> {
    let output = Command::new("git")
        .args(["--literal-pathspecs", "-C", &root.to_string_lossy()])
        .args(args)
        .output()
        .map_err(|error| WorkbenchError::Git(error.to_string()))?;
    if !output.status.success() {
        return Err(WorkbenchError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn resolve(root: &Path, relative: &str, directory: bool) -> Result<PathBuf, WorkbenchError> {
    validate_relative(relative)?;
    let root = canonical_workspace(root)?;
    let candidate = root
        .join(relative)
        .canonicalize()
        .map_err(|error| WorkbenchError::Io(error.to_string()))?;
    if !candidate.starts_with(root) {
        return Err(WorkbenchError::Invalid(
            "path resolves outside the workspace".into(),
        ));
    }
    if directory && !candidate.is_dir() {
        return Err(WorkbenchError::Invalid("path is not a directory".into()));
    }
    Ok(candidate)
}

fn canonical_workspace(root: &Path) -> Result<PathBuf, WorkbenchError> {
    let root = root
        .canonicalize()
        .map_err(|error| WorkbenchError::Io(error.to_string()))?;
    if !root.is_dir() {
        return Err(WorkbenchError::Invalid(
            "workspace root is not a directory".into(),
        ));
    }
    Ok(root)
}

fn validate_relative(path: &str) -> Result<(), WorkbenchError> {
    let path = Path::new(path);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(WorkbenchError::Invalid(
            "path must stay inside the workspace".into(),
        ));
    }
    Ok(())
}

fn parse_counter(header: &str, marker: &str) -> u32 {
    header
        .find(marker)
        .and_then(|index| {
            header[index + marker.len()..]
                .split(|value: char| !value.is_ascii_digit())
                .next()
        })
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn is_image(extension: &str) -> bool {
    matches!(extension, "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
}
fn image_mime(extension: &str) -> &'static str {
    match extension {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}
fn language(extension: &str) -> &'static str {
    match extension {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "json" => "json",
        "md" => "markdown",
        "css" => "css",
        "html" => "html",
        "py" => "python",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        _ => "text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repository(root: &Path) {
        git(root, &["init"]).unwrap();
        git(root, &["config", "user.email", "test@example.com"]).unwrap();
        git(root, &["config", "user.name", "k-Coder Tests"]).unwrap();
    }

    #[test]
    fn lists_and_previews_only_workspace_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::create_dir(root.path().join("node_modules")).unwrap();
        let entries = list_directory(root.path(), "").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            preview_file(root.path(), "main.rs").unwrap().language,
            "rust"
        );
        assert!(preview_file(root.path(), "../outside").is_err());
    }

    #[test]
    fn git_mutations_require_confirmation_and_support_branches() {
        let root = tempfile::tempdir().unwrap();
        init_repository(root.path());
        std::fs::write(root.path().join("tracked.txt"), "first\n").unwrap();

        git_action(root.path(), "stage", &["tracked.txt".into()], None, false).unwrap();
        assert!(git_action(root.path(), "commit", &[], Some("initial"), false).is_err());
        git_action(root.path(), "commit", &[], Some("initial"), true).unwrap();

        let branches = git_branches(root.path()).unwrap();
        assert_eq!(branches.branches.len(), 1);
        assert!(git_switch_branch(root.path(), "feature/test", true, false).is_err());
        git_switch_branch(root.path(), "feature/test", true, true).unwrap();
        assert_eq!(
            git_branches(root.path()).unwrap().current.as_deref(),
            Some("feature/test")
        );
    }

    #[test]
    fn git_rejects_path_traversal_and_unknown_actions() {
        let root = tempfile::tempdir().unwrap();
        init_repository(root.path());
        assert!(git_diff(root.path(), Some("../outside"), false).is_err());
        assert!(git_action(root.path(), "reset", &[], None, true).is_err());
    }
}
