use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

const MAX_FILE_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMemoryFile {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMemoryWorkspace {
    pub id: String,
    pub label: String,
    pub files: Vec<ProjectMemoryFile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMemoryContent {
    pub workspace_id: String,
    pub path: String,
    pub content: String,
}

#[derive(Clone)]
pub struct ProjectMemoryStore {
    root: PathBuf,
}

impl ProjectMemoryStore {
    pub fn new(data_root: &Path) -> Self {
        Self {
            root: data_root.join("memories/projects"),
        }
    }

    pub fn list(&self) -> Result<Vec<ProjectMemoryWorkspace>, String> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        require_plain_directory(&self.root)?;
        let mut workspaces = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_plain_segment(&name) {
                continue;
            }
            let dir = entry.path();
            if !is_plain_directory(&dir)? {
                continue;
            }
            let mut files = Vec::new();
            let index = dir.join("MEMORY.md");
            if let Some(file) = list_markdown_file(&dir, &index, "MEMORY.md")? {
                files.push(file);
            }
            let memory_dir = dir.join("memory");
            if is_plain_directory(&memory_dir)? {
                for item in std::fs::read_dir(&memory_dir).map_err(|e| e.to_string())? {
                    let item = item.map_err(|e| e.to_string())?;
                    let file_name = item.file_name().to_string_lossy().into_owned();
                    if !file_name.ends_with(".md") || !is_plain_segment(&file_name) {
                        continue;
                    }
                    if let Some(file) = list_markdown_file(
                        &memory_dir,
                        &item.path(),
                        &format!("memory/{file_name}"),
                    )? {
                        files.push(file);
                    }
                }
            }
            files.sort_by(|a, b| a.path.cmp(&b.path));
            workspaces.push(ProjectMemoryWorkspace {
                id: name.clone(),
                label: workspace_label(&name),
                files,
            });
        }
        workspaces.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
        Ok(workspaces)
    }

    pub fn read(&self, workspace_id: &str, relative: &str) -> Result<ProjectMemoryContent, String> {
        if !is_plain_segment(workspace_id) {
            return Err("invalid memory workspace".into());
        }
        let workspace = self.root.join(workspace_id);
        require_plain_directory(&self.root)?;
        require_plain_directory(&workspace)?;
        let (parent, name) = if relative == "MEMORY.md" {
            (workspace.clone(), "MEMORY.md")
        } else if let Some(name) = relative.strip_prefix("memory/") {
            (workspace.join("memory"), name)
        } else {
            return Err("invalid memory file path".into());
        };
        if !is_plain_segment(name) || !name.ends_with(".md") {
            return Err("invalid memory file name".into());
        }
        require_plain_directory(&parent)?;
        let file = parent.join(name);
        let entries = std::fs::read_dir(&parent).map_err(|e| e.to_string())?;
        if !entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() == name)
        {
            return Err("memory file was not found".into());
        }
        let metadata = std::fs::symlink_metadata(&file).map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_FILE_BYTES
        {
            return Err(
                "memory file must be a regular Markdown file no larger than 256 KiB".into(),
            );
        }
        let content = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
        Ok(ProjectMemoryContent {
            workspace_id: workspace_id.into(),
            path: relative.into(),
            content,
        })
    }

    pub fn workspace_id(workspace: &Path) -> Result<String, String> {
        let canonical = workspace.canonicalize().map_err(|e| e.to_string())?;
        let label = canonical
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("workspace");
        let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
        Ok(format!("{}-{}", label, hex_prefix(&digest, 8)))
    }

    pub fn context_for_workspace(&self, workspace: &Path) -> Result<String, String> {
        let id = Self::workspace_id(workspace)?;
        let files = self
            .list()?
            .into_iter()
            .find(|item| item.id == id)
            .map(|item| item.files)
            .unwrap_or_default();
        let mut output = String::new();
        for file in files {
            let content = self.read(&id, &file.path)?.content;
            let section = format!("\n## {}\n{}\n", file.path, content);
            if output.len() + section.len() > 32 * 1024 {
                break;
            }
            output.push_str(&section);
        }
        Ok(output)
    }
}

fn is_plain_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\'])
        && !value.chars().any(char::is_control)
}
fn require_plain_directory(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("memory path must be a regular directory".into());
    }
    Ok(())
}
fn is_plain_directory(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_dir() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}
fn list_markdown_file(
    parent: &Path,
    path: &Path,
    relative: &str,
) -> Result<Option<ProjectMemoryFile>, String> {
    let entries = std::fs::read_dir(parent).map_err(|e| e.to_string())?;
    let name = path.file_name().unwrap_or_default();
    if !entries
        .filter_map(Result::ok)
        .any(|entry| entry.file_name() == name)
    {
        return Ok(None);
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_FILE_BYTES {
        return Ok(None);
    }
    Ok(Some(ProjectMemoryFile {
        name: name.to_string_lossy().into_owned(),
        path: relative.into(),
        size_bytes: metadata.len(),
    }))
}
fn workspace_label(value: &str) -> String {
    value
        .rsplit_once('-')
        .filter(|(_, hash)| hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|(label, _)| label)
        .unwrap_or(value)
        .to_string()
}
fn hex_prefix(bytes: &[u8], count: usize) -> String {
    bytes
        .iter()
        .take(count)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_reads_only_regular_markdown_files() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let id = ProjectMemoryStore::workspace_id(workspace.path()).unwrap();
        let dir = data.path().join("memories/projects").join(&id);
        std::fs::create_dir_all(dir.join("memory")).unwrap();
        std::fs::write(dir.join("MEMORY.md"), "# project").unwrap();
        std::fs::write(dir.join("memory/rules.md"), "Use Rust").unwrap();
        let store = ProjectMemoryStore::new(data.path());
        assert_eq!(store.list().unwrap()[0].files.len(), 2);
        assert_eq!(
            store.read(&id, "memory/rules.md").unwrap().content,
            "Use Rust"
        );
        assert!(store.read(&id, "../outside.md").is_err());
    }
}
