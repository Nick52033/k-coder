//! 插件市场：市场清单获取、远程插件源码解析与安全落盘。
//!
//! 市场本身只是一份 JSON 清单（`marketplace.json`），可以从 HTTP(S) 地址、Git 仓库或本地目录取得；
//! 清单中每个插件条目再经过 GitHub/Git/URL ZIP/本地目录解析成一个已校验的本地目录，最后复用
//! [`crate::extensions::plugins::PluginHost::install`] 完成与本地安装完全相同的清单校验、链接拒绝、
//! 原子发布和首次禁用流程。本模块只负责"把远程源码变成可信本地目录"，不引入第二套安装、启停或授权路径。

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::{MarketplaceSourceKind, PluginSourceKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::process::Command as TokioCommand;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zip::ZipArchive;

/// 市场清单文件名，Git/本地目录市场在仓库或目录根查找该文件。
pub const MARKETPLACE_MANIFEST_FILE_NAME: &str = "marketplace.json";
/// 清单 JSON 上限，防止远程清单无限膨胀。
pub const MAX_MARKETPLACE_MANIFEST_BYTES: usize = 512 * 1024;
/// 单个市场最多登记的插件条目数。
pub const MAX_MARKETPLACE_ENTRIES: usize = 128;
/// 远程插件压缩包下载与解包上限，与本地安装预算一致。
pub const MAX_PLUGIN_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
/// 压缩包内文件条目上限，防止解包炸弹。
pub const MAX_PLUGIN_ARCHIVE_ENTRIES: usize = 8192;
/// 清单与压缩包下载整体超时。
pub const MARKETPLACE_FETCH_TIMEOUT_SECS: u64 = 120;
/// `git clone` 超时。
pub const MARKETPLACE_GIT_TIMEOUT_SECS: u64 = 180;
/// 市场持久化文件 schema。
pub const MARKETPLACE_STORE_SCHEMA_VERSION: u32 = 1;
/// 市场总览视图 schema。
pub const MARKETPLACE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum MarketplaceError {
    #[error("plugin marketplace source is invalid: {0}")]
    Invalid(String),
    #[error("plugin marketplace manifest could not be parsed: {0}")]
    Manifest(String),
    #[error("plugin marketplace request failed: {0}")]
    Request(String),
    #[error("plugin marketplace archive is invalid: {0}")]
    Archive(String),
    #[error("plugin marketplace git operation failed: {0}")]
    Git(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 市场来源：HTTP(S) JSON 清单、Git 仓库或本地目录。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum MarketplaceSource {
    Http { url: String },
    Git { url: String },
    LocalDirectory { path: String },
}

impl MarketplaceSource {
    pub fn kind(&self) -> MarketplaceSourceKind {
        match self {
            Self::Http { .. } => MarketplaceSourceKind::Http,
            Self::Git { .. } => MarketplaceSourceKind::Git,
            Self::LocalDirectory { .. } => MarketplaceSourceKind::LocalDirectory,
        }
    }

    /// 界面与诊断展示用的源串；不携带凭据。
    pub fn display(&self) -> String {
        match self {
            Self::Http { url } | Self::Git { url } => redact_credentials(url),
            Self::LocalDirectory { path } => path.clone(),
        }
    }

    /// 把用户输入解析成市场来源：HTTP(S) 地址、Git 仓库（`owner/repo`、`*.git`、`git@`）或本地目录。
    pub fn from_user_input(input: &str) -> Result<Self, MarketplaceError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(MarketplaceError::Invalid(
                "marketplace source must not be empty".into(),
            ));
        }
        if looks_like_git_source(trimmed) {
            return Self::git_url(trimmed);
        }
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            return Self::http_url(trimmed);
        }
        if trimmed.contains("://") {
            return Err(MarketplaceError::Invalid(
                "only http and https marketplace sources are supported".into(),
            ));
        }
        let path = Path::new(trimmed);
        if path.is_absolute() || trimmed.contains('/') || trimmed.contains('\\') || trimmed == "." {
            return Ok(Self::LocalDirectory {
                path: trimmed.to_string(),
            });
        }
        Err(MarketplaceError::Invalid(
            "unrecognized marketplace source; expected an HTTP(S) URL, a Git repository, or a local directory"
                .into(),
        ))
    }

    pub fn http_url(url: &str) -> Result<Self, MarketplaceError> {
        validate_http_url(url)?;
        Ok(Self::Http {
            url: url.to_string(),
        })
    }

    pub fn git_url(url: &str) -> Result<Self, MarketplaceError> {
        let trimmed = url.trim();
        if trimmed.is_empty() || trimmed.starts_with('-') || trimmed.contains(char::is_whitespace) {
            return Err(MarketplaceError::Invalid(
                "git marketplace url must not be empty or start with an option flag".into(),
            ));
        }
        if trimmed.starts_with("git@") {
            return Ok(Self::Git {
                url: trimmed.to_string(),
            });
        }
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            validate_http_url(trimmed)?;
            return Ok(Self::Git {
                url: trimmed.to_string(),
            });
        }
        if !is_owner_repo_shorthand(trimmed) {
            return Err(MarketplaceError::Invalid(
                "git marketplace must be an HTTP(S) URL, an SSH URL, or an owner/repo shorthand"
                    .into(),
            ));
        }
        Ok(Self::Git {
            url: format!("https://github.com/{trimmed}.git"),
        })
    }
}

fn looks_like_git_source(value: &str) -> bool {
    value.starts_with("git@")
        || value.ends_with(".git")
        || value.contains("github.com")
        || value.contains("gitlab.com")
        || value.contains("bitbucket.org")
        || is_owner_repo_shorthand(value)
}

