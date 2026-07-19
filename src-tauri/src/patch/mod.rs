use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use similar::TextDiff;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::protocol::{
    ChangeFileSnapshot, ChangeSet, ExpectedFileHash, FileOperation, PatchFilePreview, PatchPreview,
};
use crate::storage::now_ms;

const MAX_PATCH_BYTES: usize = 1024 * 1024;
const MAX_PATCH_FILES: usize = 20;
const MAX_FILE_BYTES: usize = 512 * 1024;
const MAX_TOTAL_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchDocument {
    pub operations: Vec<FilePatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePatch {
    pub path: String,
    pub kind: FilePatchKind,
    pub move_to: Option<String>,
    pub hunks: Vec<PatchHunk>,
    pub content_lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePatchKind {
    Add,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchHunk {
    pub lines: Vec<HunkLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkLine {
    pub kind: HunkLineKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkLineKind {
    Context,
    Add,
    Remove,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PatchError {
    #[error("patch syntax is invalid: {0}")]
    InvalidSyntax(String),
    #[error("patch path is denied: {0}")]
    DeniedPath(String),
    #[error("patch conflicts with the workspace: {0}")]
    Conflict(String),
    #[error("patch exceeds a safety limit: {0}")]
    Limit(String),
    #[error("patch I/O failed: {0}")]
    Io(String),
    #[error("change cannot be undone: {0}")]
    UndoConflict(String),
}

#[derive(Clone, Default)]
pub struct PatchService {
    edit_lock: Arc<Mutex<()>>,
}

impl PatchService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn preview_patch(
        &self,
        workspace_root: &Path,
        patch: &str,
    ) -> Result<PatchPreview, PatchError> {
        preview_document(workspace_root, patch)
    }

    pub fn preview_write_file(
        &self,
        workspace_root: &Path,
        path: &str,
        content: &str,
    ) -> Result<PatchPreview, PatchError> {
        preview_full_write(workspace_root, path, content)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_patch(
        &self,
        workspace_root: PathBuf,
        thread_id: String,
        turn_id: String,
        tool_call_id: String,
        patch: String,
        selected_paths: Vec<String>,
        expected_hashes: Vec<ExpectedFileHash>,
    ) -> Result<ChangeSet, PatchError> {
        let _guard = self.edit_lock.lock().await;
        tokio::task::spawn_blocking(move || {
            let preview = preview_document(&workspace_root, &patch)?;
            let selected = select_previews(preview.files, &selected_paths)?;
            verify_expected_hashes(&selected, &expected_hashes)?;
            apply_previews(&workspace_root, &selected, None)?;
            Ok(change_set(thread_id, turn_id, tool_call_id, selected))
        })
        .await
        .map_err(|error| PatchError::Io(error.to_string()))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_write_file(
        &self,
        workspace_root: PathBuf,
        thread_id: String,
        turn_id: String,
        tool_call_id: String,
        path: String,
        content: String,
        expected_hashes: Vec<ExpectedFileHash>,
    ) -> Result<ChangeSet, PatchError> {
        let _guard = self.edit_lock.lock().await;
        tokio::task::spawn_blocking(move || {
            let preview = preview_full_write(&workspace_root, &path, &content)?;
            verify_expected_hashes(&preview.files, &expected_hashes)?;
            apply_previews(&workspace_root, &preview.files, None)?;
            Ok(change_set(thread_id, turn_id, tool_call_id, preview.files))
        })
        .await
        .map_err(|error| PatchError::Io(error.to_string()))?
    }

    pub async fn undo(
        &self,
        workspace_root: PathBuf,
        change_set: ChangeSet,
    ) -> Result<ChangeSet, PatchError> {
        let _guard = self.edit_lock.lock().await;
        tokio::task::spawn_blocking(move || {
            verify_undo_state(&workspace_root, &change_set.files)?;
            rollback_snapshots(&workspace_root, &change_set.files)?;
            let mut undone = change_set;
            undone.undone = true;
            Ok(undone)
        })
        .await
        .map_err(|error| PatchError::Io(error.to_string()))?
    }

    pub async fn redo(
        &self,
        workspace_root: PathBuf,
        change_set: ChangeSet,
    ) -> Result<ChangeSet, PatchError> {
        let _guard = self.edit_lock.lock().await;
        tokio::task::spawn_blocking(move || {
            verify_redo_state(&workspace_root, &change_set.files)?;
            let previews = snapshots_as_previews(&change_set.files);
            apply_previews(&workspace_root, &previews, None)?;
            let mut redone = change_set;
            redone.undone = false;
            Ok(redone)
        })
        .await
        .map_err(|error| PatchError::Io(error.to_string()))?
    }
}

pub fn parse_patch(patch: &str) -> Result<PatchDocument, PatchError> {
    if patch.len() > MAX_PATCH_BYTES {
        return Err(PatchError::Limit(format!(
            "patch is larger than {MAX_PATCH_BYTES} bytes"
        )));
    }
    let normalized = patch.replace("\r\n", "\n");
    let lines = normalized.lines().collect::<Vec<_>>();
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        return Err(PatchError::InvalidSyntax(
            "patch must start with '*** Begin Patch' and end with '*** End Patch'".to_string(),
        ));
    }

    let mut operations = Vec::new();
    let mut index = 1usize;
    while index + 1 < lines.len() {
        if operations.len() >= MAX_PATCH_FILES {
            return Err(PatchError::Limit(format!(
                "patch contains more than {MAX_PATCH_FILES} file operations"
            )));
        }
        let header = lines[index];
        let (kind, path) = if let Some(path) = header.strip_prefix("*** Add File: ") {
            (FilePatchKind::Add, path)
        } else if let Some(path) = header.strip_prefix("*** Update File: ") {
            (FilePatchKind::Update, path)
        } else if let Some(path) = header.strip_prefix("*** Delete File: ") {
            (FilePatchKind::Delete, path)
        } else {
            return Err(PatchError::InvalidSyntax(format!(
                "expected a file header at line {}, found {header:?}",
                index + 1
            )));
        };
        if path.is_empty() || path.trim() != path {
            return Err(PatchError::InvalidSyntax(format!(
                "file path at line {} is empty or has surrounding whitespace",
                index + 1
            )));
        }
        index += 1;

        let mut move_to = None;
        if kind == FilePatchKind::Update && index + 1 < lines.len() {
            if let Some(path) = lines[index].strip_prefix("*** Move to: ") {
                if path.is_empty() || path.trim() != path {
                    return Err(PatchError::InvalidSyntax(format!(
                        "move destination at line {} is invalid",
                        index + 1
                    )));
                }
                move_to = Some(path.to_string());
                index += 1;
            }
        }

        let mut hunks = Vec::new();
        let mut content_lines = Vec::new();
        match kind {
            FilePatchKind::Add | FilePatchKind::Delete => {
                let expected_prefix = if kind == FilePatchKind::Add { '+' } else { '-' };
                while index + 1 < lines.len() && !lines[index].starts_with("*** ") {
                    let line = lines[index];
                    if !line.starts_with(expected_prefix) {
                        return Err(PatchError::InvalidSyntax(format!(
                            "{} file content at line {} must start with {expected_prefix:?}",
                            if kind == FilePatchKind::Add {
                                "added"
                            } else {
                                "deleted"
                            },
                            index + 1
                        )));
                    }
                    content_lines.push(line[1..].to_string());
                    index += 1;
                }
            }
            FilePatchKind::Update => {
                while index + 1 < lines.len() && !is_file_header(lines[index]) {
                    if !lines[index].starts_with("@@") {
                        return Err(PatchError::InvalidSyntax(format!(
                            "update hunk at line {} must start with '@@'",
                            index + 1
                        )));
                    }
                    index += 1;
                    let mut hunk_lines = Vec::new();
                    while index + 1 < lines.len()
                        && !lines[index].starts_with("@@")
                        && !is_file_header(lines[index])
                    {
                        let line = lines[index];
                        let (kind, text) = match line.as_bytes().first().copied() {
                            Some(b' ') => (HunkLineKind::Context, &line[1..]),
                            Some(b'+') => (HunkLineKind::Add, &line[1..]),
                            Some(b'-') => (HunkLineKind::Remove, &line[1..]),
                            _ => {
                                return Err(PatchError::InvalidSyntax(format!(
                                    "hunk line {} must start with space, '+' or '-'",
                                    index + 1
                                )));
                            }
                        };
                        hunk_lines.push(HunkLine {
                            kind,
                            text: text.to_string(),
                        });
                        index += 1;
                    }
                    if hunk_lines.is_empty()
                        || !hunk_lines
                            .iter()
                            .any(|line| line.kind != HunkLineKind::Context)
                    {
                        return Err(PatchError::InvalidSyntax(
                            "each update hunk must contain an addition or removal".to_string(),
                        ));
                    }
                    hunks.push(PatchHunk { lines: hunk_lines });
                }
                if hunks.is_empty() && move_to.is_none() {
                    return Err(PatchError::InvalidSyntax(
                        "update operation requires at least one hunk or a move destination"
                            .to_string(),
                    ));
                }
            }
        }
        operations.push(FilePatch {
            path: path.to_string(),
            kind,
            move_to,
            hunks,
            content_lines,
        });
    }

    if operations.is_empty() {
        return Err(PatchError::InvalidSyntax(
            "patch does not contain a file operation".to_string(),
        ));
    }
    validate_unique_paths(&operations)?;
    Ok(PatchDocument { operations })
}

fn preview_document(workspace_root: &Path, patch: &str) -> Result<PatchPreview, PatchError> {
    let root = canonical_workspace(workspace_root)?;
    let document = parse_patch(patch)?;
    let mut files = Vec::with_capacity(document.operations.len());
    let mut total_snapshot_bytes = 0usize;

    for operation in document.operations {
        let source_relative = validate_relative_path(&operation.path)?;
        let destination_relative = operation
            .move_to
            .as_deref()
            .map(validate_relative_path)
            .transpose()?;
        let (before_content, source_path) = match operation.kind {
            FilePatchKind::Add => {
                let path = resolve_new_path(&root, &source_relative)?;
                if path.exists() {
                    return Err(PatchError::Conflict(format!(
                        "cannot add {}; it already exists",
                        operation.path
                    )));
                }
                (None, path)
            }
            FilePatchKind::Update | FilePatchKind::Delete => {
                let path = resolve_existing_file(&root, &source_relative)?;
                (Some(read_text(&path)?), path)
            }
        };

        let after_content = match operation.kind {
            FilePatchKind::Add => Some(content_from_lines(&operation.content_lines)),
            FilePatchKind::Update => {
                let before = before_content.as_deref().expect("updated file has content");
                Some(apply_hunks(before, &operation.hunks, &operation.path)?)
            }
            FilePatchKind::Delete => {
                if !operation.content_lines.is_empty() {
                    let expected = content_from_lines(&operation.content_lines);
                    if normalize_newlines(before_content.as_deref().unwrap())
                        != normalize_newlines(&expected)
                    {
                        return Err(PatchError::Conflict(format!(
                            "delete content for {} no longer matches the file",
                            operation.path
                        )));
                    }
                }
                None
            }
        };

        if let Some(destination) = &destination_relative {
            let destination_path = resolve_new_path(&root, destination)?;
            if destination_path.exists() && destination_path != source_path {
                return Err(PatchError::Conflict(format!(
                    "move destination {} already exists",
                    operation.move_to.as_deref().unwrap()
                )));
            }
        }

        enforce_file_size(before_content.as_deref(), &operation.path)?;
        enforce_file_size(after_content.as_deref(), &operation.path)?;
        total_snapshot_bytes = total_snapshot_bytes
            .saturating_add(before_content.as_ref().map_or(0, String::len))
            .saturating_add(after_content.as_ref().map_or(0, String::len));
        if total_snapshot_bytes > MAX_TOTAL_SNAPSHOT_BYTES {
            return Err(PatchError::Limit(format!(
                "combined before/after snapshots exceed {MAX_TOTAL_SNAPSHOT_BYTES} bytes"
            )));
        }

        let file_operation = match operation.kind {
            FilePatchKind::Add => FileOperation::Add,
            FilePatchKind::Delete => FileOperation::Delete,
            FilePatchKind::Update if operation.move_to.is_some() => FileOperation::Move,
            FilePatchKind::Update => FileOperation::Modify,
        };
        let display_after_path = operation.move_to.as_deref().unwrap_or(&operation.path);
        let unified_diff = unified_diff(
            &operation.path,
            display_after_path,
            before_content.as_deref().unwrap_or(""),
            after_content.as_deref().unwrap_or(""),
        );
        files.push(PatchFilePreview {
            path: operation.path,
            destination_path: operation.move_to,
            operation: file_operation,
            before_hash: before_content.as_deref().map(hash_content),
            after_hash: after_content.as_deref().map(hash_content),
            before_content,
            after_content,
            unified_diff,
        });
    }

    Ok(PatchPreview {
        patch: patch.to_string(),
        files,
        total_snapshot_bytes,
    })
}

fn preview_full_write(
    workspace_root: &Path,
    path: &str,
    content: &str,
) -> Result<PatchPreview, PatchError> {
    let root = canonical_workspace(workspace_root)?;
    let relative = validate_relative_path(path)?;
    let candidate = resolve_new_path(&root, &relative)?;
    let before_content = if candidate.exists() {
        Some(read_text(&resolve_existing_file(&root, &relative)?)?)
    } else {
        None
    };
    enforce_file_size(before_content.as_deref(), path)?;
    enforce_file_size(Some(content), path)?;
    let total_snapshot_bytes = before_content.as_ref().map_or(0, String::len) + content.len();
    if total_snapshot_bytes > MAX_TOTAL_SNAPSHOT_BYTES {
        return Err(PatchError::Limit(format!(
            "combined before/after snapshots exceed {MAX_TOTAL_SNAPSHOT_BYTES} bytes"
        )));
    }
    let operation = if before_content.is_some() {
        FileOperation::Modify
    } else {
        FileOperation::Add
    };
    Ok(PatchPreview {
        patch: String::new(),
        files: vec![PatchFilePreview {
            path: path.to_string(),
            destination_path: None,
            operation,
            before_hash: before_content.as_deref().map(hash_content),
            after_hash: Some(hash_content(content)),
            unified_diff: unified_diff(
                path,
                path,
                before_content.as_deref().unwrap_or(""),
                content,
            ),
            before_content,
            after_content: Some(content.to_string()),
        }],
        total_snapshot_bytes,
    })
}

fn select_previews(
    previews: Vec<PatchFilePreview>,
    selected_paths: &[String],
) -> Result<Vec<PatchFilePreview>, PatchError> {
    if selected_paths.is_empty() {
        return Err(PatchError::Conflict(
            "at least one file must be selected".to_string(),
        ));
    }
    let selected = selected_paths.iter().collect::<HashSet<_>>();
    if selected.len() != selected_paths.len() {
        return Err(PatchError::Conflict(
            "selected file paths contain duplicates".to_string(),
        ));
    }
    let known = previews
        .iter()
        .map(|preview| &preview.path)
        .collect::<HashSet<_>>();
    if let Some(path) = selected.iter().find(|path| !known.contains(**path)) {
        return Err(PatchError::Conflict(format!(
            "selected path {path} is not present in the patch"
        )));
    }
    Ok(previews
        .into_iter()
        .filter(|preview| selected.contains(&preview.path))
        .collect())
}

fn verify_expected_hashes(
    previews: &[PatchFilePreview],
    expected: &[ExpectedFileHash],
) -> Result<(), PatchError> {
    let mut expected_by_path = HashMap::new();
    for item in expected {
        if expected_by_path
            .insert(&item.path, &item.before_hash)
            .is_some()
        {
            return Err(PatchError::Conflict(format!(
                "reviewed hash for {} was provided more than once",
                item.path
            )));
        }
    }
    if expected_by_path.len() != previews.len() {
        return Err(PatchError::Conflict(
            "reviewed file hashes do not match the selected files".to_string(),
        ));
    }
    for preview in previews {
        match expected_by_path.get(&preview.path) {
            Some(hash) if **hash == preview.before_hash => {}
            Some(_) => {
                return Err(PatchError::Conflict(format!(
                    "{} changed after the diff was reviewed",
                    preview.path
                )));
            }
            None => {
                return Err(PatchError::Conflict(format!(
                    "reviewed hash for {} is missing",
                    preview.path
                )));
            }
        }
    }
    Ok(())
}

fn apply_previews(
    workspace_root: &Path,
    previews: &[PatchFilePreview],
    fail_after: Option<usize>,
) -> Result<(), PatchError> {
    let root = canonical_workspace(workspace_root)?;
    let mut applied = Vec::new();
    let mut created_directories = Vec::new();
    for (index, preview) in previews.iter().enumerate() {
        if fail_after == Some(index) {
            let error = PatchError::Io("injected transaction failure".to_string());
            rollback_preview_subset(&root, &applied, &created_directories)?;
            return Err(error);
        }
        let result = apply_preview(&root, preview, &mut created_directories);
        match result {
            Ok(()) => applied.push(preview.clone()),
            Err(error) => {
                // The handler may have changed the current file before its I/O error.
                applied.push(preview.clone());
                rollback_preview_subset(&root, &applied, &created_directories)?;
                return Err(error);
            }
        }
    }
    Ok(())
}

fn apply_preview(
    root: &Path,
    preview: &PatchFilePreview,
    created_directories: &mut Vec<PathBuf>,
) -> Result<(), PatchError> {
    let source = validate_relative_path(&preview.path)?;
    match preview.operation {
        FileOperation::Add => {
            let target = resolve_new_path(root, &source)?;
            ensure_parent(&target, root, created_directories)?;
            write_text(&target, preview.after_content.as_deref().unwrap_or(""))
        }
        FileOperation::Modify => {
            let target = resolve_existing_file(root, &source)?;
            write_text(&target, preview.after_content.as_deref().unwrap_or(""))
        }
        FileOperation::Delete => {
            let target = resolve_existing_file(root, &source)?;
            fs::remove_file(target).map_err(|error| PatchError::Io(error.to_string()))
        }
        FileOperation::Move => {
            let target_relative =
                validate_relative_path(preview.destination_path.as_deref().ok_or_else(|| {
                    PatchError::InvalidSyntax("move destination is missing".to_string())
                })?)?;
            let source_path = resolve_existing_file(root, &source)?;
            let target = resolve_new_path(root, &target_relative)?;
            if target.exists() {
                return Err(PatchError::Conflict(format!(
                    "move destination {} already exists",
                    preview.destination_path.as_deref().unwrap()
                )));
            }
            ensure_parent(&target, root, created_directories)?;
            fs::rename(&source_path, &target).map_err(|error| PatchError::Io(error.to_string()))?;
            if let Err(error) = write_text(&target, preview.after_content.as_deref().unwrap_or(""))
            {
                let _ = fs::rename(&target, &source_path);
                return Err(error);
            }
            Ok(())
        }
    }
}

fn rollback_preview_subset(
    root: &Path,
    previews: &[PatchFilePreview],
    created_directories: &[PathBuf],
) -> Result<(), PatchError> {
    rollback_files(root, previews)?;
    for directory in created_directories.iter().rev() {
        let _ = fs::remove_dir(directory);
    }
    Ok(())
}

fn rollback_snapshots(root: &Path, snapshots: &[ChangeFileSnapshot]) -> Result<(), PatchError> {
    rollback_files(root, &snapshots_as_previews(snapshots))
}

fn snapshots_as_previews(snapshots: &[ChangeFileSnapshot]) -> Vec<PatchFilePreview> {
    snapshots
        .iter()
        .map(|snapshot| PatchFilePreview {
            path: snapshot.path.clone(),
            destination_path: snapshot.destination_path.clone(),
            operation: snapshot.operation,
            before_hash: snapshot.before_hash.clone(),
            after_hash: snapshot.after_hash.clone(),
            before_content: snapshot.before_content.clone(),
            after_content: snapshot.after_content.clone(),
            unified_diff: snapshot.unified_diff.clone(),
        })
        .collect()
}

fn rollback_files(root: &Path, previews: &[PatchFilePreview]) -> Result<(), PatchError> {
    let root = canonical_workspace(root)?;
    for preview in previews.iter().rev() {
        let source = root.join(validate_relative_path(&preview.path)?);
        let target = match &preview.destination_path {
            Some(path) => root.join(validate_relative_path(path)?),
            None => source.clone(),
        };
        if target.exists() {
            let target = resolve_existing_file(&root, target.strip_prefix(&root).unwrap())?;
            fs::remove_file(target).map_err(|error| PatchError::Io(error.to_string()))?;
        }
        if preview.destination_path.is_some() && source.exists() {
            let source = resolve_existing_file(&root, source.strip_prefix(&root).unwrap())?;
            fs::remove_file(source).map_err(|error| PatchError::Io(error.to_string()))?;
        }
        if let Some(content) = &preview.before_content {
            let safe_source = resolve_new_path(&root, source.strip_prefix(&root).unwrap())?;
            let mut ignored = Vec::new();
            ensure_parent(&safe_source, &root, &mut ignored)?;
            write_text(&safe_source, content)?;
        }
    }
    Ok(())
}

fn verify_undo_state(root: &Path, files: &[ChangeFileSnapshot]) -> Result<(), PatchError> {
    let root = canonical_workspace(root)?;
    for file in files {
        let source_relative = validate_relative_path(&file.path)?;
        let current_path = file
            .destination_path
            .as_deref()
            .map(validate_relative_path)
            .transpose()?
            .unwrap_or_else(|| source_relative.clone());
        let current = root.join(&current_path);
        match &file.after_hash {
            Some(expected) => {
                let current = resolve_existing_file(&root, &current_path)?;
                let actual = hash_content(&read_text(&current)?);
                if &actual != expected {
                    return Err(PatchError::UndoConflict(format!(
                        "{} changed after the patch was applied",
                        current_path.display()
                    )));
                }
            }
            None if current.exists() => {
                return Err(PatchError::UndoConflict(format!(
                    "{} was recreated after deletion",
                    current_path.display()
                )));
            }
            None => {}
        }
        if file.destination_path.is_some() && root.join(source_relative).exists() {
            return Err(PatchError::UndoConflict(format!(
                "move source {} was recreated",
                file.path
            )));
        }
    }
    Ok(())
}

fn verify_redo_state(root: &Path, files: &[ChangeFileSnapshot]) -> Result<(), PatchError> {
    let root = canonical_workspace(root)?;
    for file in files {
        let source_relative = validate_relative_path(&file.path)?;
        match &file.before_hash {
            Some(expected) => {
                let current = resolve_existing_file(&root, &source_relative)?;
                let actual = hash_content(&read_text(&current)?);
                if &actual != expected {
                    return Err(PatchError::Conflict(format!(
                        "{} changed while compensating a failed audit write",
                        file.path
                    )));
                }
            }
            None => {
                let current = resolve_new_path(&root, &source_relative)?;
                if current.exists() {
                    return Err(PatchError::Conflict(format!(
                        "{} was created while compensating a failed audit write",
                        file.path
                    )));
                }
            }
        }
        if let Some(destination) = &file.destination_path {
            let destination = validate_relative_path(destination)?;
            let current = resolve_new_path(&root, &destination)?;
            if current.exists() {
                return Err(PatchError::Conflict(format!(
                    "move destination {} exists while compensating a failed audit write",
                    destination.display()
                )));
            }
        }
    }
    Ok(())
}

fn change_set(
    thread_id: String,
    turn_id: String,
    tool_call_id: String,
    previews: Vec<PatchFilePreview>,
) -> ChangeSet {
    ChangeSet {
        id: Uuid::new_v4().to_string(),
        thread_id,
        turn_id,
        tool_call_id,
        created_at_ms: now_ms(),
        files: previews
            .into_iter()
            .map(|preview| ChangeFileSnapshot {
                path: preview.path,
                destination_path: preview.destination_path,
                operation: preview.operation,
                before_hash: preview.before_hash,
                after_hash: preview.after_hash,
                before_content: preview.before_content,
                after_content: preview.after_content,
                unified_diff: preview.unified_diff,
            })
            .collect(),
        undone: false,
    }
}

fn apply_hunks(current: &str, hunks: &[PatchHunk], path: &str) -> Result<String, PatchError> {
    let uses_crlf = current.contains("\r\n");
    let mut output = normalize_newlines(current);
    for hunk in hunks {
        let old = hunk
            .lines
            .iter()
            .filter(|line| line.kind != HunkLineKind::Add)
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if old.is_empty() {
            return Err(PatchError::InvalidSyntax(format!(
                "update hunk for {path} must include context or removed text"
            )));
        }
        let new = hunk
            .lines
            .iter()
            .filter(|line| line.kind != HunkLineKind::Remove)
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let matches = line_bounded_matches(&output, &old);
        if matches.len() != 1 {
            return Err(PatchError::Conflict(format!(
                "hunk for {path} matched {} locations instead of exactly one",
                matches.len()
            )));
        }
        output.replace_range(matches[0]..matches[0] + old.len(), &new);
    }
    if uses_crlf {
        Ok(output.replace('\n', "\r\n"))
    } else {
        Ok(output)
    }
}

fn line_bounded_matches(haystack: &str, needle: &str) -> Vec<usize> {
    haystack
        .match_indices(needle)
        .filter_map(|(index, _)| {
            let starts_on_line = index == 0 || haystack.as_bytes().get(index - 1) == Some(&b'\n');
            let end = index + needle.len();
            let ends_on_line =
                end == haystack.len() || haystack.as_bytes().get(end) == Some(&b'\n');
            (starts_on_line && ends_on_line).then_some(index)
        })
        .collect()
}

fn validate_unique_paths(operations: &[FilePatch]) -> Result<(), PatchError> {
    let mut paths = HashSet::new();
    for operation in operations {
        if !paths.insert(operation.path.clone()) {
            return Err(PatchError::InvalidSyntax(format!(
                "path {} appears more than once",
                operation.path
            )));
        }
        if let Some(destination) = &operation.move_to {
            if !paths.insert(destination.clone()) {
                return Err(PatchError::InvalidSyntax(format!(
                    "move destination {destination} overlaps another operation"
                )));
            }
        }
    }
    Ok(())
}

fn is_file_header(line: &str) -> bool {
    line.starts_with("*** Add File: ")
        || line.starts_with("*** Update File: ")
        || line.starts_with("*** Delete File: ")
        || line == "*** End Patch"
}

fn validate_relative_path(path: &str) -> Result<PathBuf, PatchError> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(PatchError::DeniedPath(
            "path must be a non-empty relative path".to_string(),
        ));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            _ => {
                return Err(PatchError::DeniedPath(format!(
                    "path {} contains traversal or a platform prefix",
                    path.display()
                )));
            }
        }
    }
    Ok(normalized)
}

fn canonical_workspace(root: &Path) -> Result<PathBuf, PatchError> {
    let root = root
        .canonicalize()
        .map_err(|error| PatchError::DeniedPath(error.to_string()))?;
    if !root.is_dir() {
        return Err(PatchError::DeniedPath(
            "workspace root is not a directory".to_string(),
        ));
    }
    Ok(root)
}

fn resolve_existing_file(root: &Path, relative: &Path) -> Result<PathBuf, PatchError> {
    reject_link_components(root, relative)?;
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|error| PatchError::Conflict(format!("{}: {error}", relative.display())))?;
    if !path.starts_with(root) {
        return Err(PatchError::DeniedPath(format!(
            "{} resolves outside the workspace",
            relative.display()
        )));
    }
    if !path.is_file() {
        return Err(PatchError::Conflict(format!(
            "{} is not a file",
            relative.display()
        )));
    }
    Ok(path)
}

fn resolve_new_path(root: &Path, relative: &Path) -> Result<PathBuf, PatchError> {
    reject_link_components(root, relative)?;
    let candidate = root.join(relative);
    let mut parent = candidate
        .parent()
        .ok_or_else(|| PatchError::DeniedPath(format!("{} has no parent", relative.display())))?;
    while !parent.exists() {
        parent = parent.parent().ok_or_else(|| {
            PatchError::DeniedPath(format!("{} has no existing parent", relative.display()))
        })?;
    }
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| PatchError::DeniedPath(error.to_string()))?;
    if !canonical_parent.starts_with(root) {
        return Err(PatchError::DeniedPath(format!(
            "{} resolves outside the workspace",
            relative.display()
        )));
    }
    Ok(candidate)
}

fn reject_link_components(root: &Path, relative: &Path) -> Result<(), PatchError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(PatchError::DeniedPath(format!(
                "{} contains an unsafe component",
                relative.display()
            )));
        };
        current.push(component);
        if current.exists() {
            let metadata = fs::symlink_metadata(&current)
                .map_err(|error| PatchError::Io(error.to_string()))?;
            if is_link_or_reparse_point(&metadata) {
                return Err(PatchError::DeniedPath(format!(
                    "{} traverses a symbolic link or directory junction",
                    relative.display()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(any(unix, windows)))]
fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn ensure_parent(
    path: &Path,
    root: &Path,
    created_directories: &mut Vec<PathBuf>,
) -> Result<(), PatchError> {
    let parent = path
        .parent()
        .ok_or_else(|| PatchError::DeniedPath("target has no parent".to_string()))?;
    let mut missing = Vec::new();
    let mut current = parent;
    while !current.exists() {
        missing.push(current.to_path_buf());
        current = current
            .parent()
            .ok_or_else(|| PatchError::DeniedPath("target parent is invalid".to_string()))?;
    }
    let canonical = current
        .canonicalize()
        .map_err(|error| PatchError::DeniedPath(error.to_string()))?;
    if !canonical.starts_with(root) {
        return Err(PatchError::DeniedPath(
            "target parent resolves outside the workspace".to_string(),
        ));
    }
    for directory in missing.iter().rev() {
        fs::create_dir(directory).map_err(|error| PatchError::Io(error.to_string()))?;
        created_directories.push(directory.clone());
    }
    Ok(())
}

fn read_text(path: &Path) -> Result<String, PatchError> {
    let bytes = fs::read(path).map_err(|error| PatchError::Io(error.to_string()))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(PatchError::Limit(format!(
            "{} is larger than {MAX_FILE_BYTES} bytes",
            path.display()
        )));
    }
    if bytes.iter().any(|byte| *byte == 0) {
        return Err(PatchError::Conflict(format!(
            "{} appears to be binary",
            path.display()
        )));
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| PatchError::Conflict(format!("{} is not UTF-8", path.display())))
}

fn write_text(path: &Path, content: &str) -> Result<(), PatchError> {
    fs::write(path, content.as_bytes()).map_err(|error| PatchError::Io(error.to_string()))
}

fn enforce_file_size(content: Option<&str>, path: &str) -> Result<(), PatchError> {
    if content.is_some_and(|content| content.len() > MAX_FILE_BYTES) {
        Err(PatchError::Limit(format!(
            "{path} is larger than {MAX_FILE_BYTES} bytes"
        )))
    } else {
        Ok(())
    }
}

fn content_from_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

fn normalize_newlines(content: &str) -> String {
    content.replace("\r\n", "\n")
}

fn hash_content(content: &str) -> String {
    Sha256::digest(content.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        })
}

fn unified_diff(old_path: &str, new_path: &str, before: &str, after: &str) -> String {
    TextDiff::from_lines(before, after)
        .unified_diff()
        .header(&format!("a/{old_path}"), &format!("b/{new_path}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected(preview: &PatchPreview) -> Vec<ExpectedFileHash> {
        preview
            .files
            .iter()
            .map(|file| ExpectedFileHash {
                path: file.path.clone(),
                before_hash: file.before_hash.clone(),
            })
            .collect()
    }

    #[test]
    fn parses_add_update_delete_and_move_operations_strictly() {
        let patch = "*** Begin Patch\n*** Add File: new.txt\n+new\n*** Update File: old.txt\n*** Move to: moved.txt\n@@\n-old\n+changed\n*** Delete File: remove.txt\n*** End Patch\n";
        let document = parse_patch(patch).unwrap();
        assert_eq!(document.operations.len(), 3);
        assert_eq!(document.operations[0].kind, FilePatchKind::Add);
        assert_eq!(document.operations[1].move_to.as_deref(), Some("moved.txt"));
        assert_eq!(document.operations[2].kind, FilePatchKind::Delete);

        assert!(matches!(
            parse_patch("*** Begin Patch\nplain text\n*** End Patch"),
            Err(PatchError::InvalidSyntax(_))
        ));
        assert!(matches!(
            parse_patch("*** Add File: a\n+x"),
            Err(PatchError::InvalidSyntax(_))
        ));
    }

    #[tokio::test]
    async fn previews_applies_and_undoes_all_file_operations() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("modify.txt"), "before\n").unwrap();
        fs::write(directory.path().join("delete.txt"), "delete me\n").unwrap();
        fs::write(directory.path().join("move.txt"), "move me\n").unwrap();
        let patch = "*** Begin Patch\n*** Add File: nested/new.txt\n+created\n*** Update File: modify.txt\n@@\n-before\n+after\n*** Delete File: delete.txt\n*** Update File: move.txt\n*** Move to: moved.txt\n@@\n-move me\n+moved and changed\n*** End Patch\n";
        let service = PatchService::new();
        let preview = service.preview_patch(directory.path(), patch).unwrap();
        assert_eq!(preview.files.len(), 4);
        assert!(
            preview
                .files
                .iter()
                .all(|file| !file.unified_diff.is_empty())
        );
        let change = service
            .apply_patch(
                directory.path().to_path_buf(),
                "thread".to_string(),
                "turn".to_string(),
                "call".to_string(),
                patch.to_string(),
                preview.files.iter().map(|file| file.path.clone()).collect(),
                expected(&preview),
            )
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(directory.path().join("modify.txt")).unwrap(),
            "after\n"
        );
        assert!(!directory.path().join("delete.txt").exists());
        assert!(!directory.path().join("move.txt").exists());
        assert_eq!(
            fs::read_to_string(directory.path().join("moved.txt")).unwrap(),
            "moved and changed\n"
        );

        let undone = service
            .undo(directory.path().to_path_buf(), change)
            .await
            .unwrap();
        assert!(undone.undone);
        assert_eq!(
            fs::read_to_string(directory.path().join("modify.txt")).unwrap(),
            "before\n"
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("delete.txt")).unwrap(),
            "delete me\n"
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("move.txt")).unwrap(),
            "move me\n"
        );
        assert!(!directory.path().join("moved.txt").exists());
        assert!(!directory.path().join("nested/new.txt").exists());

        let redone = service
            .redo(directory.path().to_path_buf(), undone)
            .await
            .unwrap();
        assert!(!redone.undone);
        assert_eq!(
            fs::read_to_string(directory.path().join("modify.txt")).unwrap(),
            "after\n"
        );
        assert!(!directory.path().join("delete.txt").exists());
        assert!(!directory.path().join("move.txt").exists());
        assert_eq!(
            fs::read_to_string(directory.path().join("moved.txt")).unwrap(),
            "moved and changed\n"
        );
        assert!(directory.path().join("nested/new.txt").exists());
    }

    #[tokio::test]
    async fn rejects_hash_conflicts_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "old\n").unwrap();
        let patch = "*** Begin Patch\n*** Update File: file.txt\n@@\n-old\n+new\n*** End Patch";
        let service = PatchService::new();
        let preview = service.preview_patch(directory.path(), patch).unwrap();
        fs::write(&path, "newer\n").unwrap();
        let result = service
            .apply_patch(
                directory.path().to_path_buf(),
                "thread".to_string(),
                "turn".to_string(),
                "call".to_string(),
                patch.to_string(),
                vec!["file.txt".to_string()],
                expected(&preview),
            )
            .await;
        assert!(matches!(result, Err(PatchError::Conflict(_))));
        assert_eq!(fs::read_to_string(path).unwrap(), "newer\n");
    }

    #[tokio::test]
    async fn rejects_duplicate_file_selection_and_review_hashes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "old\n").unwrap();
        let patch = "*** Begin Patch\n*** Update File: file.txt\n@@\n-old\n+new\n*** End Patch";
        let service = PatchService::new();
        let preview = service.preview_patch(directory.path(), patch).unwrap();
        let expected_hashes = expected(&preview);
        let duplicate_selection = service
            .apply_patch(
                directory.path().to_path_buf(),
                "thread".to_string(),
                "turn".to_string(),
                "call".to_string(),
                patch.to_string(),
                vec!["file.txt".to_string(), "file.txt".to_string()],
                expected_hashes.clone(),
            )
            .await;
        assert!(matches!(duplicate_selection, Err(PatchError::Conflict(_))));

        let duplicate_hashes = service
            .apply_patch(
                directory.path().to_path_buf(),
                "thread".to_string(),
                "turn".to_string(),
                "call".to_string(),
                patch.to_string(),
                vec!["file.txt".to_string()],
                vec![expected_hashes[0].clone(), expected_hashes[0].clone()],
            )
            .await;
        assert!(matches!(duplicate_hashes, Err(PatchError::Conflict(_))));
        assert_eq!(fs::read_to_string(path).unwrap(), "old\n");
    }

    #[tokio::test]
    async fn applies_only_the_reviewed_file_selection() {
        let directory = tempfile::tempdir().unwrap();
        let patch = "*** Begin Patch\n*** Add File: one.txt\n+one\n*** Add File: two.txt\n+two\n*** End Patch";
        let service = PatchService::new();
        let preview = service.preview_patch(directory.path(), patch).unwrap();
        let reviewed = preview
            .files
            .iter()
            .find(|file| file.path == "two.txt")
            .unwrap();
        let change = service
            .apply_patch(
                directory.path().to_path_buf(),
                "thread".to_string(),
                "turn".to_string(),
                "call".to_string(),
                patch.to_string(),
                vec![reviewed.path.clone()],
                vec![ExpectedFileHash {
                    path: reviewed.path.clone(),
                    before_hash: reviewed.before_hash.clone(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(change.files.len(), 1);
        assert!(!directory.path().join("one.txt").exists());
        assert_eq!(
            fs::read_to_string(directory.path().join("two.txt")).unwrap(),
            "two\n"
        );
    }

    #[test]
    fn transaction_failure_rolls_back_already_applied_files() {
        let directory = tempfile::tempdir().unwrap();
        let previews = vec![
            PatchFilePreview {
                path: "one.txt".to_string(),
                destination_path: None,
                operation: FileOperation::Add,
                before_hash: None,
                after_hash: Some(hash_content("one\n")),
                before_content: None,
                after_content: Some("one\n".to_string()),
                unified_diff: String::new(),
            },
            PatchFilePreview {
                path: "two.txt".to_string(),
                destination_path: None,
                operation: FileOperation::Add,
                before_hash: None,
                after_hash: Some(hash_content("two\n")),
                before_content: None,
                after_content: Some("two\n".to_string()),
                unified_diff: String::new(),
            },
        ];
        assert!(matches!(
            apply_previews(directory.path(), &previews, Some(1)),
            Err(PatchError::Io(_))
        ));
        assert!(!directory.path().join("one.txt").exists());
        assert!(!directory.path().join("two.txt").exists());
    }

    #[test]
    fn rejects_traversal_absolute_and_symlink_paths() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("outside.txt"), "secret").unwrap();
        let service = PatchService::new();
        for path in [
            "../outside.txt",
            outside.path().join("outside.txt").to_str().unwrap(),
        ] {
            let patch = format!("*** Begin Patch\n*** Add File: {path}\n+x\n*** End Patch");
            assert!(matches!(
                service.preview_patch(workspace.path(), &patch),
                Err(PatchError::DeniedPath(_))
            ));
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("escape")).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(outside.path(), workspace.path().join("escape"))
            .is_err()
        {
            return;
        }
        let patch = "*** Begin Patch\n*** Update File: escape/outside.txt\n@@\n-secret\n+leaked\n*** End Patch";
        assert!(matches!(
            service.preview_patch(workspace.path(), patch),
            Err(PatchError::DeniedPath(_))
        ));
    }
}
