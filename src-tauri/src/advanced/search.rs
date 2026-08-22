use std::fmt::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::protocol::{ToolDefinition, ToolResult};
use crate::tools::{ToolContext, ToolError, ToolHandler};

const MAX_INDEX_FILES: usize = 10_000;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_RESULTS: usize = 200;
const IGNORED: &[&str] = &[".git", "node_modules", "target", "dist", "build", ".next"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub preview: String,
    pub score: u32,
}

#[derive(Clone)]
pub struct RepositorySearchIndex {
    root: PathBuf,
}

impl RepositorySearchIndex {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let query = query.trim();
        if query.is_empty() || query.len() > 256 {
            return Err("search query must contain 1 to 256 characters".into());
        }
        let query_lower = query.to_lowercase();
        // 在 Windows / 跨盘符 / 工作区为符号连接时，canonicalize 经常直接失败并吞掉全部结果；
        // 这里退化到绝对路径，保证索引至少能在工作区内扫描到文件。
        let root = match self.root.canonicalize() {
            Ok(path) => path,
            Err(_) => absolutize(&self.root),
        };
        let terms = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|term| !term.is_empty())
            .collect::<Vec<_>>();
        if terms.is_empty() {
            return Err("search query must contain at least one non-whitespace term".into());
        }
        let mut files = Vec::new();
        collect_files(&root, &root, &mut files)?;
        let mut results = Vec::new();
        for path in files.into_iter().take(MAX_INDEX_FILES) {
            let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
            let Ok(content) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let relative = path
                .strip_prefix(&root)
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
            for (index, line) in content.lines().enumerate() {
                let lower = line.to_lowercase();
                if terms.iter().any(|term| lower.contains(term)) {
                    let matched = terms
                        .iter()
                        .filter(|term| lower.contains(term.as_str()))
                        .count() as u32;
                    let total = lower
                        .split(|ch: char| !ch.is_alphanumeric())
                        .filter(|token| {
                            !token.is_empty() && terms.iter().any(|term| *token == term.as_str())
                        })
                        .count() as u32;
                    // Exact symbol hits should outrank generated/minified files even when
                    // those files repeat the same token hundreds of times on one line.
                    let phrase_matches = lower.matches(&query_lower).count().min(3) as u32;
                    let mut score = matched * 100 + total.min(3) * 25 + phrase_matches * 20;
                    if is_low_signal_vendor_path(&relative) {
                        score = score.div_ceil(10);
                    }
                    let column = terms
                        .iter()
                        .filter_map(|term| lower.find(term.as_str()))
                        .min()
                        .unwrap_or(0)
                        + 1;
                    results.push(SearchResult {
                        path: relative.clone(),
                        line: index + 1,
                        column,
                        preview: bound(line.trim(), 320),
                        score,
                    });
                }
            }
        }
        results.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.line.cmp(&right.line))
        });
        results.truncate(limit.clamp(1, MAX_RESULTS));
        Ok(results)
    }
}

fn is_low_signal_vendor_path(path: &str) -> bool {
    let normalized = format!("/{}", path.replace('\\', "/").to_ascii_lowercase());
    normalized.contains("/vendor/")
        || normalized.contains("/vendors/")
        || normalized.contains("/wwwroot/lib/")
        || normalized.contains("/public/lib/")
        || normalized.contains("/third_party/")
        || normalized.contains("/third-party/")
        || normalized.ends_with(".min.js")
        || normalized.ends_with(".min.css")
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => {
            let mut resolved = cwd;
            for component in path.components() {
                match component {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        resolved.pop();
                    }
                    other => resolved.push(other.as_os_str()),
                }
            }
            resolved
        }
        Err(_) => path.to_path_buf(),
    }
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    if output.len() >= MAX_INDEX_FILES {
        return Ok(());
    }
    let mut entries = std::fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
    for entry in entries {
        if output.len() >= MAX_INDEX_FILES {
            break;
        }
        let name = entry.file_name();
        if IGNORED.contains(&name.to_string_lossy().as_ref()) {
            continue;
        }
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if metadata.is_dir() {
            let canonical = path.canonicalize().map_err(|error| error.to_string())?;
            if canonical.starts_with(root) {
                collect_files(root, &canonical, output)?;
            }
        } else if metadata.is_file() && metadata.len() <= MAX_FILE_BYTES && is_text_file(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn is_text_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "json"
            | "md"
            | "txt"
            | "toml"
            | "yaml"
            | "yml"
            | "xml"
            | "html"
            | "css"
            | "scss"
            | "sql"
            | "py"
            | "go"
            | "java"
            | "cs"
            | "cpp"
            | "c"
            | "h"
            | "hpp"
            | "sh"
            | "ps1"
            | "vue"
            | "svelte"
            | "lock"
    )
}

fn bound(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.into();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

pub struct SearchTool {
    index: RepositorySearchIndex,
}
impl SearchTool {
    pub fn new(index: RepositorySearchIndex) -> Self {
        Self { index }
    }
}

#[async_trait]
impl ToolHandler for SearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "search_repository".into(),
            description: "Search the workspace with a deterministic bounded lexical index.".into(),
            input_schema: json!({"type":"object","properties":{"query":{"type":"string","minLength":1,"maxLength":256},"limit":{"type":"integer","minimum":1,"maximum":200}},"required":["query"],"additionalProperties":false}),
        }
    }
    async fn execute(
        &self,
        _context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        #[derive(Deserialize)]
        struct Arguments {
            query: String,
            limit: Option<usize>,
        }
        let args: Arguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let index = self.index.clone();
        let query = args.query;
        let observation_query = query.trim().to_string();
        let limit = args.limit.unwrap_or(50);
        let results = tokio::task::spawn_blocking(move || index.search(&query, limit))
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?
            .map_err(ToolError::Execution)?;
        if cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let output = serde_json::to_string(&results)
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        let result_revision = sha256_hex(output.as_bytes());
        Ok(ToolResult {
            success: true,
            output,
            metadata: json!({
                "query": observation_query,
                "resultRevision": result_revision,
                "resultCount": results.len()
            }),
        })
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lexical_search_is_deterministic_and_ignores_build_output() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn alpha() {}\nalpha beta\n").unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/x.rs"), "alpha beta").unwrap();
        let index = RepositorySearchIndex::new(dir.path().to_path_buf());
        let results = index.search("alpha beta", 10).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].path, "a.rs");
        assert_eq!(results[0].line, 2);
        assert_eq!(results[1].path, "a.rs");
        assert_eq!(results[1].line, 1);
    }

    #[test]
    fn exact_source_symbol_outranks_repeated_minified_vendor_hits() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Permission.Business");
        let vendor = dir.path().join("wwwroot/lib");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&vendor).unwrap();
        std::fs::write(
            source.join("SsrRecordBll.cs"),
            "public CompanyConfig GetCompanyConfig() => config;\n",
        )
        .unwrap();
        std::fs::write(
            vendor.join("bundle.min.js"),
            "GetCompanyConfig ".repeat(100),
        )
        .unwrap();

        let results = RepositorySearchIndex::new(dir.path().to_path_buf())
            .search("GetCompanyConfig", 10)
            .unwrap();

        assert_eq!(results[0].path, "Permission.Business/SsrRecordBll.cs");
        assert!(results[0].score > results[1].score);
    }
}