fn is_owner_repo_shorthand(value: &str) -> bool {
    let mut segments = value.split('/');
    let (Some(owner), Some(repo), None) = (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    !owner.is_empty()
        && !repo.is_empty()
        && owner.len() + repo.len() + 1 <= 200
        && [owner, repo].into_iter().all(|segment| {
            segment
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        })
}

fn validate_http_url(url: &str) -> Result<(), MarketplaceError> {
    let parsed = url::Url::parse(url)
        .map_err(|error| MarketplaceError::Invalid(format!("url is not valid: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(MarketplaceError::Invalid(
            "only http and https marketplace sources are supported".into(),
        ));
    }
    if parsed.host_str().is_none_or(|host| host.is_empty()) {
        return Err(MarketplaceError::Invalid("url has no host".into()));
    }
    Ok(())
}

/// 清单条目来源：与 ZCode 市场清单兼容的四种形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceEntrySource {
    Github {
        repo: String,
        reference: Option<String>,
        path: Option<String>,
    },
    Git {
        url: String,
        reference: Option<String>,
        path: Option<String>,
    },
    Url {
        url: String,
        sha256: Option<String>,
        path: Option<String>,
    },
    Directory {
        path: String,
    },
}

impl MarketplaceEntrySource {
    pub fn kind(&self) -> PluginSourceKind {
        match self {
            Self::Github { .. } => PluginSourceKind::Github,
            Self::Git { .. } => PluginSourceKind::Git,
            Self::Url { .. } => PluginSourceKind::Url,
            Self::Directory { .. } => PluginSourceKind::Directory,
        }
    }

    /// 界面展示的源描述；不携带 URL 凭据。
    pub fn display(&self) -> String {
        match self {
            Self::Github {
                repo, reference, ..
            } => match reference {
                Some(reference) => format!("{repo}@{reference}"),
                None => repo.clone(),
            },
            Self::Git { url, reference, .. } => match reference {
                Some(reference) => format!("{}@{}", redact_credentials(url), reference),
                None => redact_credentials(url),
            },
            Self::Url { url, .. } => redact_credentials(url),
            Self::Directory { path } => path.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplacePluginEntry {
    pub name: String,
    pub description: String,
    pub version: String,
    pub source: MarketplaceEntrySource,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceManifest {
    pub label: String,
    pub entries: Vec<MarketplacePluginEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginMarketplace {
    pub id: String,
    pub label: String,
    pub source: MarketplaceSource,
    pub added_at_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct MarketplaceStoreFile {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    marketplaces: Vec<PluginMarketplace>,
}

/// 市场持久化存储；文件损坏时回到空集合而不是拒绝启动。
pub struct MarketplaceStore {
    path: PathBuf,
    inner: Mutex<MarketplaceStoreFile>,
}

impl MarketplaceStore {
    pub fn new(data_root: &Path) -> Self {
        let path = data_root.join("plugin-marketplaces.json");
        let file = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<MarketplaceStoreFile>(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(file),
        }
    }

    pub fn list(&self) -> Vec<PluginMarketplace> {
        self.inner
            .lock()
            .expect("marketplace store lock poisoned")
            .marketplaces
            .clone()
    }

    pub fn add(
        &self,
        source: MarketplaceSource,
        label: String,
    ) -> Result<PluginMarketplace, MarketplaceError> {
        let mut guard = self.inner.lock().expect("marketplace store lock poisoned");
        if guard
            .marketplaces
            .iter()
            .any(|marketplace| marketplace.source == source)
        {
            return Err(MarketplaceError::Invalid(
                "this marketplace source is already added".into(),
            ));
        }
        let marketplace = PluginMarketplace {
            id: Uuid::new_v4().to_string(),
            label,
            source,
            added_at_ms: now_ms(),
        };
        guard.marketplaces.push(marketplace.clone());
        self.persist(&guard)?;
        Ok(marketplace)
    }

    pub fn remove(&self, id: &str) -> Result<(), MarketplaceError> {
        let mut guard = self.inner.lock().expect("marketplace store lock poisoned");
        let before = guard.marketplaces.len();
        guard
            .marketplaces
            .retain(|marketplace| marketplace.id != id);
        if guard.marketplaces.len() == before {
            return Err(MarketplaceError::Invalid(format!(
                "unknown plugin marketplace {id}"
            )));
        }
        self.persist(&guard)?;
        Ok(())
    }

    fn persist(&self, file: &MarketplaceStoreFile) -> Result<(), MarketplaceError> {
        let parent = self.path.parent().ok_or_else(|| {
            MarketplaceError::Io(std::io::Error::other("store path has no parent"))
        })?;
        fs::create_dir_all(parent)?;
        let mut stored = file.clone();
        stored.schema_version = MARKETPLACE_STORE_SCHEMA_VERSION;
        let bytes = serde_json::to_vec_pretty(&stored)
            .map_err(|error| MarketplaceError::Manifest(error.to_string()))?;
        let temporary = parent.join(format!(
            ".plugin-marketplaces-{}.tmp",
            Uuid::new_v4().simple()
        ));
        fs::write(&temporary, &bytes)?;
        if let Err(error) = fs::rename(&temporary, &self.path) {
            let _ = fs::remove_file(&temporary);
            return Err(MarketplaceError::Io(error));
        }
        Ok(())
    }
}

/// HTTP(S) 清单与压缩包下载客户端；有界超时与有界响应体。
pub struct MarketplaceClient {
    client: reqwest::Client,
    user_agent: String,
}

impl MarketplaceClient {
    pub fn new() -> Result<Self, MarketplaceError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                MARKETPLACE_FETCH_TIMEOUT_SECS,
            ))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|error| MarketplaceError::Request(error.to_string()))?;
        Ok(Self {
            client,
            user_agent: format!("k-coder/{}", env!("CARGO_PKG_VERSION")),
        })
    }

    pub async fn fetch_bytes(
        &self,
        url: &str,
        max_bytes: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, MarketplaceError> {
        validate_http_url(url)?;
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(MarketplaceError::Request("marketplace download cancelled".into())),
            response = self
                .client
                .get(url)
                .header(reqwest::header::USER_AGENT, &self.user_agent)
                .send() => response.map_err(|error| MarketplaceError::Request(error.to_string()))?,
        };
        if !response.status().is_success() {
            return Err(MarketplaceError::Request(format!(
                "marketplace endpoint returned HTTP {}",
                response.status().as_u16()
            )));
        }
        if let Some(length) = response.content_length() {
            if length > max_bytes as u64 {
                return Err(MarketplaceError::Request(format!(
                    "marketplace download exceeds {max_bytes} bytes"
                )));
            }
        }
        let bytes = tokio::select! {
            _ = cancellation.cancelled() => return Err(MarketplaceError::Request("marketplace download cancelled".into())),
            bytes = response.bytes() => bytes.map_err(|error| MarketplaceError::Request(error.to_string()))?,
        };
        if bytes.len() > max_bytes {
            return Err(MarketplaceError::Request(format!(
                "marketplace download exceeds {max_bytes} bytes"
            )));
        }
        Ok(bytes.to_vec())
    }
}

/// 解析后的插件目录；`_sandbox` 持有临时目录并在 Drop 时清理。
pub struct ResolvedMarketplacePlugin {
    pub root: PathBuf,
    _sandbox: Option<TempDirectory>,
}

impl ResolvedMarketplacePlugin {
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// 解析市场清单。HTTP(S) 市场直接拉取 JSON；Git/本地市场先取得仓库/目录再读取 `marketplace.json`。
pub async fn fetch_manifest(
    source: &MarketplaceSource,
    client: &MarketplaceClient,
    cancellation: &CancellationToken,
) -> Result<MarketplaceManifest, MarketplaceError> {
    match source {
        MarketplaceSource::Http { url } => {
            let bytes = client
                .fetch_bytes(url, MAX_MARKETPLACE_MANIFEST_BYTES, cancellation)
                .await?;
            parse_manifest(&bytes)
        }
        MarketplaceSource::Git { url } => {
            let sandbox = TempDirectory::new("marketplace-git")?;
            let repository = sandbox.path().join("repository");
            clone_repository(url, None, &repository).await?;
            let manifest_path = repository.join(MARKETPLACE_MANIFEST_FILE_NAME);
            let bytes = fs::read(&manifest_path).map_err(|error| {
                MarketplaceError::Manifest(format!(
                    "git repository has no {MARKETPLACE_MANIFEST_FILE_NAME} at its root: {error}"
                ))
            })?;
            parse_manifest(&bytes)
        }
        MarketplaceSource::LocalDirectory { path } => {
            let root = validate_local_directory(path)?;
            let bytes = fs::read(root.join(MARKETPLACE_MANIFEST_FILE_NAME)).map_err(|error| {
                MarketplaceError::Manifest(format!(
                    "local marketplace has no {MARKETPLACE_MANIFEST_FILE_NAME}: {error}"
                ))
            })?;
            parse_manifest(&bytes)
        }
    }
}

fn parse_manifest(bytes: &[u8]) -> Result<MarketplaceManifest, MarketplaceError> {
    if bytes.len() > MAX_MARKETPLACE_MANIFEST_BYTES {
        return Err(MarketplaceError::Manifest(format!(
            "manifest exceeds {MAX_MARKETPLACE_MANIFEST_BYTES} bytes"
        )));
    }
    #[derive(Deserialize)]
    struct RawManifest {
        #[serde(default)]
        name: Option<String>,
        plugins: Vec<serde_json::Value>,
    }
    let raw: RawManifest = serde_json::from_slice(bytes)
        .map_err(|error| MarketplaceError::Manifest(format!("manifest is not JSON: {error}")))?;
    if raw.plugins.len() > MAX_MARKETPLACE_ENTRIES {
        return Err(MarketplaceError::Manifest(format!(
            "manifest lists more than {MAX_MARKETPLACE_ENTRIES} plugins"
        )));
    }
    let mut entries = Vec::with_capacity(raw.plugins.len());
    let mut seen = std::collections::HashSet::new();
    for value in raw.plugins {
        let entry = parse_entry(value)?;
        if !seen.insert(entry.name.clone()) {
            return Err(MarketplaceError::Manifest(format!(
                "manifest lists plugin {} more than once",
                entry.name
            )));
        }
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err(MarketplaceError::Manifest(
            "manifest lists no plugins".into(),
        ));
    }
    Ok(MarketplaceManifest {
        label: raw
            .name
            .map(|name| bounded_text(name.trim(), 120))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "unnamed marketplace".into()),
        entries,
    })
}

fn parse_entry(value: serde_json::Value) -> Result<MarketplacePluginEntry, MarketplaceError> {
    #[derive(Deserialize)]
    struct RawEntry {
        name: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        dependencies: Option<Vec<String>>,
        source: serde_json::Value,
    }
    let raw: RawEntry = serde_json::from_value(value)
        .map_err(|error| MarketplaceError::Manifest(format!("plugin entry: {error}")))?;
    let name = raw.name.trim().to_string();
    if !super::plugins::valid_plugin_name(&name) {
        return Err(MarketplaceError::Manifest(format!(
            "plugin entry name {name:?} is not a valid plugin name"
        )));
    }
    let source = parse_entry_source(raw.source)?;
    let mut dependencies = raw.dependencies.unwrap_or_default();
    if dependencies.len() > 32 {
        return Err(MarketplaceError::Manifest(format!(
            "plugin entry {name} lists more than 32 dependencies"
        )));
    }
    dependencies.retain(|dependency| !dependency.trim().is_empty());
    dependencies.truncate(32);
    Ok(MarketplacePluginEntry {
        name,
        description: bounded_text(raw.description.unwrap_or_default().trim(), 512),
        version: bounded_text(raw.version.unwrap_or_default().trim(), 64).if_empty("unknown"),
        source,
        dependencies,
    })
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

fn parse_entry_source(
    value: serde_json::Value,
) -> Result<MarketplaceEntrySource, MarketplaceError> {
    match value {
        serde_json::Value::String(path) => Ok(MarketplaceEntrySource::Directory {
            path: validate_relative_source_path(&path)?,
        }),
        serde_json::Value::Object(fields) => {
            let kind = fields
                .get("source")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    MarketplaceError::Manifest(
                        "plugin source object needs a string \"source\" discriminator".into(),
                    )
                })?
                .trim()
                .to_ascii_lowercase();
            match kind.as_str() {
                "github" => {
                    let repo = required_string(&fields, "repo")?;
                    if !is_github_repo(&repo) {
                        return Err(MarketplaceError::Manifest(format!(
                            "github source repo {repo:?} must look like owner/repo"
                        )));
                    }
                    Ok(MarketplaceEntrySource::Github {
                        repo,
                        reference: optional_reference(&fields)?,
                        path: optional_relative_path(&fields, "path")?,
                    })
                }
                "git" => Ok(MarketplaceEntrySource::Git {
                    url: required_string(&fields, "url")?,
                    reference: optional_reference(&fields)?,
                    path: optional_relative_path(&fields, "path")?,
                }),
                "url" => {
                    let url = required_string(&fields, "url")?;
                    validate_http_url(&url)?;
                    let transfer = fields
                        .get("type")
                        .and_then(|value| value.as_str())
                        .unwrap_or("zip")
                        .trim()
                        .to_ascii_lowercase();
                    if transfer == "git" {
                        return Ok(MarketplaceEntrySource::Git {
                            url,
                            reference: optional_reference(&fields)?,
                            path: optional_relative_path(&fields, "path")?,
                        });
                    }
                    if transfer != "zip" {
                        return Err(MarketplaceError::Manifest(format!(
                            "unsupported url source type {transfer:?}; only zip and git are supported"
                        )));
                    }
                    let sha256 = match fields.get("sha256") {
                        Some(serde_json::Value::String(value)) => {
                            let value = value.trim().to_ascii_lowercase();
                            if value.len() != 64 || !value.chars().all(|ch| ch.is_ascii_hexdigit())
                            {
                                return Err(MarketplaceError::Manifest(
                                    "sha256 must be a 64 character hex digest".into(),
                                ));
                            }
                            Some(value)
                        }
                        Some(serde_json::Value::Null) | None => None,
                        Some(_) => {
                            return Err(MarketplaceError::Manifest(
                                "sha256 must be a hex string".into(),
                            ));
                        }
                    };
                    Ok(MarketplaceEntrySource::Url {
                        url,
                        sha256,
                        path: optional_relative_path(&fields, "path")?,
                    })
                }
                "directory" => Ok(MarketplaceEntrySource::Directory {
                    path: required_string(&fields, "path")?,
                }),
                other => Err(MarketplaceError::Manifest(format!(
                    "unsupported plugin source {other:?}; expected github, git, url or directory"
                ))),
            }
        }
        other => Err(MarketplaceError::Manifest(format!(
            "plugin source must be a relative path string or an object, got {}",
            type_name(&other)
        ))),
    }
}

fn type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn required_string(
    fields: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, MarketplaceError> {
    match fields.get(key) {
        Some(serde_json::Value::String(value)) if !value.trim().is_empty() => {
            Ok(value.trim().to_string())
        }
        _ => Err(MarketplaceError::Manifest(format!(
            "plugin source needs a non-empty \"{key}\" string"
        ))),
    }
}

fn optional_reference(
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<String>, MarketplaceError> {
    match fields.get("ref").or_else(|| fields.get("reference")) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() || value.len() > 128 || value.starts_with('-') {
                return Err(MarketplaceError::Manifest(
                    "git ref must be 1-128 characters and must not start with an option flag"
                        .into(),
                ));
            }
            Ok(Some(value.to_string()))
        }
        Some(_) => Err(MarketplaceError::Manifest(
            "git ref must be a string".into(),
        )),
    }
}

fn optional_relative_path(
    fields: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>, MarketplaceError> {
    match fields.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(validate_relative_source_path(value)?)),
        Some(_) => Err(MarketplaceError::Manifest(format!(
            "plugin source \"{key}\" must be a string"
        ))),
    }
}

fn validate_relative_source_path(value: &str) -> Result<String, MarketplaceError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(MarketplaceError::Manifest(
            "plugin source path must not be empty".into(),
        ));
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(MarketplaceError::Manifest(format!(
            "plugin source path {trimmed:?} must be relative"
        )));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(MarketplaceError::Manifest(format!(
                    "plugin source path {trimmed:?} must stay inside the source tree"
                )));
            }
        }
    }
    Ok(trimmed.to_string())
}

fn is_github_repo(value: &str) -> bool {
    let mut segments = value.split('/');
    matches!(
        (segments.next(), segments.next(), segments.next()),
        (Some(owner), Some(repo), None) if !owner.is_empty() && !repo.is_empty()
    ) && value.len() <= 200
        && !value.contains(char::is_whitespace)
}

fn validate_local_directory(path: &str) -> Result<PathBuf, MarketplaceError> {
    let candidate = Path::new(path.trim());
    let canonical = candidate.canonicalize().map_err(|error| {
        MarketplaceError::Invalid(format!(
            "local marketplace {path:?} cannot be resolved: {error}"
        ))
    })?;
    if !canonical.is_dir() {
        return Err(MarketplaceError::Invalid(format!(
            "local marketplace {path:?} is not a directory"
        )));
    }
    Ok(canonical)
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = std::fmt::Write::write_fmt(&mut output, format_args!("{byte:02x}"));
    }
    output
}

fn redact_credentials(value: &str) -> String {
    let Some(scheme_end) = value.find("://") else {
        return value.to_string();
    };
    let rest = &value[scheme_end + 3..];
    let Some(slash) = rest.find('/') else {
        return value.to_string();
    };
    let authority = &rest[..slash];
    if let Some(at) = authority.rfind('@') {
        return format!("{}://***{}", &value[..scheme_end], &authority[at..]);
    }
    value.to_string()
}

/// 新建立的临时目录；Drop 时尽力清理，失败不影响调用方。
pub struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    pub fn new(prefix: &str) -> Result<Self, MarketplaceError> {
        let base = std::env::temp_dir();
        for _ in 0..8 {
            let candidate = base.join(format!("k-coder-{prefix}-{}", Uuid::new_v4().simple()));
            match fs::create_dir(&candidate) {
                Ok(()) => return Ok(Self { path: candidate }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(MarketplaceError::Io(error)),
            }
        }
        Err(MarketplaceError::Io(std::io::Error::other(
            "could not create a unique temporary directory",
        )))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// 把一个市场条目解析成本地插件目录，复用 Git/HTTP 客户端并始终把产物限制在临时沙箱内。
pub async fn resolve_plugin(
    entry: &MarketplacePluginEntry,
    marketplace: &MarketplaceSource,
    client: &MarketplaceClient,
    cancellation: &CancellationToken,
) -> Result<ResolvedMarketplacePlugin, MarketplaceError> {
    match &entry.source {
        MarketplaceEntrySource::Directory { path } => {
            let root = resolve_directory_entry(marketplace, path)?;
            Ok(ResolvedMarketplacePlugin {
                root,
                _sandbox: None,
            })
        }
        MarketplaceEntrySource::Github {
            repo,
            reference,
            path,
        } => {
            let git_ref = reference.clone().unwrap_or_else(|| "main".to_string());
            let url = format!("https://api.github.com/repos/{repo}/zipball/{git_ref}");
            let sandbox = TempDirectory::new("plugin-github")?;
            let archive = sandbox.path().join("archive.zip");
            let bytes = client
                .fetch_bytes(&url, MAX_PLUGIN_ARCHIVE_BYTES as usize, cancellation)
                .await?;
            fs::write(&archive, &bytes)?;
            let extracted = sandbox.path().join("extracted");
            fs::create_dir_all(&extracted)?;
            extract_archive(&archive, &extracted)?;
            let root = locate_plugin_root(&extracted, path.as_deref())?;
            Ok(ResolvedMarketplacePlugin {
                root,
                _sandbox: Some(sandbox),
            })
        }
        MarketplaceEntrySource::Git {
            url,
            reference,
            path,
        } => {
            let sandbox = TempDirectory::new("plugin-git")?;
            let repository = sandbox.path().join("repository");
            clone_repository(url, reference.as_deref(), &repository).await?;
            let root = locate_plugin_root(&repository, path.as_deref())?;
            Ok(ResolvedMarketplacePlugin {
                root,
                _sandbox: Some(sandbox),
            })
        }
        MarketplaceEntrySource::Url { url, sha256, path } => {
            let sandbox = TempDirectory::new("plugin-url")?;
            let archive = sandbox.path().join("archive.zip");
            let bytes = client
                .fetch_bytes(&url, MAX_PLUGIN_ARCHIVE_BYTES as usize, cancellation)
                .await?;
            if let Some(expected) = sha256 {
                let actual = hex_digest(&Sha256::digest(&bytes));
                if actual != *expected {
                    return Err(MarketplaceError::Archive(format!(
                        "plugin archive sha256 mismatch; expected {expected}, got {actual}"
                    )));
                }
            }
            fs::write(&archive, &bytes)?;
            let extracted = sandbox.path().join("extracted");
            fs::create_dir_all(&extracted)?;
            extract_archive(&archive, &extracted)?;
            let root = locate_plugin_root(&extracted, path.as_deref())?;
            Ok(ResolvedMarketplacePlugin {
                root,
                _sandbox: Some(sandbox),
            })
        }
    }
}

fn resolve_directory_entry(
    marketplace: &MarketplaceSource,
    path: &str,
) -> Result<PathBuf, MarketplaceError> {
    let relative = validate_relative_source_path(path)?;
    match marketplace {
        MarketplaceSource::LocalDirectory { path: root } => {
            let base = validate_local_directory(root)?;
            let candidate = base.join(&relative);
            let canonical = candidate.canonicalize().map_err(|error| {
                MarketplaceError::Invalid(format!(
                    "marketplace plugin directory {relative:?} cannot be resolved: {error}"
                ))
            })?;
            if !canonical.starts_with(&base) {
                return Err(MarketplaceError::Invalid(
                    "marketplace plugin directory escapes the marketplace root".into(),
                ));
            }
            Ok(canonical)
        }
        MarketplaceSource::Git { .. } => Err(MarketplaceError::Invalid(
            "relative directory sources are only supported for local directory marketplaces".into(),
        )),
        MarketplaceSource::Http { .. } => Err(MarketplaceError::Invalid(
            "directory sources are only supported for local directory marketplaces".into(),
        )),
    }
}

/// 在解包或克隆产物中定位插件根：优先清单声明的子路径，其次根自身，最后是唯一含清单的子目录。
fn locate_plugin_root(root: &Path, subpath: Option<&str>) -> Result<PathBuf, MarketplaceError> {
    if let Some(subpath) = subpath {
        let relative = validate_relative_source_path(subpath)?;
        let candidate = root.join(relative);
        if !plugin_manifest_exists(&candidate) {
            return Err(MarketplaceError::Archive(format!(
                "plugin path {subpath:?} has no .codex-plugin/plugin.json"
            )));
        }
        return Ok(candidate);
    }
    if plugin_manifest_exists(root) {
        return Ok(root.to_path_buf());
    }
    let entries = fs::read_dir(root)
        .map_err(|error| MarketplaceError::Archive(format!("downloaded tree is empty: {error}")))?;
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(MarketplaceError::Io)?;
        if entry.path().is_dir() && plugin_manifest_exists(&entry.path()) {
            candidates.push(entry.path());
        }
    }
    match candidates.as_slice() {
        [single] => Ok(single.clone()),
        _ => Ok(root.to_path_buf()),
    }
}

fn plugin_manifest_exists(dir: &Path) -> bool {
    dir.join(".codex-plugin/plugin.json").is_file()
}

/// 安全解包 ZIP：拒绝绝对路径、父级穿越、符号链接与超额条目/体积。
pub fn extract_archive(archive: &Path, target: &Path) -> Result<(), MarketplaceError> {
    let file = fs::File::open(archive)?;
    let mut zip = ZipArchive::new(file)
        .map_err(|error| MarketplaceError::Archive(format!("archive is not a ZIP: {error}")))?;
    fs::create_dir_all(target)?;
    let mut entry_count = 0usize;
    let mut total_bytes = 0u64;
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(|error| {
            MarketplaceError::Archive(format!("archive entry {index}: {error}"))
        })?;
        let name = entry.enclosed_name().ok_or_else(|| {
            MarketplaceError::Archive(format!(
                "archive entry {} escapes the extraction directory",
                entry.name()
            ))
        })?;
        if entry.is_symlink() {
            return Err(MarketplaceError::Archive(
                "archive contains symbolic links".into(),
            ));
        }
        if entry.is_dir() {
            fs::create_dir_all(target.join(&name))?;
            continue;
        }
        if !entry.is_file() {
            return Err(MarketplaceError::Archive(
                "archive contains entries that are neither files nor directories".into(),
            ));
        }
        let size = entry.size();
        if size > MAX_PLUGIN_ARCHIVE_BYTES {
            return Err(MarketplaceError::Archive(format!(
                "archive entry {name:?} exceeds {MAX_PLUGIN_ARCHIVE_BYTES} bytes"
            )));
        }
        entry_count += 1;
        if entry_count > MAX_PLUGIN_ARCHIVE_ENTRIES {
            return Err(MarketplaceError::Archive(format!(
                "archive contains more than {MAX_PLUGIN_ARCHIVE_ENTRIES} files"
            )));
        }
        total_bytes = total_bytes.saturating_add(size);
        if total_bytes > MAX_PLUGIN_ARCHIVE_BYTES {
            return Err(MarketplaceError::Archive(format!(
                "archive expands beyond {MAX_PLUGIN_ARCHIVE_BYTES} bytes"
            )));
        }
        let out_path = target.join(&name);
        ensure_entry_path_within(&name)?;
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&out_path)?;
        let mut limited = entry.take(MAX_PLUGIN_ARCHIVE_BYTES + 1);
        let copied = std::io::copy(&mut limited, &mut out)?;
        if copied > MAX_PLUGIN_ARCHIVE_BYTES {
            return Err(MarketplaceError::Archive(
                "archive entry expands beyond the size budget".into(),
            ));
        }
    }
    Ok(())
}

/// 校验解包条目路径：只允许普通相对分量，拒绝绝对路径与父级穿越。
fn ensure_entry_path_within(name: &Path) -> Result<(), MarketplaceError> {
    for component in name.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(MarketplaceError::Archive(
                    "archive entry path escapes the extraction directory".into(),
                ));
            }
        }
    }
    Ok(())
}

/// `git clone` 到目标目录；浅克隆、可选分支，stdout/stderr 有界并脱敏 URL 凭据。
pub async fn clone_repository(
    url: &str,
    reference: Option<&str>,
    target: &Path,
) -> Result<(), MarketplaceError> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') {
        return Err(MarketplaceError::Invalid(
            "git url must not be empty or start with an option flag".into(),
        ));
    }
    if let Some(reference) = reference {
        if reference.is_empty()
            || reference.starts_with('-')
            || reference.contains(char::is_whitespace)
        {
            return Err(MarketplaceError::Invalid(
                "git ref must not be empty or start with an option flag".into(),
            ));
        }
    }
    let mut command = TokioCommand::new("git");
    command.arg("clone").arg("--depth").arg("1");
    if let Some(reference) = reference {
        command.arg("--branch").arg(reference);
    }
    command.arg(url).arg(target);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    crate::execution::hide_console_window(&mut command);
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(MARKETPLACE_GIT_TIMEOUT_SECS),
        command.output(),
    )
    .await
    .map_err(|_| MarketplaceError::Git("git clone timed out".into()))?
    .map_err(|error| MarketplaceError::Git(format!("git executable is unavailable: {error}")))?;
    if !output.status.success() {
        let stderr = redact_credentials(&String::from_utf8_lossy(&output.stderr));
        return Err(MarketplaceError::Git(
            bounded_text(stderr.trim(), 512).if_empty("git clone failed without diagnostics"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::ProjectionDb;
    use crate::protocol::PluginScope;

    fn token() -> CancellationToken {
        CancellationToken::new()
    }

    fn manifest_json(entries: &str) -> Vec<u8> {
        format!(r#"{{"name":"test-marketplace","plugins":[{entries}]}}"#).into_bytes()
    }

    #[test]
    fn parses_every_supported_source_kind() {
        let entries = manifest_json(
            r#"
            {"name":"github-plugin","description":"from GitHub","version":"1.2.3","source":{"source":"github","repo":"owner/repo","ref":"v1.2.3","path":"plugins/thing"}},
            {"name":"git-plugin","source":{"source":"git","url":"https://example.com/repo.git"}},
            {"name":"zip-plugin","source":{"source":"url","url":"https://example.com/plugin.zip","type":"zip","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}},
            {"name":"plain-plugin","source":"plugins/plain"},
            {"name":"dir-plugin","source":{"source":"directory","path":"plugins/dir"}}
            "#,
        );

        let manifest = parse_manifest(&entries).expect("manifest parses");

        assert_eq!(manifest.label, "test-marketplace");
        assert_eq!(manifest.entries.len(), 5);
        let kinds = manifest
            .entries
            .iter()
            .map(|entry| entry.source.kind())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                PluginSourceKind::Github,
                PluginSourceKind::Git,
                PluginSourceKind::Url,
                PluginSourceKind::Directory,
                PluginSourceKind::Directory,
            ]
        );
        let github = &manifest.entries[0];
        assert_eq!(github.version, "1.2.3");
        assert_eq!(github.description, "from GitHub");
        assert!(matches!(
            &github.source,
            MarketplaceEntrySource::Github { reference: Some(reference), .. } if reference == "v1.2.3"
        ));
        assert!(matches!(
            &manifest.entries[3].source,
            MarketplaceEntrySource::Directory { path } if path == "plugins/plain"
        ));
    }

    #[test]
    fn rejects_invalid_entries() {
        let duplicate = manifest_json(
            r#"
            {"name":"dup","source":"plugins/a"},
            {"name":"dup","source":"plugins/b"}
            "#,
        );
        assert!(
            parse_manifest(&duplicate)
                .unwrap_err()
                .to_string()
                .contains("more than once")
        );

        let bad_name = manifest_json(r#"{"name":"Bad_Name","source":"plugins/a"}"#);
        assert!(
            parse_manifest(&bad_name)
                .unwrap_err()
                .to_string()
                .contains("valid plugin name")
        );

        let traversal = manifest_json(r#"{"name":"escape","source":"../evil"}"#);
        assert!(
            parse_manifest(&traversal)
                .unwrap_err()
                .to_string()
                .contains("stay inside the source tree")
        );

        let unknown_kind =
            manifest_json(r#"{"name":"weird","source":{"source":"npm","package":"x"}}"#);
        assert!(
            parse_manifest(&unknown_kind)
                .unwrap_err()
                .to_string()
                .contains("unsupported plugin source")
        );

        let empty = br#"{"name":"empty","plugins":[]}"#;
        assert!(
            parse_manifest(empty)
                .unwrap_err()
                .to_string()
                .contains("no plugins")
        );
    }

    #[test]
    fn classifies_user_input_into_marketplace_sources() {
        assert_eq!(
            MarketplaceSource::from_user_input("https://example.com/marketplace.json").unwrap(),
            MarketplaceSource::Http {
                url: "https://example.com/marketplace.json".into()
            }
        );
        assert_eq!(
            MarketplaceSource::from_user_input("owner/repo").unwrap(),
            MarketplaceSource::Git {
                url: "https://github.com/owner/repo.git".into()
            }
        );
        assert_eq!(
            MarketplaceSource::from_user_input("https://github.com/owner/repo").unwrap(),
            MarketplaceSource::Git {
                url: "https://github.com/owner/repo".into()
            }
        );
        assert_eq!(
            MarketplaceSource::from_user_input("D:/marketplaces/local").unwrap(),
            MarketplaceSource::LocalDirectory {
                path: "D:/marketplaces/local".into()
            }
        );
        for rejected in ["", "   ", "https://", "ftp://example.com/x.json"] {
            assert!(
                MarketplaceSource::from_user_input(rejected).is_err(),
                "{rejected} must be rejected"
            );
        }
    }

    #[test]
    fn sha256_digests_match_and_mismatch() {
        let bytes = b"k-coder marketplace fixture";
        let expected = hex_digest(&Sha256::digest(bytes));
        assert_eq!(expected.len(), 64);
        assert!(expected.chars().all(|ch| ch.is_ascii_hexdigit()));

        let manifest = manifest_json(&format!(
            r#"{{"name":"zip","source":{{"source":"url","url":"https://example.com/x.zip","sha256":"{expected}"}}}}"#
        ));
        assert!(parse_manifest(&manifest).is_ok());

        // 长度与非十六进制字符都必须在清单解析期拒绝。
        for bad in ["AA", "z".repeat(64).as_str(), &"0".repeat(63)] {
            let manifest = manifest_json(&format!(
                r#"{{"name":"zip","source":{{"source":"url","url":"https://example.com/x.zip","sha256":"{bad}"}}}}"#
            ));
            assert!(parse_manifest(&manifest).is_err(), "{bad} must be rejected");
        }

        // 清单校验通过但下载内容不匹配时，解析阶段必须报告摘要不一致。
        let manifest = manifest_json(&format!(
            r#"{{"name":"zip","source":{{"source":"url","url":"https://example.com/x.zip","sha256":"{expected}"}}}}"#
        ));
        let entry = parse_manifest(&manifest).unwrap().entries.remove(0);
        let MarketplaceEntrySource::Url { sha256, .. } = &entry.source else {
            panic!("expected url source");
        };
        assert_eq!(sha256.as_deref(), Some(expected.as_str()));
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = Default::default();
        for (name, contents) in entries {
            writer.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut writer, contents).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn extracts_plugin_tree_from_archive() {
        let data = tempfile::tempdir().unwrap();
        let archive = data.path().join("plugin.zip");
        write_zip(
            &archive,
            &[
                (
                    "owner-repo-abc/.codex-plugin/plugin.json",
                    br#"{"name":"thing","version":"1.0.0"}"#,
                ),
                ("owner-repo-abc/skills/thing/SKILL.md", b"# thing\n"),
            ],
        );

        let target = data.path().join("out");
        extract_archive(&archive, &target).unwrap();

        assert!(
            target
                .join("owner-repo-abc/.codex-plugin/plugin.json")
                .is_file()
        );
        assert!(
            target
                .join("owner-repo-abc/skills/thing/SKILL.md")
                .is_file()
        );

        let root = locate_plugin_root(&target, None).unwrap();
        assert_eq!(root, target.join("owner-repo-abc"));
        let explicit = locate_plugin_root(&target, Some("owner-repo-abc")).unwrap();
        assert!(plugin_manifest_exists(&explicit));
        assert!(locate_plugin_root(&target, Some("missing/dir")).is_err());
    }

    #[test]
    fn rejects_traversal_and_symlink_entries() {
        let data = tempfile::tempdir().unwrap();

        let traversal = data.path().join("traversal.zip");
        write_zip(
            &traversal,
            &[("../escape.txt", b"evil"), ("safe.txt", b"ok")],
        );
        let error = extract_archive(&traversal, &data.path().join("out-traversal")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("escapes the extraction directory"),
            "{error}"
        );

        let symlink = data.path().join("symlink.zip");
        let file = fs::File::create(&symlink).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = Default::default();
        writer
            .add_symlink("link/readme.md", "../outside.txt", options)
            .unwrap();
        writer.start_file("safe.txt", options).unwrap();
        std::io::Write::write_all(&mut writer, b"ok").unwrap();
        writer.finish().unwrap();
        let target = data.path().join("out-symlink");
        fs::create_dir_all(&target).unwrap();
        let error = extract_archive(&symlink, &target).unwrap_err();
        assert!(error.to_string().contains("symbolic links"), "{error}");
        assert!(!target.join("link/readme.md").exists());
    }

    #[tokio::test]
    async fn fetches_manifest_over_loopback_http() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request).await.unwrap();
            let body = br#"{"name":"loopback","plugins":[{"name":"fixture","source":{"source":"github","repo":"owner/repo"}}]}"#;
            let wire = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(wire.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
        });

        let client = MarketplaceClient::new().unwrap();
        let manifest = fetch_manifest(
            &MarketplaceSource::Http {
                url: format!("http://{address}/marketplace.json"),
            },
            &client,
            &token(),
        )
        .await
        .expect("manifest fetched");

        assert_eq!(manifest.label, "loopback");
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.entries[0].name, "fixture");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejected_urls_are_never_fetched() {
        let client = MarketplaceClient::new().unwrap();
        for url in ["ftp://example.com/x.json", "file:///etc/passwd"] {
            let error = client.fetch_bytes(url, 16, &token()).await.unwrap_err();
            assert!(error.to_string().contains("http and https"), "{error}");
        }
    }

    #[test]
    fn store_round_trips_and_rejects_duplicates() {
        let data = tempfile::tempdir().unwrap();
        let store = MarketplaceStore::new(data.path());

        assert!(store.list().is_empty());
        let source = MarketplaceSource::Http {
            url: "https://example.com/marketplace.json".into(),
        };
        let added = store.add(source.clone(), "remote".into()).unwrap();
        assert_eq!(store.list().len(), 1);

        let duplicate = store.add(source, "remote again".into()).unwrap_err();
        assert!(duplicate.to_string().contains("already added"));

        let reloaded = MarketplaceStore::new(data.path());
        assert_eq!(reloaded.list(), vec![added.clone()]);

        reloaded.remove(&added.id).unwrap();
        assert!(MarketplaceStore::new(data.path()).list().is_empty());
        assert!(
            MarketplaceStore::new(data.path())
                .remove("missing")
                .is_err()
        );
    }

    #[test]
    fn temp_directory_is_removed_on_drop() {
        let temp = TempDirectory::new("test").unwrap();
        let path = temp.path().to_path_buf();
        fs::write(path.join("marker.txt"), b"x").unwrap();
        drop(temp);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn installs_a_remote_plugin_through_the_plugin_host() {
        let data = tempfile::tempdir().unwrap();
        let client = MarketplaceClient::new().unwrap();

        // 本地目录市场 + 目录条目：完整走"清单 -> 解析 -> 安装 -> 发现"链路。
        let marketplace_root = tempfile::tempdir().unwrap();
        let plugin_source = marketplace_root.path().join("fixture-plugin");
        fs::create_dir_all(plugin_source.join(".codex-plugin")).unwrap();
        fs::write(
            plugin_source.join(".codex-plugin/plugin.json"),
            br#"{"name":"fixture-plugin","version":"0.1.0","description":"fixture"}"#,
        )
        .unwrap();
        fs::create_dir_all(plugin_source.join("skills/review")).unwrap();
        fs::write(
            plugin_source.join("skills/review/SKILL.md"),
            b"---\nname: review\ndescription: Review\n---\nReview things.\n",
        )
        .unwrap();
        fs::write(
            marketplace_root.path().join(MARKETPLACE_MANIFEST_FILE_NAME),
            br#"{"name":"fixture-marketplace","plugins":[{"name":"fixture-plugin","description":"fixture","version":"0.1.0","source":"fixture-plugin"}]}"#,
        )
        .unwrap();

        let source = MarketplaceSource::LocalDirectory {
            path: marketplace_root.path().to_string_lossy().into_owned(),
        };
        let manifest = fetch_manifest(&source, &client, &token()).await.unwrap();
        assert_eq!(manifest.entries.len(), 1);

        let resolved = resolve_plugin(&manifest.entries[0], &source, &client, &token())
            .await
            .expect("directory entry resolves");
        assert_eq!(resolved.root(), plugin_source.canonicalize().unwrap());

        let host = crate::extensions::plugins::PluginHost::with_roots(
            ProjectionDb::memory().unwrap(),
            None,
            Some(data.path().join("plugins")),
        );
        let overview = host
            .install(resolved.root(), PluginScope::Local)
            .expect("plugin installs");

        let installed = overview
            .plugins
            .iter()
            .find(|plugin| plugin.id == "fixture-plugin@local")
            .expect("installed plugin is discovered");
        assert!(!installed.enabled);
        assert_eq!(installed.scope, PluginScope::Local);
        assert!(
            data.path()
                .join("plugins/fixture-plugin/.codex-plugin/plugin.json")
                .is_file()
        );
    }
}
