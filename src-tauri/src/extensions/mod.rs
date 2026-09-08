pub mod hooks;
pub mod mcp;
pub mod plugins;

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use serde::de::Error as _;
use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::logging::StructuredLogger;
use crate::persistence::ProjectionDb;
use crate::protocol::{PluginOverview, PluginState, ToolDefinition, ToolResult, ToolRisk};
use crate::tools::{ToolContext, ToolError, ToolHandler, ToolHookRunner};

use self::hooks::{HookConfig, HookPipeline};
use self::mcp::{McpSecretStore, McpServerConfig};
use self::plugins::PluginHost;

const MAX_INSTRUCTION_FILE_BYTES: usize = 256 * 1024;
const MAX_RUNTIME_INSTRUCTION_BYTES: usize = 48 * 1024;
const MAX_SKILL_BYTES: usize = 256 * 1024;
const MAX_SKILL_RESOURCE_FILE_BYTES: usize = 256 * 1024;
const MAX_SKILL_RESOURCE_READ_BYTES: usize = 64 * 1024;
const DEFAULT_SKILL_RESOURCE_READ_BYTES: usize = 16 * 1024;
const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_SELECTED_SKILLS: usize = 4;
const MAX_AUDIT_RECORDS: usize = 200;
const MAX_AUDIT_BYTES: u64 = 2 * 1024 * 1024;
const MCP_CONFIG_FILE_NAME: &str = "mcp.json";
const USER_RULES_CONFIG_FILE_NAME: &str = "user-rules.json";
const USER_RULES_SCHEMA_VERSION: u32 = 1;
const MAX_USER_RULES: usize = 64;
const MAX_USER_RULE_TITLE_CHARS: usize = 80;
const MAX_USER_RULE_BYTES: usize = 16 * 1024;
const MAX_USER_RULES_CONFIG_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("extension configuration failed: {0}")]
    Config(String),
    #[error("extension I/O failed: {0}")]
    Io(String),
    #[error("Skill validation failed: {0}")]
    Skill(String),
    #[error(transparent)]
    Mcp(#[from] mcp::McpError),
    #[error(transparent)]
    Plugin(#[from] plugins::PluginError),
    #[error("extension tool registration failed: {0}")]
    Tool(String),
}

impl From<ToolError> for ExtensionError {
    fn from(value: ToolError) -> Self {
        Self::Tool(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionConfig {
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub hooks: Vec<HookConfig>,
}

impl Default for ExtensionConfig {
    fn default() -> Self {
        Self {
            mcp_servers: Vec::new(),
            hooks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct McpConfigFile {
    pub mcp_servers: Vec<McpServerConfig>,
}

const DEFAULT_MCP_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct McpConfigFileWire {
    #[serde(default)]
    mcp_servers: McpServersWire,
}

#[derive(Debug, Default)]
struct McpServersWire(Vec<McpServerConfig>);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NamedMcpServerWire {
    #[serde(default, rename = "type", alias = "transport")]
    kind: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default, alias = "timeout_ms")]
    timeout_ms: Option<u64>,
    #[serde(default)]
    command: Option<NamedMcpCommandWire>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, deserialize_with = "mcp::deserialize_header_map")]
    headers: HashMap<String, String>,
    #[serde(default, rename = "secret_env", alias = "secretEnv")]
    secret_env: HashMap<String, String>,
    #[serde(
        default,
        rename = "secret_headers",
        alias = "secretHeaders",
        deserialize_with = "mcp::deserialize_header_map"
    )]
    secret_headers: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum NamedMcpCommandWire {
    Program(String),
    Structured(Vec<String>),
}

impl<'de> Deserialize<'de> for McpServersWire {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = McpServersWire;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an MCP server object or a legacy MCP server array")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut servers = Vec::new();
                while let Some(server) = sequence.next_element::<McpServerConfig>()? {
                    servers.push(server);
                }
                Ok(McpServersWire(servers))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut ids = HashSet::new();
                let mut servers = Vec::new();
                while let Some((id, server)) = map.next_entry::<String, NamedMcpServerWire>()? {
                    if !ids.insert(id.clone()) {
                        return Err(A::Error::custom(format!("duplicate MCP server {id}")));
                    }
                    servers.push(server.into_config(id).map_err(A::Error::custom)?);
                }
                Ok(McpServersWire(servers))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl NamedMcpServerWire {
    fn into_config(self, id: String) -> Result<McpServerConfig, String> {
        let Self {
            kind,
            enabled,
            timeout_ms,
            command,
            args,
            url,
            headers,
            secret_env,
            secret_headers,
        } = self;
        let is_http = match kind.as_deref() {
            Some("stdio") => false,
            Some("http" | "streamable-http" | "streamable_http") => true,
            Some(other) => {
                return Err(format!(
                    "MCP server {id} type must be stdio or streamable-http, got {other}"
                ));
            }
            None if command.is_some() && url.is_none() => false,
            None if url.is_some() && command.is_none() => true,
            None => {
                return Err(format!(
                    "MCP server {id} must set type or provide exactly one of command and url"
                ));
            }
        };
        let enabled = enabled.unwrap_or(true);
        let timeout_ms = timeout_ms.unwrap_or(DEFAULT_MCP_TIMEOUT_MS);

        let transport = if is_http {
            if command.is_some() || !args.is_empty() || !secret_env.is_empty() {
                return Err(format!(
                    "MCP server {id} streamable-http configuration cannot contain stdio fields"
                ));
            }
            let url = url
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("MCP server {id} requires url"))?;
            mcp::McpTransportConfig::StreamableHttp {
                url,
                headers,
                secret_headers,
            }
        } else {
            if url.is_some() || !headers.is_empty() || !secret_headers.is_empty() {
                return Err(format!(
                    "MCP server {id} stdio configuration cannot contain HTTP fields"
                ));
            }
            let command = command.ok_or_else(|| format!("MCP server {id} requires command"))?;
            let command = match command {
                NamedMcpCommandWire::Program(program) => {
                    if program.trim().is_empty() {
                        return Err(format!("MCP server {id} requires command"));
                    }
                    let mut structured = Vec::with_capacity(args.len() + 1);
                    structured.push(program);
                    structured.extend(args);
                    structured
                }
                NamedMcpCommandWire::Structured(structured) => {
                    if !args.is_empty() {
                        return Err(format!(
                            "MCP server {id} cannot combine an array command with args"
                        ));
                    }
                    structured
                }
            };
            mcp::McpTransportConfig::Stdio {
                command,
                secret_env,
            }
        };

        Ok(McpServerConfig {
            id,
            enabled,
            timeout_ms,
            transport,
        })
    }
}

impl<'de> Deserialize<'de> for McpConfigFile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = McpConfigFileWire::deserialize(deserializer)?;
        Ok(Self {
            mcp_servers: wire.mcp_servers.0,
        })
    }
}

struct NamedMcpServers<'a>(&'a [McpServerConfig]);
struct NamedMcpServer<'a>(&'a McpServerConfig);
struct SortedStringMap<'a>(&'a HashMap<String, String>);

impl Serialize for McpConfigFile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut document = serializer.serialize_struct("McpConfigFile", 1)?;
        document.serialize_field("mcpServers", &NamedMcpServers(&self.mcp_servers))?;
        document.end()
    }
}

impl Serialize for NamedMcpServers<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut servers = serializer.serialize_map(Some(self.0.len()))?;
        for server in self.0 {
            servers.serialize_entry(&server.id, &NamedMcpServer(server))?;
        }
        servers.end()
    }
}

impl Serialize for NamedMcpServer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let server = self.0;
        match &server.transport {
            mcp::McpTransportConfig::Stdio {
                command,
                secret_env,
            } => {
                let mut fields = 2;
                fields += usize::from(!server.enabled);
                fields += usize::from(server.timeout_ms != DEFAULT_MCP_TIMEOUT_MS);
                fields += usize::from(command.len() > 1);
                fields += usize::from(!secret_env.is_empty());
                let mut value = serializer.serialize_struct("NamedMcpServer", fields)?;
                value.serialize_field("type", "stdio")?;
                if !server.enabled {
                    value.serialize_field("enabled", &false)?;
                }
                if server.timeout_ms != DEFAULT_MCP_TIMEOUT_MS {
                    value.serialize_field("timeoutMs", &server.timeout_ms)?;
                }
                value.serialize_field(
                    "command",
                    command.first().map(String::as_str).unwrap_or_default(),
                )?;
                if command.len() > 1 {
                    value.serialize_field("args", &command[1..])?;
                }
                if !secret_env.is_empty() {
                    value.serialize_field("secret_env", &SortedStringMap(secret_env))?;
                }
                value.end()
            }
            mcp::McpTransportConfig::StreamableHttp {
                url,
                headers,
                secret_headers,
            } => {
                let mut fields = 2;
                fields += usize::from(!server.enabled);
                fields += usize::from(server.timeout_ms != DEFAULT_MCP_TIMEOUT_MS);
                fields += usize::from(!headers.is_empty());
                fields += usize::from(!secret_headers.is_empty());
                let mut value = serializer.serialize_struct("NamedMcpServer", fields)?;
                value.serialize_field("type", "streamable-http")?;
                if !server.enabled {
                    value.serialize_field("enabled", &false)?;
                }
                if server.timeout_ms != DEFAULT_MCP_TIMEOUT_MS {
                    value.serialize_field("timeoutMs", &server.timeout_ms)?;
                }
                value.serialize_field("url", url)?;
                if !headers.is_empty() {
                    value.serialize_field("headers", &SortedStringMap(headers))?;
                }
                if !secret_headers.is_empty() {
                    value.serialize_field("secret_headers", &SortedStringMap(secret_headers))?;
                }
                value.end()
            }
        }
    }
}

impl Serialize for SortedStringMap<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut entries = self.0.iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(right.0));
        let mut map = serializer.serialize_map(Some(entries.len()))?;
        for (key, value) in entries {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigDocumentView {
    pub scope: String,
    pub path: String,
    pub exists: bool,
    pub content: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigView {
    pub schema_version: u32,
    pub global: McpConfigDocumentView,
    pub project: McpConfigDocumentView,
    pub overview: ExtensionOverview,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionSource {
    pub path: String,
    pub scope: String,
    pub priority: u32,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserRule {
    pub id: String,
    pub title: String,
    pub content: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveUserRuleRequest {
    pub id: Option<String>,
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRulesView {
    pub schema_version: u32,
    pub path: String,
    pub rules: Vec<UserRule>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UserRulesFile {
    schema_version: u32,
    #[serde(default)]
    rules: Vec<UserRule>,
}

impl Default for UserRulesFile {
    fn default() -> Self {
        Self {
            schema_version: USER_RULES_SCHEMA_VERSION,
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct LoadedInstruction {
    source: InstructionSource,
    content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillCategory {
    RequirementsPlanning,
    DevelopmentDelivery,
    QualityReview,
    Testing,
    DesignExperience,
    DataDocuments,
    Observability,
    IntegrationAutomation,
    ExtensionPlatform,
    Other,
}

impl Default for SkillCategory {
    fn default() -> Self {
        Self::Other
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillMetadata {
    name: String,
    description: String,
    triggers: Vec<String>,
    risk: ToolRisk,
    #[serde(default)]
    category: SkillCategory,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone)]
struct LoadedSkill {
    metadata: SkillMetadata,
    path: PathBuf,
    resource_root: SkillResourceRoot,
    definition_identity: SkillFileIdentity,
    scope: String,
    robot_pack: bool,
    body: String,
    enabled: bool,
}

impl std::fmt::Debug for LoadedSkill {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoadedSkill")
            .field("metadata", &self.metadata)
            .field("scope", &self.scope)
            .field("enabled", &self.enabled)
            .field("bytes", &self.body.len())
            .field("sha256", &sha256_hex(self.body.as_bytes()))
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct SkillResourceRoot {
    handle: Arc<File>,
    path: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SkillFileIdentity {
    volume: u64,
    file: u64,
}

impl std::fmt::Debug for SkillResourceRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SkillResourceRoot")
            .finish_non_exhaustive()
    }
}

impl SkillResourceRoot {
    fn open(scope_root: &Path, path: &Path) -> Result<Self, ExtensionError> {
        let handle = open_skill_resource_root(scope_root, path).map_err(ExtensionError::Skill)?;
        Ok(Self {
            handle: Arc::new(handle),
            path: path.to_path_buf(),
        })
    }

    fn read(
        &self,
        relative: &Path,
        definition_identity: SkillFileIdentity,
    ) -> Result<(Vec<u8>, usize), ToolError> {
        read_skill_resource_handle(self, relative, definition_identity)
    }
}

#[cfg(unix)]
fn open_skill_resource_root(scope_root: &Path, path: &Path) -> Result<File, String> {
    let relative = path
        .strip_prefix(scope_root)
        .map_err(|_| "Skill resource root escapes its scope".to_string())?;
    let mut current = unix_open_absolute_directory(scope_root)?;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err("Skill resource root contains an unsupported component".into());
        };
        current = unix_open_directory_at(&current, component)?;
    }
    Ok(current)
}

#[cfg(unix)]
fn unix_open_absolute_directory(path: &Path) -> Result<File, String> {
    use std::os::unix::fs::OpenOptionsExt;

    if !path.is_absolute() {
        return Err("Skill scope root must be absolute".into());
    }
    let mut current = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(Path::new("/"))
        .map_err(|error| format!("filesystem root cannot be opened safely: {error}"))?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(component) => {
                current = unix_open_directory_at(&current, component)?;
            }
            _ => return Err("Skill scope root contains an unsupported component".into()),
        }
    }
    Ok(current)
}

#[cfg(unix)]
fn unix_open_directory_at(parent: &File, component: &std::ffi::OsStr) -> Result<File, String> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let component = CString::new(component.as_bytes())
        .map_err(|_| "Skill directory contains a null byte".to_string())?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            component.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        return Err("Skill directory cannot be opened without following links".into());
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn read_skill_resource_handle(
    root: &SkillResourceRoot,
    relative: &Path,
    definition_identity: SkillFileIdentity,
) -> Result<(Vec<u8>, usize), ToolError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let duplicated = unsafe { libc::fcntl(root.handle.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        return Err(ToolError::Execution(
            "Skill resource root handle cannot be duplicated".into(),
        ));
    }
    let mut current = unsafe { File::from_raw_fd(duplicated) };
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(ToolError::InvalidArguments(
                "Skill resource path contains an unsupported component".into(),
            ));
        };
        let component = CString::new(component.as_bytes()).map_err(|_| {
            ToolError::InvalidArguments("Skill resource path contains a null byte".into())
        })?;
        let final_component = index + 1 == components.len();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if final_component {
                0
            } else {
                libc::O_DIRECTORY
            };
        let descriptor = unsafe { libc::openat(current.as_raw_fd(), component.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(ToolError::InvalidArguments(
                "Skill resource path cannot be opened without following links".into(),
            ));
        }
        current = unsafe { File::from_raw_fd(descriptor) };
        let metadata = current
            .metadata()
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        if final_component && !metadata.is_file() {
            return Err(ToolError::InvalidArguments(
                "Skill resource path must identify a regular file".into(),
            ));
        }
    }
    if skill_file_identity(&current).map_err(ToolError::Execution)? == definition_identity {
        return Err(ToolError::InvalidArguments(
            "Skill definition files cannot be read as persistent resources".into(),
        ));
    }
    read_bounded_skill_resource_file(current)
}

#[cfg(windows)]
fn open_skill_resource_root(scope_root: &Path, path: &Path) -> Result<File, String> {
    let scope_handle = open_windows_skill_directory(scope_root)?;
    let handle = open_windows_skill_directory(path)?;
    let scope_final = windows_final_path(&scope_handle)?;
    let opened = windows_final_path(&handle)?;
    let expected_scope = normalize_windows_final_path(scope_root);
    let expected = normalize_windows_final_path(path);
    if scope_final != expected_scope || opened != expected {
        return Err("Skill resource root changed while it was opened".into());
    }
    let prefix = format!("{}\\", scope_final.trim_end_matches('\\'));
    if !opened.starts_with(&prefix) {
        return Err("Skill resource root escapes its open scope".into());
    }
    Ok(handle)
}

#[cfg(windows)]
fn open_windows_skill_directory(path: &Path) -> Result<File, String> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let handle = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| format!("Skill resource root cannot be opened safely: {error}"))?;
    let metadata = handle
        .metadata()
        .map_err(|error| format!("Skill resource root metadata cannot be read: {error}"))?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !metadata.is_dir() {
        return Err("Skill resource root must be a real directory, not a reparse point".into());
    }
    Ok(handle)
}

#[cfg(windows)]
fn read_skill_resource_handle(
    root: &SkillResourceRoot,
    relative: &Path,
    definition_identity: SkillFileIdentity,
) -> Result<(Vec<u8>, usize), ToolError> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let candidate = root.path.join(relative);
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&candidate)
        .map_err(|_| {
            ToolError::InvalidArguments(
                "Skill resource path cannot be opened without following links".into(),
            )
        })?;
    let metadata = file
        .metadata()
        .map_err(|error| ToolError::Execution(error.to_string()))?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !metadata.is_file() {
        return Err(ToolError::InvalidArguments(
            "Skill resource path must identify a regular non-reparse file".into(),
        ));
    }
    let root_path = windows_final_path(&root.handle).map_err(ToolError::Execution)?;
    let resource_path = windows_final_path(&file).map_err(ToolError::Execution)?;
    let prefix = format!("{}\\", root_path.trim_end_matches('\\'));
    if !resource_path.starts_with(&prefix) {
        return Err(ToolError::InvalidArguments(
            "Skill resource path escapes its open Skill root".into(),
        ));
    }
    if skill_file_identity(&file).map_err(ToolError::Execution)? == definition_identity {
        return Err(ToolError::InvalidArguments(
            "Skill definition files cannot be read as persistent resources".into(),
        ));
    }
    read_bounded_skill_resource_file(file)
}

#[cfg(unix)]
fn skill_file_identity(file: &File) -> Result<SkillFileIdentity, String> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file
        .metadata()
        .map_err(|error| format!("Skill file identity cannot be read: {error}"))?;
    Ok(SkillFileIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
    })
}

#[cfg(windows)]
fn skill_file_identity(file: &File) -> Result<SkillFileIdentity, String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle, &mut information) } == 0 {
        return Err("Skill file identity cannot be read from its handle".into());
    }
    Ok(SkillFileIdentity {
        volume: u64::from(information.dwVolumeSerialNumber),
        file: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    })
}

#[cfg(windows)]
fn windows_final_path(file: &File) -> Result<String, String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
    };

    let handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let required = unsafe {
        GetFinalPathNameByHandleW(
            handle,
            std::ptr::null_mut(),
            0,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    };
    if required == 0 || required > 32_768 {
        return Err("open Skill resource path cannot be resolved from its handle".into());
    }
    let mut buffer = vec![0u16; required as usize + 1];
    let written = unsafe {
        GetFinalPathNameByHandleW(
            handle,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    };
    if written == 0 || written as usize >= buffer.len() {
        return Err("open Skill resource path cannot be resolved from its handle".into());
    }
    Ok(normalize_windows_path_text(&String::from_utf16_lossy(
        &buffer[..written as usize],
    )))
}

#[cfg(windows)]
fn normalize_windows_final_path(path: &Path) -> String {
    normalize_windows_path_text(&path.to_string_lossy())
}

#[cfg(windows)]
fn normalize_windows_path_text(path: &str) -> String {
    path.strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn read_bounded_skill_resource_file(mut file: File) -> Result<(Vec<u8>, usize), ToolError> {
    let metadata = file
        .metadata()
        .map_err(|error| ToolError::Execution(error.to_string()))?;
    if !metadata.is_file() {
        return Err(ToolError::InvalidArguments(
            "Skill resource path must identify a regular file".into(),
        ));
    }
    let total_bytes = usize::try_from(metadata.len()).map_err(|_| {
        ToolError::InvalidArguments("Skill resource size cannot be represented".into())
    })?;
    if total_bytes > MAX_SKILL_RESOURCE_FILE_BYTES {
        return Err(ToolError::InvalidArguments(format!(
            "Skill resource exceeds the {MAX_SKILL_RESOURCE_FILE_BYTES} byte file limit"
        )));
    }
    let mut bytes = Vec::with_capacity(total_bytes.min(MAX_SKILL_RESOURCE_FILE_BYTES));
    Read::by_ref(&mut file)
        .take((MAX_SKILL_RESOURCE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| ToolError::Execution(error.to_string()))?;
    if bytes.len() > MAX_SKILL_RESOURCE_FILE_BYTES {
        return Err(ToolError::InvalidArguments(format!(
            "Skill resource exceeds the {MAX_SKILL_RESOURCE_FILE_BYTES} byte file limit"
        )));
    }
    let bytes_read = bytes.len();
    Ok((bytes, bytes_read))
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum ResolvedSkillSource {
    Ordinary { scope: String },
    Plugin { plugin_id: String },
    OrdinaryFallback { plugin_id: String, scope: String },
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ResolvedSkill {
    pub(crate) id: String,
    pub(crate) source: ResolvedSkillSource,
    pub(crate) risk: ToolRisk,
    pub(crate) enabled: bool,
    pub(crate) body: String,
    pub(crate) bytes: usize,
    pub(crate) sha256: String,
}

impl std::fmt::Debug for ResolvedSkill {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedSkill")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("risk", &self.risk)
            .field("enabled", &self.enabled)
            .field("bytes", &self.bytes)
            .field("sha256", &self.sha256)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDiagnostic {
    pub name: String,
    pub description: String,
    pub path: String,
    pub scope: String,
    pub risk: ToolRisk,
    pub category: SkillCategory,
    pub triggers: Vec<String>,
    pub enabled: bool,
    /// Built-in robot bindings own these Skills; they are always enabled and
    /// cannot be changed through the ordinary extension toggle.
    pub managed_by_robot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDiagnostic {
    pub id: String,
    pub transport: String,
    pub enabled: bool,
    pub state: String,
    pub tool_count: usize,
    pub credentials: Vec<CredentialDiagnostic>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialDiagnostic {
    pub name: String,
    pub configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookDiagnostic {
    pub id: String,
    pub phase: String,
    pub tool: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionAudit {
    pub timestamp_ms: u64,
    pub event: String,
    pub kind: String,
    pub id: String,
    pub success: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionOverview {
    pub schema_version: u32,
    pub config_paths: Vec<String>,
    pub instructions: Vec<InstructionSource>,
    pub skills: Vec<SkillDiagnostic>,
    pub mcp_servers: Vec<McpDiagnostic>,
    pub hooks: Vec<HookDiagnostic>,
    pub audit: Vec<ExtensionAudit>,
    pub error: Option<String>,
}

pub struct PreparedExtensions {
    pub handlers: Vec<Arc<dyn ToolHandler>>,
    pub risks: HashMap<String, ToolRisk>,
    pub hooks: Option<Arc<dyn ToolHookRunner>>,
}

#[derive(Clone)]
pub struct ExtensionService {
    data_root: PathBuf,
    builtin_skills_root: Option<PathBuf>,
    projection: ProjectionDb,
    secrets: Arc<dyn McpSecretStore>,
    logger: StructuredLogger,
    overview: Arc<RwLock<ExtensionOverview>>,
    instructions: Arc<RwLock<Vec<LoadedInstruction>>>,
    skills: Arc<RwLock<Vec<LoadedSkill>>>,
    audit: Arc<Mutex<Vec<ExtensionAudit>>>,
    audit_path: PathBuf,
    user_rules_lock: Arc<Mutex<()>>,
    plugins: PluginHost,
}

fn skill_is_selected(lower_input: &str, skill: &LoadedSkill) -> bool {
    let explicit_name = format!("/{}", skill.metadata.name);
    let explicitly_invoked = lower_input
        .split_whitespace()
        .any(|token| token == explicit_name);
    explicitly_invoked
        || skill.metadata.triggers.iter().any(|trigger| {
            !trigger.trim().is_empty() && lower_input.contains(&trigger.to_lowercase())
        })
}

impl ExtensionService {
    pub fn new(
        data_root: PathBuf,
        projection: ProjectionDb,
        secrets: Arc<dyn McpSecretStore>,
        logger: StructuredLogger,
    ) -> Self {
        Self::with_builtin_skills(data_root, None, projection, secrets, logger)
    }

    pub fn with_builtin_skills(
        data_root: PathBuf,
        builtin_skills_root: Option<PathBuf>,
        projection: ProjectionDb,
        secrets: Arc<dyn McpSecretStore>,
        logger: StructuredLogger,
    ) -> Self {
        let audit_path = data_root.join("extension-audit.jsonl");
        let audit = load_audit(&audit_path);
        let plugins = PluginHost::new(data_root.clone(), projection.clone());
        Self {
            data_root,
            builtin_skills_root,
            projection,
            secrets,
            logger,
            overview: Arc::new(RwLock::new(ExtensionOverview {
                schema_version: 1,
                audit: audit.clone(),
                ..ExtensionOverview::default()
            })),
            instructions: Arc::new(RwLock::new(Vec::new())),
            skills: Arc::new(RwLock::new(Vec::new())),
            audit: Arc::new(Mutex::new(audit)),
            audit_path,
            user_rules_lock: Arc::new(Mutex::new(())),
            plugins,
        }
    }

    fn config_paths(
        &self,
        workspace: &Path,
    ) -> Result<(Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>), ExtensionError> {
        let global_extensions =
            resolve_scoped_config_path(&self.data_root, Path::new("extensions.json"), false)?;
        let global_mcp =
            resolve_scoped_config_path(&self.data_root, Path::new(MCP_CONFIG_FILE_NAME), false)?;
        let project_extensions_relative = Path::new(".k-coder").join("extensions.json");
        let project_mcp_relative = Path::new(".k-coder").join(MCP_CONFIG_FILE_NAME);
        let project_extensions =
            resolve_scoped_config_path(workspace, &project_extensions_relative, false)?;
        let project_mcp = resolve_scoped_config_path(workspace, &project_mcp_relative, false)?;
        let extension_paths = vec![global_extensions.clone(), project_extensions.clone()];
        let mcp_paths = vec![global_mcp.clone(), project_mcp.clone()];
        let ordered_paths = vec![
            global_extensions,
            global_mcp,
            project_extensions,
            project_mcp,
        ];
        Ok((extension_paths, mcp_paths, ordered_paths))
    }

    pub async fn prepare(
        &self,
        workspace: &Path,
        cancellation: CancellationToken,
    ) -> Result<PreparedExtensions, ExtensionError> {
        let workspace = workspace
            .canonicalize()
            .map_err(|error| ExtensionError::Io(error.to_string()))?;
        let (extension_config_paths, mcp_config_paths, config_paths) =
            self.config_paths(&workspace)?;
        let config = merge_configs(&extension_config_paths, &mcp_config_paths)?;
        let instructions = discover_instructions(&self.data_root, &workspace)?;
        let skills = discover_skills(
            self.builtin_skills_root.as_deref(),
            &self.data_root,
            &workspace,
            &self.projection,
        )?;
        let skill_resource_handler = Arc::new(SkillResourceReadTool {
            service: self.clone(),
        }) as Arc<dyn ToolHandler>;
        let mut handlers = vec![skill_resource_handler];
        let mut risks = HashMap::from([("skill_resource_read".to_string(), ToolRisk::Read)]);
        let mut tool_names = HashSet::from(["skill_resource_read".to_string()]);
        let mut mcp_diagnostics = Vec::new();

        for server in &config.mcp_servers {
            server.validate()?;
            let enabled = server.enabled && self.enabled("mcp", &server.id, true)?;
            let credentials = server
                .credential_names()
                .into_iter()
                .map(|name| {
                    let configured = self.secrets.get(&server.id, &name)?.is_some();
                    Ok(CredentialDiagnostic { name, configured })
                })
                .collect::<Result<Vec<_>, mcp::McpError>>()?;
            if !enabled {
                mcp_diagnostics.push(McpDiagnostic {
                    id: server.id.clone(),
                    transport: server.transport_name().into(),
                    enabled: false,
                    state: "disabled".into(),
                    tool_count: 0,
                    credentials,
                    error: None,
                });
                continue;
            }
            let tools = match mcp::connect(server, self.secrets.clone(), cancellation.clone()).await
            {
                Ok(tools) => tools,
                Err(error) => {
                    self.record("mcp_connect", "mcp", &server.id, false, &error.to_string());
                    mcp_diagnostics.push(McpDiagnostic {
                        id: server.id.clone(),
                        transport: server.transport_name().into(),
                        enabled: true,
                        state: "failed".into(),
                        tool_count: 0,
                        credentials,
                        error: Some(error.to_string()),
                    });
                    self.update_overview(
                        &config_paths,
                        &instructions,
                        &skills,
                        mcp_diagnostics,
                        &config.hooks,
                        Some(error.to_string()),
                    );
                    return Err(error.into());
                }
            };
            for tool in &tools {
                if !tool_names.insert(tool.name.clone()) {
                    return Err(ExtensionError::Tool(format!(
                        "MCP namespace collision: {}",
                        tool.name
                    )));
                }
                risks.insert(tool.name.clone(), tool.risk);
                handlers.push(tool.handler());
            }
            self.record(
                "mcp_connect",
                "mcp",
                &server.id,
                true,
                &format!("{} tools discovered", tools.len()),
            );
            mcp_diagnostics.push(McpDiagnostic {
                id: server.id.clone(),
                transport: server.transport_name().into(),
                enabled: true,
                state: "ready".into(),
                tool_count: tools.len(),
                credentials,
                error: None,
            });
        }

        let plugin_prepared = self
            .plugins
            .prepare(self.secrets.clone(), cancellation.clone())
            .await?;
        self.record_auto_disabled_plugins();
        for handler in plugin_prepared.handlers {
            let name = handler.definition().name;
            if !tool_names.insert(name.clone()) {
                return Err(ExtensionError::Tool(format!(
                    "plugin tool conflicts with an existing extension tool: {name}"
                )));
            }
            let risk = plugin_prepared.risks.get(&name).copied().ok_or_else(|| {
                ExtensionError::Tool(format!("plugin tool is missing risk metadata: {name}"))
            })?;
            risks.insert(name, risk);
            handlers.push(handler);
        }
        for plugin in plugin_prepared
            .overview
            .plugins
            .iter()
            .filter(|plugin| plugin.enabled)
        {
            let success = matches!(plugin.state, PluginState::Loaded | PluginState::Degraded);
            self.record(
                "plugin_prepared",
                "plugin",
                &plugin.id,
                success,
                &format!(
                    "state={:?}, skills={}, mcp_servers={}, mcp_tools={}",
                    plugin.state,
                    plugin.components.skill_count,
                    plugin.components.mcp_server_count,
                    plugin.components.mcp_tool_count
                ),
            );
        }

        let mut enabled_hooks = Vec::new();
        for hook in &config.hooks {
            hook.validate().map_err(ExtensionError::Config)?;
            let mut hook = hook.clone();
            hook.enabled = hook.enabled && self.enabled("hook", &hook.id, true)?;
            if hook.enabled {
                enabled_hooks.push(hook);
            }
        }
        let pipeline = HookPipeline::new(enabled_hooks, workspace, self.logger.clone())
            .map_err(ExtensionError::Config)?;
        let hooks = (!pipeline.is_empty()).then(|| Arc::new(pipeline) as Arc<dyn ToolHookRunner>);

        *self
            .instructions
            .write()
            .expect("instruction lock poisoned") = instructions.clone();
        *self.skills.write().expect("skill lock poisoned") = skills.clone();
        self.update_overview(
            &config_paths,
            &instructions,
            &skills,
            mcp_diagnostics,
            &config.hooks,
            None,
        );
        self.record(
            "extensions_ready",
            "runtime",
            "all",
            true,
            "extensions loaded",
        );
        Ok(PreparedExtensions {
            handlers,
            risks,
            hooks,
        })
    }

    pub fn revision(&self, workspace: &Path) -> Result<u64, ExtensionError> {
        let (_, _, mut paths) = self.config_paths(workspace)?;
        let user_rules_path = resolve_scoped_config_path(
            &self.data_root,
            Path::new(USER_RULES_CONFIG_FILE_NAME),
            false,
        )?;
        paths.extend([
            self.data_root.join("AGENTS.md"),
            user_rules_path,
            workspace.join("AGENTS.md"),
        ]);
        if let Some(root) = &self.builtin_skills_root {
            collect_extension_files(root, &mut paths)?;
        }
        collect_extension_files(&self.data_root.join("skills"), &mut paths)?;
        collect_extension_files(&workspace.join(".k-coder").join("skills"), &mut paths)?;
        collect_extension_files(&workspace.join(".k-coder").join("rules"), &mut paths)?;
        paths.sort();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for path in paths {
            path.hash(&mut hasher);
            match path.metadata() {
                Ok(metadata) => {
                    metadata.len().hash(&mut hasher);
                    metadata
                        .modified()
                        .ok()
                        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|value| value.as_nanos())
                        .unwrap_or(0)
                        .hash(&mut hasher);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0u8.hash(&mut hasher),
                Err(error) => return Err(ExtensionError::Io(error.to_string())),
            }
        }
        let plugin_revision = self.plugins.revision()?;
        self.record_auto_disabled_plugins();
        plugin_revision.hash(&mut hasher);
        Ok(hasher.finish())
    }

    pub fn runtime_instructions(&self, input: &str) -> Result<String, ExtensionError> {
        self.runtime_instructions_with_builtin_skills(input, true)
    }

    pub(crate) fn runtime_instructions_for_robot(
        &self,
        input: &str,
    ) -> Result<String, ExtensionError> {
        self.runtime_instructions_with_builtin_skills(input, false)
    }

    fn runtime_instructions_with_builtin_skills(
        &self,
        input: &str,
        include_builtin_skills: bool,
    ) -> Result<String, ExtensionError> {
        let instructions = self.instructions.read().expect("instruction lock poisoned");
        let skills = self.skills.read().expect("skill lock poisoned");
        let mut output = String::from(
            "[k-Coder runtime instructions]\nSources are ordered from lower to higher priority. Later instructions win on conflict. Extensions never grant tool permissions.\n",
        );
        for instruction in instructions.iter() {
            output.push_str(&format!(
                "\n--- {} (priority {}) ---\n{}\n",
                instruction.source.path, instruction.source.priority, instruction.content
            ));
        }
        let lower_input = input.to_lowercase();
        let selected = skills
            .iter()
            .filter(|skill| {
                skill.enabled
                    && (include_builtin_skills || skill.scope != "builtin")
                    && skill_is_selected(&lower_input, skill)
            })
            .take(MAX_SELECTED_SKILLS)
            .collect::<Vec<_>>();
        if !selected.is_empty() {
            output.push_str("\n[Selected Skills: instructions were read before execution]\n");
            for skill in selected {
                output.push_str(&format!(
                    "\n--- Skill {} (risk: {:?}, source: {}) ---\n{}\n",
                    skill.metadata.name,
                    skill.metadata.risk,
                    user_facing_path(&skill.path),
                    skill.body
                ));
                self.record(
                    "skill_selected",
                    "skill",
                    &skill.metadata.name,
                    true,
                    &format!("risk={:?}", skill.metadata.risk),
                );
            }
        }
        let plugin_catalog = self.plugins.runtime_catalog(input);
        if !plugin_catalog.is_empty() {
            output.push_str("\n");
            output.push_str(&plugin_catalog);
        }
        if output.len() > MAX_RUNTIME_INSTRUCTION_BYTES {
            return Err(ExtensionError::Config(format!(
                "combined runtime instructions exceed {MAX_RUNTIME_INSTRUCTION_BYTES} bytes"
            )));
        }
        Ok(output)
    }

    pub(crate) fn resolve_ordinary_skill(
        &self,
        skill_id: &str,
    ) -> Result<ResolvedSkill, ExtensionError> {
        let skills = self.skills.read().expect("skill lock poisoned");
        let skill = skills
            .iter()
            .find(|skill| skill.metadata.name == skill_id)
            .ok_or_else(|| ExtensionError::Skill(format!("Skill {skill_id} is not available")))?;
        Ok(resolved_ordinary_skill(skill))
    }

    /// Resolve the immutable Skill shipped for robot workflows. Robot-pack
    /// entries are deliberately looked up by their built-in scope instead of
    /// through the ordinary effective-scope map, so a global/project Skill
    /// with the same id cannot replace the workflow contract.
    pub(crate) fn resolve_robot_skill(
        &self,
        skill_id: &str,
    ) -> Result<ResolvedSkill, ExtensionError> {
        let skills = self.skills.read().expect("skill lock poisoned");
        let skill = skills
            .iter()
            .find(|skill| {
                skill.scope == "builtin" && skill.robot_pack && skill.metadata.name == skill_id
            })
            .or_else(|| {
                // Test fixtures and older resource bundles may place built-in
                // compatibility Skills directly under the built-in root.
                skills
                    .iter()
                    .find(|skill| skill.scope == "builtin" && skill.metadata.name == skill_id)
            })
            .ok_or_else(|| {
                ExtensionError::Skill(format!("robot Skill {skill_id} is not available"))
            })?;
        let mut resolved = resolved_ordinary_skill(skill);
        // Robot-pack entries are part of the immutable workflow contract.
        // Frontmatter and persisted ordinary-scope settings must not turn
        // them into a readiness blocker.
        resolved.enabled = true;
        Ok(resolved)
    }

    pub(crate) fn record_robot_skill_selected(
        &self,
        declaration: &str,
        source: &ResolvedSkillSource,
        sha256: &str,
        bytes: usize,
    ) {
        self.record(
            "robot_skill_selected",
            "skill",
            declaration,
            true,
            &format!("source={:?}, sha256={sha256}, bytes={bytes}", source),
        );
    }

    #[allow(dead_code)]
    pub(crate) fn resolve_plugin_skill(
        &self,
        plugin_id: &str,
        skill_id: &str,
        fallback_skill_id: Option<&str>,
    ) -> Result<ResolvedSkill, ExtensionError> {
        match self.plugins.resolve_skill(plugin_id, skill_id) {
            Ok(skill) => Ok(ResolvedSkill {
                id: skill.skill_id,
                source: ResolvedSkillSource::Plugin {
                    plugin_id: skill.plugin_id,
                },
                risk: skill.risk,
                enabled: skill.enabled,
                body: skill.body,
                bytes: skill.bytes,
                sha256: skill.sha256,
            }),
            Err(plugin_error) => {
                let Some(fallback_skill_id) = fallback_skill_id else {
                    return Err(ExtensionError::Skill(format!(
                        "plugin Skill {plugin_id}/{skill_id} is unavailable and has no ordinary fallback: {plugin_error}"
                    )));
                };
                let mut fallback = self.resolve_ordinary_skill(fallback_skill_id).map_err(|error| {
                    ExtensionError::Skill(format!(
                        "plugin Skill {plugin_id}/{skill_id} is unavailable ({plugin_error}); fallback {fallback_skill_id} failed: {error}"
                    ))
                })?;
                if !fallback.enabled {
                    return Err(ExtensionError::Skill(format!(
                        "plugin Skill {plugin_id}/{skill_id} is unavailable ({plugin_error}); fallback {fallback_skill_id} is disabled"
                    )));
                }
                let scope = match &fallback.source {
                    ResolvedSkillSource::Ordinary { scope } => scope.clone(),
                    _ => unreachable!("ordinary resolution must have an ordinary source"),
                };
                fallback.source = ResolvedSkillSource::OrdinaryFallback {
                    plugin_id: plugin_id.to_string(),
                    scope,
                };
                Ok(fallback)
            }
        }
    }

    fn read_skill_resource(
        &self,
        skill_id: &str,
        raw_path: &str,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<ToolResult, ToolError> {
        let resolved = self
            .resolve_ordinary_skill(skill_id)
            .map_err(|_| ToolError::Denied(format!("Skill {skill_id} is not available")))?;
        if !resolved.enabled {
            return Err(ToolError::Denied(format!("Skill {skill_id} is disabled")));
        }
        let (resource_root, definition_identity) = self
            .skills
            .read()
            .expect("skill lock poisoned")
            .iter()
            .find(|skill| skill.metadata.name == skill_id)
            .map(|skill| (skill.resource_root.clone(), skill.definition_identity))
            .ok_or_else(|| ToolError::Denied(format!("Skill {skill_id} is not available")))?;
        let relative = validate_skill_resource_path(raw_path)?;
        let skill_root = resource_root.path.as_path();
        resolve_skill_resource_path(skill_root, &relative)?;
        #[cfg(test)]
        run_skill_resource_before_open_hook(skill_root);
        let (bytes, total_bytes) = resource_root.read(&relative, definition_identity)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| ToolError::Execution("Skill resource must be UTF-8".into()))?;
        let offset = offset.unwrap_or(0);
        if offset > text.len() || !text.is_char_boundary(offset) {
            return Err(ToolError::InvalidArguments(
                "offset must be a UTF-8 byte boundary within the Skill resource".into(),
            ));
        }
        let limit = limit.unwrap_or(DEFAULT_SKILL_RESOURCE_READ_BYTES);
        if !(1..=MAX_SKILL_RESOURCE_READ_BYTES).contains(&limit) {
            return Err(ToolError::InvalidArguments(format!(
                "limit must be between 1 and {MAX_SKILL_RESOURCE_READ_BYTES} bytes"
            )));
        }
        let mut end = offset.saturating_add(limit).min(text.len());
        while end > offset && !text.is_char_boundary(end) {
            end -= 1;
        }
        Ok(ToolResult {
            success: true,
            output: text[offset..end].to_string(),
            metadata: json!({
                "skillId": skill_id,
                "path": raw_path,
                "offset": offset,
                "bytesReturned": end - offset,
                "totalBytes": total_bytes,
                "truncated": offset > 0 || end < text.len(),
            }),
        })
    }

    pub fn overview(&self) -> ExtensionOverview {
        let mut overview = self
            .overview
            .read()
            .expect("overview lock poisoned")
            .clone();
        overview.audit = self.audit.lock().expect("audit lock poisoned").clone();
        overview
    }

    pub fn plugin_overview(&self, refresh: bool) -> Result<PluginOverview, ExtensionError> {
        let result = if refresh {
            self.plugins.scan()
        } else {
            Ok(self.plugins.overview())
        };
        self.record_auto_disabled_plugins();
        Ok(result?)
    }

    pub fn set_plugin_enabled(
        &self,
        plugin_id: &str,
        enabled: bool,
    ) -> Result<PluginOverview, ExtensionError> {
        let result = self.plugins.set_enabled(plugin_id, enabled);
        self.record_auto_disabled_plugins();
        self.record(
            "plugin_toggled",
            "plugin",
            plugin_id,
            result.is_ok(),
            if enabled { "enabled" } else { "disabled" },
        );
        Ok(result?)
    }

    pub fn delete_plugin(&self, plugin_id: &str) -> Result<PluginOverview, ExtensionError> {
        let result = self.plugins.delete(plugin_id);
        self.record_auto_disabled_plugins();
        self.record(
            "plugin_deleted",
            "plugin",
            plugin_id,
            result.is_ok(),
            result
                .as_ref()
                .map(|_| "deleted")
                .unwrap_or("filesystem deletion failed"),
        );
        Ok(result?)
    }

    fn record_auto_disabled_plugins(&self) {
        for plugin_id in self.plugins.take_auto_disabled_ids() {
            self.record(
                "plugin_auto_disabled",
                "plugin",
                &plugin_id,
                true,
                "enabled plugin disappeared or became invalid; persisted state reset",
            );
        }
    }

    pub fn mcp_config_view(&self, workspace: &Path) -> Result<McpConfigView, ExtensionError> {
        let (_, mcp_paths, _) = self.config_paths(workspace)?;
        let global = read_mcp_config_document("global", &mcp_paths[0])?;
        let project = read_mcp_config_document("project", &mcp_paths[1])?;
        Ok(McpConfigView {
            schema_version: 2,
            global,
            project,
            overview: self.overview(),
        })
    }

    pub fn save_mcp_config(
        &self,
        workspace: &Path,
        scope: &str,
        content: &str,
    ) -> Result<(), ExtensionError> {
        let relative = match scope {
            "global" => PathBuf::from(MCP_CONFIG_FILE_NAME),
            "project" => Path::new(".k-coder").join(MCP_CONFIG_FILE_NAME),
            _ => {
                return Err(ExtensionError::Config(
                    "MCP configuration scope must be global or project".into(),
                ));
            }
        };
        let root = if scope == "global" {
            self.data_root.as_path()
        } else {
            workspace
        };
        let display_path = resolve_scoped_config_path(root, &relative, false)?;
        let config = parse_mcp_config(content.as_bytes(), &display_path)?;
        let path = resolve_scoped_config_path(root, &relative, true)?;
        write_mcp_config(&path, &config)?;
        for server in &config.mcp_servers {
            self.projection
                .set_setting(&format!("extension/mcp/{}", server.id), "true")
                .map_err(|error| ExtensionError::Config(error.to_string()))?;
        }
        self.record(
            "mcp_config_saved",
            "mcp_config",
            scope,
            true,
            &format!("{} servers", config.mcp_servers.len()),
        );
        Ok(())
    }

    pub fn user_rules_view(&self) -> Result<UserRulesView, ExtensionError> {
        let _guard = self
            .user_rules_lock
            .lock()
            .map_err(|_| ExtensionError::Io("user rule lock poisoned".into()))?;
        let path = resolve_scoped_config_path(
            &self.data_root,
            Path::new(USER_RULES_CONFIG_FILE_NAME),
            false,
        )?;
        let file = read_user_rules_file(&path)?;
        Ok(UserRulesView {
            schema_version: USER_RULES_SCHEMA_VERSION,
            path: user_facing_path(&path),
            rules: file.rules,
            error: None,
        })
    }

    pub fn save_user_rule(&self, request: SaveUserRuleRequest) -> Result<(), ExtensionError> {
        let _guard = self
            .user_rules_lock
            .lock()
            .map_err(|_| ExtensionError::Io("user rule lock poisoned".into()))?;
        let display_path = resolve_scoped_config_path(
            &self.data_root,
            Path::new(USER_RULES_CONFIG_FILE_NAME),
            false,
        )?;
        let mut file = read_user_rules_file(&display_path)?;
        let (title, content) = validate_user_rule_input(&request.title, &request.content)?;
        let now = crate::storage::now_ms();
        let (id, detail) = if let Some(id) = request.id {
            validate_user_rule_id(&id)?;
            let rule = file
                .rules
                .iter_mut()
                .find(|rule| rule.id == id)
                .ok_or_else(|| ExtensionError::Config("user rule no longer exists".into()))?;
            rule.title = title;
            rule.content = content;
            rule.updated_at_ms = now.max(rule.created_at_ms);
            (id, "updated")
        } else {
            if file.rules.len() >= MAX_USER_RULES {
                return Err(ExtensionError::Config(format!(
                    "no more than {MAX_USER_RULES} user rules are allowed"
                )));
            }
            let id = uuid::Uuid::new_v4().to_string();
            file.rules.push(UserRule {
                id: id.clone(),
                title,
                content,
                created_at_ms: now,
                updated_at_ms: now,
            });
            (id, "created")
        };
        validate_user_rules_file(&file, &display_path)?;
        let path = resolve_scoped_config_path(
            &self.data_root,
            Path::new(USER_RULES_CONFIG_FILE_NAME),
            true,
        )?;
        write_user_rules_file(&path, &file)?;
        self.record("user_rule_saved", "user_rule", &id, true, detail);
        Ok(())
    }

    pub fn delete_user_rule(&self, id: &str) -> Result<(), ExtensionError> {
        validate_user_rule_id(id)?;
        let _guard = self
            .user_rules_lock
            .lock()
            .map_err(|_| ExtensionError::Io("user rule lock poisoned".into()))?;
        let display_path = resolve_scoped_config_path(
            &self.data_root,
            Path::new(USER_RULES_CONFIG_FILE_NAME),
            false,
        )?;
        let mut file = read_user_rules_file(&display_path)?;
        let previous_len = file.rules.len();
        file.rules.retain(|rule| rule.id != id);
        if file.rules.len() == previous_len {
            return Err(ExtensionError::Config("user rule no longer exists".into()));
        }
        let path = resolve_scoped_config_path(
            &self.data_root,
            Path::new(USER_RULES_CONFIG_FILE_NAME),
            true,
        )?;
        write_user_rules_file(&path, &file)?;
        self.record("user_rule_deleted", "user_rule", id, true, "deleted");
        Ok(())
    }

    pub fn set_enabled(&self, kind: &str, id: &str, enabled: bool) -> Result<(), ExtensionError> {
        if !matches!(kind, "skill" | "mcp" | "hook") || id.trim().is_empty() {
            return Err(ExtensionError::Config("invalid extension toggle".into()));
        }
        if kind == "skill" && self.is_robot_pack_skill_id(id) {
            if enabled {
                return Ok(());
            }
            return Err(ExtensionError::Config(
                "robot-pack Skills are required by built-in robots and cannot be disabled".into(),
            ));
        }
        self.projection
            .set_setting(
                &format!("extension/{kind}/{id}"),
                if enabled { "true" } else { "false" },
            )
            .map_err(|error| ExtensionError::Config(error.to_string()))?;
        self.record(
            "extension_toggled",
            kind,
            id,
            true,
            if enabled { "enabled" } else { "disabled" },
        );
        Ok(())
    }

    fn is_robot_pack_skill_id(&self, id: &str) -> bool {
        let id = id.trim();
        if !valid_skill_name(id) {
            return false;
        }
        let Some(root) = self.builtin_skills_root.as_deref() else {
            return false;
        };
        let direct = root.join(id).join("SKILL.md");
        let grouped = root.join("robot-pack").join(id).join("SKILL.md");
        (direct.is_file()
            && root
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("robot-pack")))
            || grouped.is_file()
    }

    pub fn save_secret(&self, server: &str, name: &str, value: &str) -> Result<(), ExtensionError> {
        validate_secret_identifier(server, name)?;
        self.secrets.set(server, name, value)?;
        self.record("credential_saved", "mcp", server, true, name);
        Ok(())
    }

    pub fn delete_secret(&self, server: &str, name: &str) -> Result<(), ExtensionError> {
        validate_secret_identifier(server, name)?;
        self.secrets.delete(server, name)?;
        self.record("credential_deleted", "mcp", server, true, name);
        Ok(())
    }

    fn enabled(&self, kind: &str, id: &str, default: bool) -> Result<bool, ExtensionError> {
        Ok(self
            .projection
            .setting(&format!("extension/{kind}/{id}"))
            .map_err(|error| ExtensionError::Config(error.to_string()))?
            .map(|value| value == "true")
            .unwrap_or(default))
    }

    fn update_overview(
        &self,
        config_paths: &[PathBuf],
        instructions: &[LoadedInstruction],
        skills: &[LoadedSkill],
        mcp_servers: Vec<McpDiagnostic>,
        hooks: &[HookConfig],
        error: Option<String>,
    ) {
        let overview = ExtensionOverview {
            schema_version: 1,
            config_paths: config_paths
                .iter()
                .map(|path| user_facing_path(path))
                .collect(),
            instructions: instructions
                .iter()
                .map(|value| value.source.clone())
                .collect(),
            skills: skills
                .iter()
                .map(|skill| SkillDiagnostic {
                    name: skill.metadata.name.clone(),
                    description: skill.metadata.description.clone(),
                    path: user_facing_path(&skill.path),
                    scope: skill.scope.clone(),
                    risk: skill.metadata.risk,
                    category: skill.metadata.category,
                    triggers: skill.metadata.triggers.clone(),
                    enabled: skill.enabled,
                    managed_by_robot: skill.robot_pack,
                })
                .collect(),
            mcp_servers,
            hooks: hooks
                .iter()
                .map(|hook| HookDiagnostic {
                    id: hook.id.clone(),
                    phase: format!("{:?}", hook.phase).to_lowercase(),
                    tool: hook.tool.clone(),
                    enabled: hook.enabled,
                })
                .collect(),
            audit: self.audit.lock().expect("audit lock poisoned").clone(),
            error,
        };
        *self.overview.write().expect("overview lock poisoned") = overview;
    }

    fn record(&self, event: &str, kind: &str, id: &str, success: bool, detail: &str) {
        let record = ExtensionAudit {
            timestamp_ms: crate::storage::now_ms(),
            event: event.into(),
            kind: kind.into(),
            id: id.into(),
            success,
            detail: detail.chars().take(1000).collect(),
        };
        if let Ok(mut audit) = self.audit.lock() {
            audit.push(record.clone());
            if audit.len() > MAX_AUDIT_RECORDS {
                let remove = audit.len() - MAX_AUDIT_RECORDS;
                audit.drain(..remove);
            }
        }
        if self
            .audit_path
            .metadata()
            .is_ok_and(|metadata| metadata.len() >= MAX_AUDIT_BYTES)
        {
            let previous = self.audit_path.with_extension("jsonl.1");
            let _ = fs::remove_file(&previous);
            let _ = fs::rename(&self.audit_path, previous);
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_path)
        {
            let _ = serde_json::to_writer(&mut file, &record);
            let _ = file.write_all(b"\n");
        }
        let _ = self.logger.log(
            if success { "info" } else { "error" },
            event,
            serde_json::json!({ "kind": kind, "id": id, "success": success, "detail": detail }),
        );
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillResourceReadArguments {
    skill_id: String,
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Clone)]
struct SkillResourceReadTool {
    service: ExtensionService,
}

#[async_trait]
impl ToolHandler for SkillResourceReadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "skill_resource_read".into(),
            description: "Read a bounded UTF-8 reference resource from the current effective enabled ordinary Skill. This tool only reads files and never executes scripts.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "skillId": { "type": "string", "minLength": 1, "maxLength": 64 },
                    "path": { "type": "string", "minLength": 1, "maxLength": 1024 },
                    "offset": { "type": "integer", "minimum": 0 },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_SKILL_RESOURCE_READ_BYTES
                    }
                },
                "required": ["skillId", "path"],
                "additionalProperties": false
            }),
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
        let arguments: SkillResourceReadArguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        self.service.read_skill_resource(
            &arguments.skill_id,
            &arguments.path,
            arguments.offset,
            arguments.limit,
        )
    }
}

fn merge_configs(
    extension_paths: &[PathBuf],
    mcp_paths: &[PathBuf],
) -> Result<ExtensionConfig, ExtensionError> {
    let mut servers = HashMap::<String, McpServerConfig>::new();
    let mut hooks = HashMap::<String, HookConfig>::new();
    let source_count = extension_paths.len().max(mcp_paths.len());
    for index in 0..source_count {
        if let Some(path) = extension_paths.get(index) {
            if let Some(config) = read_config(path)? {
                merge_mcp_servers(&mut servers, path, config.mcp_servers)?;
                let mut local_hooks = HashSet::new();
                for hook in config.hooks {
                    hook.validate().map_err(ExtensionError::Config)?;
                    if !local_hooks.insert(hook.id.clone()) {
                        return Err(ExtensionError::Config(format!(
                            "{} contains duplicate hook {}",
                            user_facing_path(path),
                            hook.id
                        )));
                    }
                    hooks.insert(hook.id.clone(), hook);
                }
            }
        }
        if let Some(path) = mcp_paths.get(index) {
            if let Some(config) = read_mcp_config(path)? {
                merge_mcp_servers(&mut servers, path, config.mcp_servers)?;
            }
        }
    }
    let mut mcp_servers = servers.into_values().collect::<Vec<_>>();
    let mut hooks = hooks.into_values().collect::<Vec<_>>();
    mcp_servers.sort_by(|left, right| left.id.cmp(&right.id));
    hooks.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(ExtensionConfig { mcp_servers, hooks })
}

fn merge_mcp_servers(
    servers: &mut HashMap<String, McpServerConfig>,
    path: &Path,
    values: Vec<McpServerConfig>,
) -> Result<(), ExtensionError> {
    let mut local_servers = HashSet::new();
    for server in values {
        server.validate()?;
        if !local_servers.insert(server.id.clone()) {
            return Err(ExtensionError::Config(format!(
                "{} contains duplicate MCP server {}",
                user_facing_path(path),
                server.id
            )));
        }
        servers.insert(server.id.clone(), server);
    }
    Ok(())
}

fn read_config(path: &Path) -> Result<Option<ExtensionConfig>, ExtensionError> {
    let Some(bytes) = read_config_bytes(path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| ExtensionError::Config(format!("{}: {error}", user_facing_path(path))))
}

fn read_mcp_config(path: &Path) -> Result<Option<McpConfigFile>, ExtensionError> {
    let Some(bytes) = read_config_bytes(path)? else {
        return Ok(None);
    };
    parse_mcp_config(&bytes, path).map(Some)
}

fn parse_mcp_config(bytes: &[u8], path: &Path) -> Result<McpConfigFile, ExtensionError> {
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(ExtensionError::Config(format!(
            "{} must be no larger than {MAX_CONFIG_BYTES} bytes",
            user_facing_path(path)
        )));
    }
    let config: McpConfigFile = serde_json::from_slice(bytes)
        .map_err(|error| ExtensionError::Config(format!("{}: {error}", user_facing_path(path))))?;
    let mut ids = HashSet::new();
    for server in &config.mcp_servers {
        server.validate()?;
        if !ids.insert(server.id.as_str()) {
            return Err(ExtensionError::Config(format!(
                "{} contains duplicate MCP server {}",
                user_facing_path(path),
                server.id
            )));
        }
    }
    Ok(config)
}

fn read_config_bytes(path: &Path) -> Result<Option<Vec<u8>>, ExtensionError> {
    if !path.exists() {
        return Ok(None);
    }
    let metadata = path
        .metadata()
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    if !metadata.is_file() || metadata.len() as usize > MAX_CONFIG_BYTES {
        return Err(ExtensionError::Config(format!(
            "{} must be a file no larger than {MAX_CONFIG_BYTES} bytes",
            user_facing_path(path)
        )));
    }
    fs::read(path)
        .map(Some)
        .map_err(|error| ExtensionError::Io(error.to_string()))
}

fn read_mcp_config_document(
    scope: &str,
    path: &Path,
) -> Result<McpConfigDocumentView, ExtensionError> {
    let Some(bytes) = read_config_bytes(path)? else {
        return Ok(McpConfigDocumentView {
            scope: scope.into(),
            path: user_facing_path(path),
            exists: false,
            content: default_mcp_config_content(),
            error: None,
        });
    };
    let content = String::from_utf8_lossy(&bytes).into_owned();
    let error = String::from_utf8(bytes.clone())
        .map_err(|_| {
            ExtensionError::Config(format!(
                "{} must contain UTF-8 JSON",
                user_facing_path(path)
            ))
        })
        .and_then(|_| parse_mcp_config(&bytes, path).map(|_| ()))
        .err()
        .map(|error| error.to_string());
    Ok(McpConfigDocumentView {
        scope: scope.into(),
        path: user_facing_path(path),
        exists: true,
        content,
        error,
    })
}

fn default_mcp_config_content() -> String {
    "{\n  \"mcpServers\": {}\n}\n".into()
}

fn resolve_scoped_config_path(
    root: &Path,
    relative: &Path,
    create_parent: bool,
) -> Result<PathBuf, ExtensionError> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(ExtensionError::Config(
            "MCP configuration path must remain inside its scope".into(),
        ));
    }
    let root = root
        .canonicalize()
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    let candidate = root.join(relative);
    if candidate.exists() {
        let canonical = candidate
            .canonicalize()
            .map_err(|error| ExtensionError::Io(error.to_string()))?;
        if !canonical.starts_with(&root) {
            return Err(ExtensionError::Config(format!(
                "{} escapes its configuration scope",
                user_facing_path(&candidate)
            )));
        }
        return Ok(candidate);
    }

    let parent = candidate
        .parent()
        .ok_or_else(|| ExtensionError::Config("configuration path has no parent".into()))?;
    let mut existing = parent;
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| {
            ExtensionError::Config("configuration parent cannot be resolved".into())
        })?;
    }
    let canonical_existing = existing
        .canonicalize()
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    if !canonical_existing.starts_with(&root) {
        return Err(ExtensionError::Config(format!(
            "{} escapes its configuration scope",
            user_facing_path(&candidate)
        )));
    }
    if create_parent {
        fs::create_dir_all(parent).map_err(|error| ExtensionError::Io(error.to_string()))?;
        let canonical_parent = parent
            .canonicalize()
            .map_err(|error| ExtensionError::Io(error.to_string()))?;
        if !canonical_parent.starts_with(&root) {
            return Err(ExtensionError::Config(format!(
                "{} escapes its configuration scope",
                user_facing_path(&candidate)
            )));
        }
    }
    Ok(candidate)
}

fn write_mcp_config(path: &Path, config: &McpConfigFile) -> Result<(), ExtensionError> {
    let mut serialized = serde_json::to_vec_pretty(config)
        .map_err(|error| ExtensionError::Config(error.to_string()))?;
    serialized.push(b'\n');
    let temporary = path.with_extension("json.tmp");
    let mut file =
        fs::File::create(&temporary).map_err(|error| ExtensionError::Io(error.to_string()))?;
    if let Err(error) = file.write_all(&serialized).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(ExtensionError::Io(error.to_string()));
    }
    #[cfg(target_os = "windows")]
    if path.exists() {
        fs::remove_file(path).map_err(|error| ExtensionError::Io(error.to_string()))?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ExtensionError::Io(error.to_string())
    })
}

fn validate_user_rule_id(id: &str) -> Result<(), ExtensionError> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| ExtensionError::Config("user rule id is invalid".into()))
}

fn validate_user_rule_input(
    title: &str,
    content: &str,
) -> Result<(String, String), ExtensionError> {
    let title = title.trim();
    if title.is_empty()
        || title.chars().count() > MAX_USER_RULE_TITLE_CHARS
        || title.chars().any(char::is_control)
    {
        return Err(ExtensionError::Config(format!(
            "user rule title must contain 1-{MAX_USER_RULE_TITLE_CHARS} visible characters"
        )));
    }
    if content.trim().is_empty() || content.len() > MAX_USER_RULE_BYTES {
        return Err(ExtensionError::Config(format!(
            "user rule content must contain 1-{MAX_USER_RULE_BYTES} UTF-8 bytes"
        )));
    }
    Ok((title.to_string(), content.to_string()))
}

fn validate_user_rules_file(file: &UserRulesFile, path: &Path) -> Result<(), ExtensionError> {
    if file.schema_version != USER_RULES_SCHEMA_VERSION {
        return Err(ExtensionError::Config(format!(
            "{} uses unsupported user rule schema version {}",
            user_facing_path(path),
            file.schema_version
        )));
    }
    if file.rules.len() > MAX_USER_RULES {
        return Err(ExtensionError::Config(format!(
            "{} contains more than {MAX_USER_RULES} user rules",
            user_facing_path(path)
        )));
    }
    let mut ids = HashSet::new();
    for rule in &file.rules {
        validate_user_rule_id(&rule.id)?;
        if !ids.insert(rule.id.as_str()) {
            return Err(ExtensionError::Config(format!(
                "{} contains duplicate user rule {}",
                user_facing_path(path),
                rule.id
            )));
        }
        let (title, _) = validate_user_rule_input(&rule.title, &rule.content)?;
        if title != rule.title {
            return Err(ExtensionError::Config(format!(
                "user rule {} title contains surrounding whitespace",
                rule.id
            )));
        }
        if rule.created_at_ms == 0
            || rule.updated_at_ms == 0
            || rule.updated_at_ms < rule.created_at_ms
        {
            return Err(ExtensionError::Config(format!(
                "user rule {} has invalid timestamps",
                rule.id
            )));
        }
    }
    Ok(())
}

fn read_user_rules_file(path: &Path) -> Result<UserRulesFile, ExtensionError> {
    let Some(bytes) = read_config_bytes(path)? else {
        return Ok(UserRulesFile::default());
    };
    if bytes.len() > MAX_USER_RULES_CONFIG_BYTES {
        return Err(ExtensionError::Config(format!(
            "{} must be no larger than {MAX_USER_RULES_CONFIG_BYTES} bytes",
            user_facing_path(path)
        )));
    }
    let file = serde_json::from_slice::<UserRulesFile>(&bytes)
        .map_err(|error| ExtensionError::Config(format!("{}: {error}", user_facing_path(path))))?;
    validate_user_rules_file(&file, path)?;
    Ok(file)
}

fn write_user_rules_file(path: &Path, file: &UserRulesFile) -> Result<(), ExtensionError> {
    validate_user_rules_file(file, path)?;
    let mut serialized = serde_json::to_vec_pretty(file)
        .map_err(|error| ExtensionError::Config(error.to_string()))?;
    serialized.push(b'\n');
    if serialized.len() > MAX_USER_RULES_CONFIG_BYTES {
        return Err(ExtensionError::Config(format!(
            "{} must be no larger than {MAX_USER_RULES_CONFIG_BYTES} bytes",
            user_facing_path(path)
        )));
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ExtensionError::Config("user rule path has no file name".into()))?;
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    if let Err(error) = output
        .write_all(&serialized)
        .and_then(|_| output.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(ExtensionError::Io(error.to_string()));
    }
    drop(output);

    #[cfg(target_os = "windows")]
    {
        let backup = path.with_file_name(format!(".{file_name}.{}.backup", uuid::Uuid::new_v4()));
        let had_existing = path.exists();
        if had_existing {
            fs::rename(path, &backup).map_err(|error| {
                let _ = fs::remove_file(&temporary);
                ExtensionError::Io(error.to_string())
            })?;
        }
        if let Err(error) = fs::rename(&temporary, path) {
            if had_existing {
                let _ = fs::rename(&backup, path);
            }
            let _ = fs::remove_file(&temporary);
            return Err(ExtensionError::Io(error.to_string()));
        }
        if had_existing {
            let _ = fs::remove_file(backup);
        }
    }
    #[cfg(not(target_os = "windows"))]
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ExtensionError::Io(error.to_string())
    })?;
    Ok(())
}

fn discover_instructions(
    data_root: &Path,
    workspace: &Path,
) -> Result<Vec<LoadedInstruction>, ExtensionError> {
    let workspace = workspace
        .canonicalize()
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    let mut paths = vec![(data_root.join("AGENTS.md"), "global".to_string(), 100)];
    let user_rules_path =
        resolve_scoped_config_path(data_root, Path::new(USER_RULES_CONFIG_FILE_NAME), false)?;
    let user_rules = read_user_rules_file(&user_rules_path)?;
    let mut user_rule_instructions = Vec::with_capacity(user_rules.rules.len());
    for (index, rule) in user_rules.rules.into_iter().enumerate() {
        let content = format!("# {}\n\n{}", rule.title, rule.content);
        user_rule_instructions.push(LoadedInstruction {
            source: InstructionSource {
                path: format!("{}#{}", user_facing_path(&user_rules_path), rule.id),
                scope: "user_rule".into(),
                priority: 110 + index as u32,
                bytes: content.len(),
            },
            content,
        });
    }
    paths.push((workspace.join("AGENTS.md"), "project".to_string(), 200));
    let rules = workspace.join(".k-coder").join("rules");
    if rules.exists() {
        let canonical_rules = rules
            .canonicalize()
            .map_err(|error| ExtensionError::Io(error.to_string()))?;
        if !canonical_rules.starts_with(&workspace) {
            return Err(ExtensionError::Config(
                "project rule directory escapes the workspace".into(),
            ));
        }
        let mut rule_paths = fs::read_dir(&canonical_rules)
            .map_err(|error| ExtensionError::Io(error.to_string()))?
            .map(|entry| entry.map(|value| value.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ExtensionError::Io(error.to_string()))?;
        rule_paths.sort();
        for (index, path) in rule_paths.into_iter().enumerate() {
            if path.extension().and_then(|value| value.to_str()) == Some("md") {
                paths.push((path, "project_rule".into(), 300 + index as u32));
            }
        }
    }
    let mut result = Vec::new();
    for (path, scope, priority) in paths {
        if !path.exists() {
            continue;
        }
        let content = read_bounded_utf8(&path, MAX_INSTRUCTION_FILE_BYTES)?;
        if content.trim().is_empty() {
            return Err(ExtensionError::Config(format!(
                "instruction file {} is empty",
                user_facing_path(&path)
            )));
        }
        result.push(LoadedInstruction {
            source: InstructionSource {
                path: user_facing_path(&path),
                scope,
                priority,
                bytes: content.len(),
            },
            content,
        });
    }
    result.extend(user_rule_instructions);
    result.sort_by_key(|instruction| instruction.source.priority);
    Ok(result)
}

fn discover_skills(
    builtin_skills_root: Option<&Path>,
    data_root: &Path,
    workspace: &Path,
    projection: &ProjectionDb,
) -> Result<Vec<LoadedSkill>, ExtensionError> {
    let mut roots = Vec::with_capacity(3);
    if let Some(root) = builtin_skills_root {
        if !root.is_dir() {
            return Err(ExtensionError::Skill(format!(
                "built-in Skill root {} is missing or is not a directory",
                user_facing_path(root)
            )));
        }
        roots.push((root.to_path_buf(), "builtin"));
    }
    roots.extend([
        (data_root.join("skills"), "global"),
        (workspace.join(".k-coder").join("skills"), "project"),
    ]);
    let mut selected = HashMap::<String, LoadedSkill>::new();
    for (root, scope) in roots {
        if !root.exists() {
            continue;
        }
        reject_skill_link_or_reparse(&root)?;
        let canonical_root = root
            .canonicalize()
            .map_err(|error| ExtensionError::Io(error.to_string()))?;
        let mut directories = fs::read_dir(&canonical_root)
            .map_err(|error| ExtensionError::Io(error.to_string()))?
            .map(|entry| entry.map(|value| value.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ExtensionError::Io(error.to_string()))?;
        directories.sort();
        let mut scope_ids = HashSet::<String>::new();
        for directory in directories {
            reject_skill_link_or_reparse(&directory)?;
            if !directory.is_dir() {
                continue;
            }
            let file = directory.join("SKILL.md");
            if file.exists() {
                load_skill_candidate(
                    &canonical_root,
                    &directory,
                    &file,
                    scope,
                    projection,
                    &mut scope_ids,
                    &mut selected,
                )?;
                continue;
            }

            let canonical_group = directory
                .canonicalize()
                .map_err(|error| ExtensionError::Io(error.to_string()))?;
            if !canonical_group.starts_with(&canonical_root) {
                return Err(ExtensionError::Skill(format!(
                    "{} escapes the Skill root",
                    user_facing_path(&directory)
                )));
            }
            let mut grouped_directories = fs::read_dir(&canonical_group)
                .map_err(|error| ExtensionError::Io(error.to_string()))?
                .map(|entry| entry.map(|value| value.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| ExtensionError::Io(error.to_string()))?;
            grouped_directories.sort();
            for grouped_directory in grouped_directories {
                reject_skill_link_or_reparse(&grouped_directory)?;
                if !grouped_directory.is_dir() {
                    continue;
                }
                let file = grouped_directory.join("SKILL.md");
                if !file.exists() {
                    continue;
                }
                load_skill_candidate(
                    &canonical_root,
                    &grouped_directory,
                    &file,
                    scope,
                    projection,
                    &mut scope_ids,
                    &mut selected,
                )?;
            }
        }
    }
    let mut skills = selected.into_values().collect::<Vec<_>>();
    skills.sort_by(|left, right| left.metadata.name.cmp(&right.metadata.name));
    Ok(skills)
}

fn load_skill_candidate(
    canonical_root: &Path,
    directory: &Path,
    file: &Path,
    scope: &str,
    projection: &ProjectionDb,
    scope_ids: &mut HashSet<String>,
    selected: &mut HashMap<String, LoadedSkill>,
) -> Result<(), ExtensionError> {
    reject_skill_link_or_reparse(directory)?;
    reject_skill_link_or_reparse(file)?;
    let canonical = file
        .canonicalize()
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    if !canonical.starts_with(canonical_root) {
        return Err(ExtensionError::Skill(format!(
            "{} escapes the Skill root",
            user_facing_path(file)
        )));
    }
    let robot_pack = scope == "builtin" && is_robot_pack_directory(canonical_root, directory);
    let (content, definition_identity) =
        read_bounded_skill_utf8_with_identity(&canonical, MAX_SKILL_BYTES)?;
    let (metadata, body) = parse_skill(&content, &canonical)?;
    let directory_name = directory
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if metadata.name != directory_name || !valid_skill_name(&metadata.name) {
        return Err(ExtensionError::Skill(format!(
            "{} name must match its directory and use lowercase kebab-case",
            user_facing_path(&canonical)
        )));
    }
    if !scope_ids.insert(metadata.name.clone()) {
        return Err(ExtensionError::Skill(format!(
            "duplicate Skill {} in {scope} scope",
            metadata.name
        )));
    }
    let override_enabled = projection
        .setting(&format!("extension/skill/{}", metadata.name))
        .map_err(|error| ExtensionError::Config(error.to_string()))?;
    let enabled = if robot_pack {
        // Robot-pack Skills are part of a built-in workflow contract. A
        // persisted ordinary toggle or frontmatter value must not disable it.
        true
    } else {
        match metadata.risk {
            ToolRisk::Read => override_enabled
                .map(|value| value == "true")
                .unwrap_or(metadata.enabled),
            ToolRisk::Write | ToolRisk::Delete | ToolRisk::External => {
                override_enabled.as_deref() == Some("true")
            }
        }
    };
    let resource_root = SkillResourceRoot::open(
        canonical_root,
        canonical.parent().ok_or_else(|| {
            ExtensionError::Skill("Skill definition has no parent directory".into())
        })?,
    )?;
    let loaded = LoadedSkill {
        metadata,
        path: canonical,
        resource_root,
        definition_identity,
        scope: scope.into(),
        robot_pack,
        body,
        enabled,
    };
    let should_replace = selected
        .get(&loaded.metadata.name)
        .is_none_or(|existing| !existing.robot_pack || loaded.robot_pack);
    if should_replace {
        selected.insert(loaded.metadata.name.clone(), loaded);
    }
    Ok(())
}

fn is_robot_pack_directory(canonical_root: &Path, directory: &Path) -> bool {
    if canonical_root
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("robot-pack"))
    {
        return true;
    }
    directory
        .strip_prefix(canonical_root)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| {
            matches!(component, Component::Normal(value) if value.to_string_lossy().eq_ignore_ascii_case("robot-pack"))
        })
}

fn reject_skill_link_or_reparse(path: &Path) -> Result<(), ExtensionError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| ExtensionError::Io(error.to_string()))?;
    if metadata.file_type().is_symlink() || skill_metadata_is_reparse_point(&metadata) {
        return Err(ExtensionError::Skill(format!(
            "Skill path {} must not contain a symbolic link or directory junction",
            user_facing_path(path)
        )));
    }
    Ok(())
}

fn validate_skill_resource_path(raw_path: &str) -> Result<PathBuf, ToolError> {
    let path = Path::new(raw_path);
    if raw_path.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ToolError::InvalidArguments(
            "Skill resource path must be a non-empty relative path without parent traversal".into(),
        ));
    }
    if path.components().count() == 1 && raw_path.eq_ignore_ascii_case("SKILL.md") {
        return Err(ToolError::InvalidArguments(
            "Skill definition files cannot be read as persistent resources".into(),
        ));
    }
    #[cfg(windows)]
    if path.components().any(|component| {
        matches!(component, Component::Normal(value) if value.to_string_lossy().contains(':'))
    }) {
        return Err(ToolError::InvalidArguments(
            "Skill resource path components must not contain Windows stream separators".into(),
        ));
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
type SkillResourceBeforeOpenHook = Arc<dyn Fn(&Path) + Send + Sync>;

#[cfg(test)]
fn skill_resource_before_open_hook() -> &'static Mutex<Option<SkillResourceBeforeOpenHook>> {
    static HOOK: std::sync::OnceLock<Mutex<Option<SkillResourceBeforeOpenHook>>> =
        std::sync::OnceLock::new();
    HOOK.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
fn set_skill_resource_before_open_hook(hook: Option<SkillResourceBeforeOpenHook>) {
    *skill_resource_before_open_hook()
        .lock()
        .expect("Skill resource hook lock poisoned") = hook;
}

#[cfg(test)]
fn run_skill_resource_before_open_hook(root: &Path) {
    let hook = skill_resource_before_open_hook()
        .lock()
        .expect("Skill resource hook lock poisoned")
        .clone();
    if let Some(hook) = hook {
        hook(root);
    }
}

fn resolve_skill_resource_path(root: &Path, relative: &Path) -> Result<PathBuf, ToolError> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| ToolError::Execution(error.to_string()))?;
    let mut current = canonical_root.clone();
    reject_skill_resource_link(&current)?;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(ToolError::InvalidArguments(
                "Skill resource path contains an unsupported component".into(),
            ));
        };
        current.push(component);
        reject_skill_resource_link(&current)?;
    }
    let canonical = current
        .canonicalize()
        .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(ToolError::InvalidArguments(
            "Skill resource path escapes its Skill root".into(),
        ));
    }
    Ok(canonical)
}

fn reject_skill_resource_link(path: &Path) -> Result<(), ToolError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
    if metadata.file_type().is_symlink() || skill_metadata_is_reparse_point(&metadata) {
        return Err(ToolError::InvalidArguments(format!(
            "Skill resource path {} must not contain a symbolic link or directory junction",
            user_facing_path(path)
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn skill_metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn skill_metadata_is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn parse_skill(content: &str, path: &Path) -> Result<(SkillMetadata, String), ExtensionError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let body = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .ok_or_else(|| {
            ExtensionError::Skill(format!(
                "{} must start with YAML frontmatter",
                user_facing_path(path)
            ))
        })?;
    let mut offset = 0;
    let mut sections = None;
    for line in body.split_inclusive('\n') {
        let delimiter = line.strip_suffix('\n').unwrap_or(line);
        let delimiter = delimiter.strip_suffix('\r').unwrap_or(delimiter);
        if delimiter == "---" {
            sections = Some((&body[..offset], &body[offset + line.len()..]));
            break;
        }
        offset += line.len();
    }
    let (frontmatter, body) = sections.ok_or_else(|| {
        ExtensionError::Skill(format!(
            "{} frontmatter is not closed",
            user_facing_path(path)
        ))
    })?;
    let metadata: SkillMetadata = serde_yaml::from_str(frontmatter).map_err(|error| {
        ExtensionError::Skill(format!(
            "{} metadata is invalid: {error}",
            user_facing_path(path)
        ))
    })?;
    if metadata.description.trim().is_empty()
        || metadata.description.len() > 512
        || metadata.triggers.is_empty()
        || metadata.triggers.len() > 32
        || metadata
            .triggers
            .iter()
            .any(|trigger| trigger.trim().is_empty() || trigger.len() > 120)
        || body.trim().is_empty()
    {
        return Err(ExtensionError::Skill(format!(
            "{} metadata or instructions violate bounded Skill rules",
            user_facing_path(path)
        )));
    }
    Ok((metadata, normalize_skill_body(body)))
}

fn resolved_ordinary_skill(skill: &LoadedSkill) -> ResolvedSkill {
    ResolvedSkill {
        id: skill.metadata.name.clone(),
        source: ResolvedSkillSource::Ordinary {
            scope: skill.scope.clone(),
        },
        risk: skill.metadata.risk,
        enabled: skill.enabled,
        body: skill.body.clone(),
        bytes: skill.body.len(),
        sha256: sha256_hex(skill.body.as_bytes()),
    }
}

fn normalize_skill_body(body: &str) -> String {
    body.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_bounded_skill_utf8_with_identity(
    path: &Path,
    limit: usize,
) -> Result<(String, SkillFileIdentity), ExtensionError> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    #[cfg(windows)]
    use std::os::windows::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    #[cfg(windows)]
    options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    let mut file = options
        .open(path)
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    if !metadata.is_file()
        || skill_metadata_is_reparse_point(&metadata)
        || metadata.len() > limit as u64
    {
        return Err(ExtensionError::Config(format!(
            "{} must be a real file no larger than {limit} bytes",
            user_facing_path(path)
        )));
    }
    let identity = skill_file_identity(&file).map_err(ExtensionError::Io)?;
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(limit));
    Read::by_ref(&mut file)
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    if bytes.len() > limit {
        return Err(ExtensionError::Config(format!(
            "{} must be a file no larger than {limit} bytes",
            user_facing_path(path)
        )));
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| ExtensionError::Config(format!("{} must be UTF-8", user_facing_path(path))))?;
    Ok((content, identity))
}

fn read_bounded_utf8(path: &Path, limit: usize) -> Result<String, ExtensionError> {
    let metadata = path
        .metadata()
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    if !metadata.is_file() || metadata.len() as usize > limit {
        return Err(ExtensionError::Config(format!(
            "{} must be a file no larger than {limit} bytes",
            user_facing_path(path)
        )));
    }
    let bytes = fs::read(path).map_err(|error| ExtensionError::Io(error.to_string()))?;
    String::from_utf8(bytes)
        .map_err(|_| ExtensionError::Config(format!("{} must be UTF-8", user_facing_path(path))))
}

fn user_facing_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(path) = path.strip_prefix(r"\\?\") {
            return path.to_string();
        }
    }
    path.into_owned()
}

fn valid_skill_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn validate_secret_identifier(server: &str, name: &str) -> Result<(), ExtensionError> {
    let valid = |value: &str| {
        !value.is_empty()
            && value.len() <= 128
            && value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
            })
    };
    if !valid(server) || !valid(name) {
        return Err(ExtensionError::Config(
            "MCP server and credential names contain invalid characters".into(),
        ));
    }
    Ok(())
}

fn load_audit(path: &Path) -> Vec<ExtensionAudit> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .rev()
        .take(MAX_AUDIT_RECORDS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn collect_extension_files(root: &Path, paths: &mut Vec<PathBuf>) -> Result<(), ExtensionError> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| ExtensionError::Io(error.to_string()))? {
        let path = entry
            .map_err(|error| ExtensionError::Io(error.to_string()))?
            .path();
        if path.is_dir() {
            for child in
                fs::read_dir(&path).map_err(|error| ExtensionError::Io(error.to_string()))?
            {
                let child = child
                    .map_err(|error| ExtensionError::Io(error.to_string()))?
                    .path();
                if child.file_name().and_then(|value| value.to_str()) == Some("SKILL.md") {
                    paths.push(child);
                }
            }
        } else if path.extension().and_then(|value| value.to_str()) == Some("md") {
            paths.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn tool_context(workspace: &Path) -> crate::tools::ToolContext {
        crate::tools::ToolContext {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            call_id: "call-1".into(),
            workspace_root: workspace.to_path_buf(),
            approval: None,
            progress: None,
        }
    }

    async fn prepared_skill_resource_handler(
        service: &ExtensionService,
        workspace: &Path,
    ) -> Arc<dyn ToolHandler> {
        service
            .prepare(workspace, CancellationToken::new())
            .await
            .unwrap()
            .handlers
            .into_iter()
            .find(|handler| handler.definition().name == "skill_resource_read")
            .expect("ordinary Skill resource handler")
    }

    fn write_test_skill(root: &Path, name: &str, body: &str) {
        let directory = root.join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Review code\ntriggers: [review]\nrisk: read\nenabled: true\n---\n{body}"
            ),
        )
        .unwrap();
    }

    fn write_grouped_test_skill(root: &Path, group: &str, name: &str, body: &str) {
        write_test_skill(&root.join(group), name, body);
    }

    fn write_test_plugin(data_root: &Path, folder: &str, name: &str) -> PathBuf {
        let plugin_root = data_root.join("plugins").join(folder);
        fs::create_dir_all(plugin_root.join(".codex-plugin")).unwrap();
        fs::write(
            plugin_root.join(".codex-plugin/plugin.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "name": name,
                "version": "1.0.0",
                "description": "Test plugin"
            }))
            .unwrap(),
        )
        .unwrap();
        let skill_root = plugin_root.join("skills/review");
        fs::create_dir_all(&skill_root).unwrap();
        fs::write(
            skill_root.join("SKILL.md"),
            "---\nname: review\ndescription: Review from a local plugin\n---\nPLUGIN-REVIEW-BODY",
        )
        .unwrap();
        plugin_root
    }

    #[test]
    fn project_instructions_override_global_and_rules_are_last() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        fs::write(data.path().join("AGENTS.md"), "global").unwrap();
        fs::write(workspace.path().join("AGENTS.md"), "project").unwrap();
        fs::create_dir_all(workspace.path().join(".k-coder/rules")).unwrap();
        fs::write(workspace.path().join(".k-coder/rules/10-final.md"), "rule").unwrap();
        let values = discover_instructions(data.path(), workspace.path()).unwrap();
        assert_eq!(
            values
                .iter()
                .map(|value| value.source.priority)
                .collect::<Vec<_>>(),
            vec![100, 200, 300]
        );
    }

    #[test]
    fn user_rules_are_ordered_between_global_and_project_instructions() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );
        service
            .save_user_rule(SaveUserRuleRequest {
                id: None,
                title: "注释规则".into(),
                content: "公共方法需要注释".into(),
            })
            .unwrap();
        service
            .save_user_rule(SaveUserRuleRequest {
                id: None,
                title: "审批规则".into(),
                content: "审批通过后结束流程".into(),
            })
            .unwrap();
        fs::write(data.path().join("AGENTS.md"), "global").unwrap();
        fs::write(workspace.path().join("AGENTS.md"), "project").unwrap();
        fs::create_dir_all(workspace.path().join(".k-coder/rules")).unwrap();
        fs::write(
            workspace.path().join(".k-coder/rules/10-final.md"),
            "project rule",
        )
        .unwrap();

        let values = discover_instructions(data.path(), workspace.path()).unwrap();

        assert_eq!(
            values
                .iter()
                .map(|value| value.source.priority)
                .collect::<Vec<_>>(),
            vec![100, 110, 111, 200, 300]
        );
        assert_eq!(values[1].source.scope, "user_rule");
        assert_eq!(values[1].content, "# 注释规则\n\n公共方法需要注释");
        assert_eq!(values[2].content, "# 审批规则\n\n审批通过后结束流程");
    }

    #[test]
    fn user_rules_can_be_created_edited_deleted_and_are_strictly_bounded() {
        let data = tempfile::tempdir().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );
        service
            .save_user_rule(SaveUserRuleRequest {
                id: None,
                title: "方法注释".into(),
                content: "公共方法必须包含注释。".into(),
            })
            .unwrap();
        let created = service.user_rules_view().unwrap();
        assert_eq!(created.schema_version, USER_RULES_SCHEMA_VERSION);
        assert_eq!(created.rules.len(), 1);
        let id = created.rules[0].id.clone();
        assert!(uuid::Uuid::parse_str(&id).is_ok());

        service
            .save_user_rule(SaveUserRuleRequest {
                id: Some(id.clone()),
                title: "实体注释".into(),
                content: "实体缺少注释时需要提醒。".into(),
            })
            .unwrap();
        let updated = service.user_rules_view().unwrap();
        assert_eq!(updated.rules[0].title, "实体注释");
        assert_eq!(updated.rules[0].content, "实体缺少注释时需要提醒。");
        assert_eq!(
            updated.rules[0].created_at_ms,
            created.rules[0].created_at_ms
        );
        assert!(updated.rules[0].updated_at_ms >= updated.rules[0].created_at_ms);

        let oversized = service
            .save_user_rule(SaveUserRuleRequest {
                id: Some(id.clone()),
                title: "超限".into(),
                content: "x".repeat(MAX_USER_RULE_BYTES + 1),
            })
            .unwrap_err();
        assert!(oversized.to_string().contains("UTF-8 bytes"));
        assert_eq!(
            service.user_rules_view().unwrap().rules[0].title,
            "实体注释"
        );

        service.delete_user_rule(&id).unwrap();
        assert!(service.user_rules_view().unwrap().rules.is_empty());
        let missing = service.delete_user_rule(&id).unwrap_err();
        assert!(missing.to_string().contains("no longer exists"));
    }

    #[test]
    fn user_rule_configuration_rejects_link_escape() {
        let data = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join(USER_RULES_CONFIG_FILE_NAME);
        fs::write(&outside_file, r#"{"schemaVersion":1,"rules":[]}"#).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_file, data.path().join(USER_RULES_CONFIG_FILE_NAME))
            .unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(
            &outside_file,
            data.path().join(USER_RULES_CONFIG_FILE_NAME),
        )
        .is_err()
        {
            return;
        }
        let logger = StructuredLogger::new(data.path()).unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );

        let error = service.user_rules_view().unwrap_err();

        assert!(
            error
                .to_string()
                .contains("escapes its configuration scope")
        );
    }

    #[test]
    fn builtin_global_and_project_skills_have_deterministic_precedence() {
        let builtin = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let global_root = data.path().join("skills");
        let project_root = workspace.path().join(".k-coder/skills");
        write_test_skill(builtin.path(), "review", "BUILTIN-INSTRUCTIONS");
        write_test_skill(&global_root, "review", "GLOBAL-INSTRUCTIONS");
        write_test_skill(&project_root, "review", "PROJECT-INSTRUCTIONS");
        let projection = ProjectionDb::memory().unwrap();

        let skills = discover_skills(
            Some(builtin.path()),
            data.path(),
            workspace.path(),
            &projection,
        )
        .unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].scope, "project");
        assert_eq!(skills[0].body, "PROJECT-INSTRUCTIONS");

        fs::remove_dir_all(project_root.join("review")).unwrap();
        let skills = discover_skills(
            Some(builtin.path()),
            data.path(),
            workspace.path(),
            &projection,
        )
        .unwrap();
        assert_eq!(skills[0].scope, "global");
        assert_eq!(skills[0].body, "GLOBAL-INSTRUCTIONS");

        fs::remove_dir_all(global_root.join("review")).unwrap();
        let skills = discover_skills(
            Some(builtin.path()),
            data.path(),
            workspace.path(),
            &projection,
        )
        .unwrap();
        assert_eq!(skills[0].scope, "builtin");
        assert_eq!(skills[0].body, "BUILTIN-INSTRUCTIONS");
    }

    #[tokio::test]
    async fn resolved_ordinary_skill_uses_the_effective_project_scope() {
        let builtin = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(builtin.path(), "review", "BUILTIN-INSTRUCTIONS");
        write_test_skill(&data.path().join("skills"), "review", "GLOBAL-INSTRUCTIONS");
        write_test_skill(
            &workspace.path().join(".k-coder/skills"),
            "review",
            "PROJECT-INSTRUCTIONS",
        );
        let service = ExtensionService::with_builtin_skills(
            data.path().into(),
            Some(builtin.path().into()),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();

        let resolved = service.resolve_ordinary_skill("review").unwrap();

        assert_eq!(resolved.id, "review");
        assert_eq!(
            resolved.source,
            ResolvedSkillSource::Ordinary {
                scope: "project".into()
            }
        );
        assert_eq!(resolved.risk, ToolRisk::Read);
        assert!(resolved.enabled);
        assert_eq!(resolved.body, "PROJECT-INSTRUCTIONS");
        assert_eq!(resolved.bytes, 20);
    }

    #[tokio::test]
    async fn robot_pack_skills_are_reserved_enabled_and_not_overridable() {
        let builtin = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(
            &builtin.path().join("robot-pack"),
            "review",
            "ROBOT-CONTRACT",
        );
        write_test_skill(&data.path().join("skills"), "review", "GLOBAL-OVERRIDE");
        write_test_skill(
            &workspace.path().join(".k-coder/skills"),
            "review",
            "PROJECT-OVERRIDE",
        );
        let projection = ProjectionDb::memory().unwrap();
        projection
            .set_setting("extension/skill/review", "false")
            .unwrap();
        let service = ExtensionService::with_builtin_skills(
            data.path().into(),
            Some(builtin.path().into()),
            projection.clone(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();

        let resolved = service.resolve_robot_skill("review").unwrap();
        assert_eq!(resolved.body, "ROBOT-CONTRACT");
        assert!(resolved.enabled);
        assert_eq!(
            resolved.source,
            ResolvedSkillSource::Ordinary {
                scope: "builtin".into()
            }
        );
        let effective = service.resolve_ordinary_skill("review").unwrap();
        assert_eq!(effective.body, "ROBOT-CONTRACT");
        assert!(effective.enabled);

        let error = service.set_enabled("skill", "review", false).unwrap_err();
        assert!(error.to_string().contains("cannot be disabled"));
        service.set_enabled("skill", "review", true).unwrap();
        assert_eq!(
            projection
                .setting("extension/skill/review")
                .unwrap()
                .as_deref(),
            Some("false")
        );

        let diagnostic = service
            .overview()
            .skills
            .into_iter()
            .find(|skill| skill.name == "review")
            .unwrap();
        assert!(diagnostic.managed_by_robot);
        assert!(diagnostic.enabled);
    }

    #[tokio::test]
    async fn enabled_plugin_skill_is_preferred_over_its_ordinary_fallback() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(&data.path().join("skills"), "review", "FALLBACK-BODY");
        write_test_plugin(data.path(), "review-package", "review-tools");
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        service.plugin_overview(true).unwrap();
        service
            .set_plugin_enabled("review-tools@local", true)
            .unwrap();
        service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();

        let resolved = service
            .resolve_plugin_skill("review-tools@local", "review", Some("review"))
            .unwrap();

        assert_eq!(resolved.id, "review");
        assert_eq!(
            resolved.source,
            ResolvedSkillSource::Plugin {
                plugin_id: "review-tools@local".into()
            }
        );
        assert_eq!(resolved.body, "PLUGIN-REVIEW-BODY");
        assert!(resolved.enabled);
    }

    #[tokio::test]
    async fn missing_or_disabled_plugin_skill_uses_only_the_explicit_fallback() {
        for create_disabled_plugin in [false, true] {
            let data = tempfile::tempdir().unwrap();
            let workspace = tempfile::tempdir().unwrap();
            write_test_skill(&data.path().join("skills"), "review", "FALLBACK-BODY");
            if create_disabled_plugin {
                write_test_plugin(data.path(), "review-package", "review-tools");
            }
            let service = ExtensionService::new(
                data.path().into(),
                ProjectionDb::memory().unwrap(),
                Arc::new(mcp::OsMcpSecretStore::new()),
                StructuredLogger::new(data.path()).unwrap(),
            );
            service
                .prepare(workspace.path(), CancellationToken::new())
                .await
                .unwrap();

            let resolved = service
                .resolve_plugin_skill("review-tools@local", "review", Some("review"))
                .unwrap();

            assert_eq!(
                resolved.source,
                ResolvedSkillSource::OrdinaryFallback {
                    plugin_id: "review-tools@local".into(),
                    scope: "global".into(),
                }
            );
            assert_eq!(resolved.body, "FALLBACK-BODY");
            assert!(resolved.enabled);
        }
    }

    #[tokio::test]
    async fn unavailable_plugin_skill_without_fallback_fails_closed() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();

        let error = service
            .resolve_plugin_skill("review-tools@local", "review", None)
            .unwrap_err();

        assert!(error.to_string().contains("no ordinary fallback"));
    }

    #[tokio::test]
    async fn equal_normalized_ordinary_and_plugin_bodies_have_the_same_sha256() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(
            &data.path().join("skills"),
            "review",
            "\r\nSAME-INSTRUCTIONS\r\n",
        );
        let plugin = write_test_plugin(data.path(), "review-package", "review-tools");
        fs::write(
            plugin.join("skills/review/SKILL.md"),
            "---\r\nname: review\r\ndescription: Review\r\n---\r\nSAME-INSTRUCTIONS\r\n",
        )
        .unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        service.plugin_overview(true).unwrap();
        service
            .set_plugin_enabled("review-tools@local", true)
            .unwrap();
        service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();

        let ordinary = service.resolve_ordinary_skill("review").unwrap();
        let plugin = service
            .resolve_plugin_skill("review-tools@local", "review", None)
            .unwrap();

        assert_eq!(ordinary.body, "SAME-INSTRUCTIONS");
        assert_eq!(ordinary.sha256, plugin.sha256);
        assert_eq!(
            ordinary.sha256,
            "52fecfbec62ca42116258d60a662c76fc7891a95b41709fe1816343dc7e9f495"
        );
    }

    #[tokio::test]
    async fn resolved_skill_body_is_absent_from_debug_and_serialized_diagnostics() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(
            &data.path().join("skills"),
            "review",
            "UNIQUE-PRIVATE-SKILL-BODY",
        );
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();

        let resolved = service.resolve_ordinary_skill("review").unwrap();
        let loaded_debug = format!(
            "{:?}",
            service.skills.read().expect("skill lock poisoned")[0]
        );
        let debug = format!("{resolved:?}");
        let diagnostics = serde_json::to_string(&service.overview()).unwrap();

        assert!(!loaded_debug.contains("UNIQUE-PRIVATE-SKILL-BODY"));
        assert!(!debug.contains("UNIQUE-PRIVATE-SKILL-BODY"));
        assert!(!diagnostics.contains("UNIQUE-PRIVATE-SKILL-BODY"));
        assert!(diagnostics.contains("review"));
    }

    #[tokio::test]
    async fn skill_resource_read_returns_a_bounded_utf8_range() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let skill = data.path().join("skills/review");
        write_test_skill(&data.path().join("skills"), "review", "REVIEW");
        fs::create_dir_all(skill.join("references")).unwrap();
        fs::write(skill.join("references/guide.md"), "0123456789").unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        let handler = prepared_skill_resource_handler(&service, workspace.path()).await;

        let result = handler
            .execute(
                &tool_context(workspace.path()),
                serde_json::json!({
                    "skillId": "review",
                    "path": "references/guide.md",
                    "offset": 2,
                    "limit": 4
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.output, "2345");
        assert_eq!(result.metadata["skillId"], "review");
        assert_eq!(result.metadata["path"], "references/guide.md");
        assert_eq!(result.metadata["offset"], 2);
        assert_eq!(result.metadata["bytesReturned"], 4);
        assert_eq!(result.metadata["totalBytes"], 10);
        assert_eq!(result.metadata["truncated"], true);
    }

    #[tokio::test]
    async fn skill_resource_read_rejects_unknown_and_disabled_skills() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let projection = ProjectionDb::memory().unwrap();
        write_test_skill(&data.path().join("skills"), "review", "REVIEW");
        projection
            .set_setting("extension/skill/review", "false")
            .unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            projection,
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        let handler = prepared_skill_resource_handler(&service, workspace.path()).await;

        for skill_id in ["missing", "review"] {
            let error = handler
                .execute(
                    &tool_context(workspace.path()),
                    serde_json::json!({ "skillId": skill_id, "path": "SKILL.md" }),
                    CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert!(matches!(error, ToolError::Denied(_)));
        }
    }

    #[tokio::test]
    async fn skill_resource_read_rejects_absolute_parent_and_directory_paths() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_test_skill(&data.path().join("skills"), "review", "REVIEW");
        let skill = data.path().join("skills/review");
        fs::create_dir_all(skill.join("references")).unwrap();
        fs::write(outside.path().join("secret.md"), "SECRET").unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        let handler = prepared_skill_resource_handler(&service, workspace.path()).await;

        for path in [
            outside
                .path()
                .join("secret.md")
                .to_string_lossy()
                .into_owned(),
            "../secret.md".into(),
            "references".into(),
            "SKILL.md".into(),
            "skill.MD".into(),
        ] {
            let error = handler
                .execute(
                    &tool_context(workspace.path()),
                    serde_json::json!({ "skillId": "review", "path": path }),
                    CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert!(matches!(error, ToolError::InvalidArguments(_)));
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn skill_resource_read_rejects_windows_definition_ads_aliases() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(
            &data.path().join("skills"),
            "review",
            "UNIQUE-PRIVATE-SKILL-BODY",
        );
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        let handler = prepared_skill_resource_handler(&service, workspace.path()).await;

        let validation_error = validate_skill_resource_path("SKILL.md::$DATA").unwrap_err();
        assert!(matches!(
            validation_error,
            ToolError::Denied(_) | ToolError::InvalidArguments(_)
        ));
        let error = handler
            .execute(
                &tool_context(workspace.path()),
                serde_json::json!({
                    "skillId": "review",
                    "path": "SKILL.md::$DATA"
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ToolError::Denied(_) | ToolError::InvalidArguments(_)
        ));
        assert!(!error.to_string().contains("UNIQUE-PRIVATE-SKILL-BODY"));
    }

    #[tokio::test]
    async fn skill_resource_read_rejects_a_definition_file_hard_link() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(
            &data.path().join("skills"),
            "review",
            "UNIQUE-PRIVATE-SKILL-BODY",
        );
        let skill = data.path().join("skills/review");
        fs::create_dir_all(skill.join("references")).unwrap();
        fs::hard_link(
            skill.join("SKILL.md"),
            skill.join("references/definition-alias.md"),
        )
        .unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        let handler = prepared_skill_resource_handler(&service, workspace.path()).await;

        let error = handler
            .execute(
                &tool_context(workspace.path()),
                serde_json::json!({
                    "skillId": "review",
                    "path": "references/definition-alias.md"
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, ToolError::InvalidArguments(_)));
        assert!(!error.to_string().contains("UNIQUE-PRIVATE-SKILL-BODY"));
    }

    #[tokio::test]
    async fn skill_resource_read_rejects_binary_resources() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(&data.path().join("skills"), "review", "REVIEW");
        let skill = data.path().join("skills/review");
        fs::write(skill.join("binary.dat"), [0xff, 0xfe]).unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        let handler = prepared_skill_resource_handler(&service, workspace.path()).await;

        let error = handler
            .execute(
                &tool_context(workspace.path()),
                serde_json::json!({ "skillId": "review", "path": "binary.dat" }),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, ToolError::Execution(_)));
        assert!(error.to_string().contains("UTF-8"));
    }

    #[tokio::test]
    async fn skill_resource_read_rejects_link_or_reparse_escape() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_test_skill(&data.path().join("skills"), "review", "REVIEW");
        fs::write(outside.path().join("secret.md"), "SECRET").unwrap();
        let linked = data.path().join("skills/review/linked");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &linked).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(outside.path(), &linked).is_err() {
            let linked = linked.to_string_lossy().replace('/', "\\");
            let outside = outside.path().to_string_lossy().replace('/', "\\");
            let output = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J", &linked, &outside])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "failed to create test junction: stdout={}, stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let linked_metadata = fs::symlink_metadata(&linked).unwrap();
        assert!(
            linked_metadata.file_type().is_symlink()
                || skill_metadata_is_reparse_point(&linked_metadata)
        );
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        let handler = prepared_skill_resource_handler(&service, workspace.path()).await;

        let error = handler
            .execute(
                &tool_context(workspace.path()),
                serde_json::json!({ "skillId": "review", "path": "linked/secret.md" }),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, ToolError::InvalidArguments(_)));
        assert!(
            error
                .to_string()
                .contains("symbolic link or directory junction")
        );
    }

    #[tokio::test]
    async fn skill_resource_read_never_follows_a_parent_replaced_after_validation() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_test_skill(&data.path().join("skills"), "review", "REVIEW");
        let skill = data.path().join("skills/review");
        fs::create_dir_all(skill.join("references")).unwrap();
        fs::write(skill.join("references/guide.md"), "ORIGINAL-CONTENT").unwrap();
        fs::create_dir_all(outside.path().join("references")).unwrap();
        fs::write(
            outside.path().join("references/guide.md"),
            "EXTERNAL-SECRET-CONTENT",
        )
        .unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        let handler = prepared_skill_resource_handler(&service, workspace.path()).await;
        let moved = data.path().join("skills/review-original");
        let expected_root = skill.canonicalize().unwrap();
        let swapped = Arc::new(AtomicBool::new(false));
        let swapped_for_hook = swapped.clone();
        let skill_for_hook = skill.clone();
        let moved_for_hook = moved.clone();
        let outside_for_hook = outside.path().to_path_buf();
        set_skill_resource_before_open_hook(Some(Arc::new(move |root| {
            if root != expected_root || swapped_for_hook.swap(true, Ordering::SeqCst) {
                return;
            }
            fs::rename(&skill_for_hook, &moved_for_hook).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside_for_hook, &skill_for_hook).unwrap();
            #[cfg(windows)]
            {
                let skill = skill_for_hook.to_string_lossy().replace('/', "\\");
                let outside = outside_for_hook.to_string_lossy().replace('/', "\\");
                let output = std::process::Command::new("cmd")
                    .args(["/C", "mklink", "/J", &skill, &outside])
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "failed to create race junction: link={skill}, target={outside}, stdout={}, stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        })));

        let result = handler
            .execute(
                &tool_context(workspace.path()),
                serde_json::json!({
                    "skillId": "review",
                    "path": "references/guide.md"
                }),
                CancellationToken::new(),
            )
            .await;
        set_skill_resource_before_open_hook(None);

        assert!(swapped.load(Ordering::SeqCst));
        match result {
            Ok(result) => assert_eq!(result.output, "ORIGINAL-CONTENT"),
            Err(error) => assert!(!error.to_string().contains("EXTERNAL-SECRET-CONTENT")),
        }
    }

    #[tokio::test]
    async fn skill_resource_read_enforces_file_and_requested_range_bounds() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(&data.path().join("skills"), "review", "REVIEW");
        let skill = data.path().join("skills/review");
        fs::write(
            skill.join("large.txt"),
            vec![b'x'; MAX_SKILL_RESOURCE_FILE_BYTES + 1],
        )
        .unwrap();
        fs::write(skill.join("small.txt"), "small").unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            ProjectionDb::memory().unwrap(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            StructuredLogger::new(data.path()).unwrap(),
        );
        let handler = prepared_skill_resource_handler(&service, workspace.path()).await;

        for arguments in [
            serde_json::json!({ "skillId": "review", "path": "large.txt" }),
            serde_json::json!({
                "skillId": "review",
                "path": "small.txt",
                "limit": MAX_SKILL_RESOURCE_READ_BYTES + 1
            }),
        ] {
            let error = handler
                .execute(
                    &tool_context(workspace.path()),
                    arguments,
                    CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert!(matches!(error, ToolError::InvalidArguments(_)));
        }
    }

    #[test]
    fn bounded_skill_resource_reader_reports_actual_bytes_read() {
        use std::io::{Seek, SeekFrom};

        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("resource.txt");
        fs::write(&path, "0123456789").unwrap();
        let mut file = File::open(path).unwrap();
        file.seek(SeekFrom::Start(3)).unwrap();

        let (bytes, total_bytes) = read_bounded_skill_resource_file(file).unwrap();

        assert_eq!(bytes, b"3456789");
        assert_eq!(total_bytes, 7);
    }

    #[test]
    fn discovers_skill_in_one_group_deep_directory() {
        let builtin = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_grouped_test_skill(
            builtin.path(),
            "robot-pack",
            "review",
            "GROUPED-INSTRUCTIONS",
        );

        let skills = discover_skills(
            Some(builtin.path()),
            data.path(),
            workspace.path(),
            &ProjectionDb::memory().unwrap(),
        )
        .unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].metadata.name, "review");
        assert_eq!(skills[0].body, "GROUPED-INSTRUCTIONS");
    }

    #[test]
    fn rejects_duplicate_skill_ids_within_one_scope() {
        let builtin = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(builtin.path(), "review", "FLAT-INSTRUCTIONS");
        write_grouped_test_skill(
            builtin.path(),
            "robot-pack",
            "review",
            "GROUPED-INSTRUCTIONS",
        );

        let error = discover_skills(
            Some(builtin.path()),
            data.path(),
            workspace.path(),
            &ProjectionDb::memory().unwrap(),
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("duplicate Skill review in builtin scope")
        );
    }

    #[test]
    fn rejects_group_directory_link_that_escapes_skill_root() {
        let builtin = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(outside.path(), "review", "OUTSIDE-INSTRUCTIONS");

        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), builtin.path().join("robot-pack")).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(outside.path(), builtin.path().join("robot-pack"))
            .is_err()
        {
            let link = builtin.path().join("robot-pack");
            let output = std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    link.to_str().unwrap(),
                    outside.path().to_str().unwrap(),
                ])
                .output()
                .unwrap();
            assert!(output.status.success(), "failed to create test junction");
        }

        let error = discover_skills(
            Some(builtin.path()),
            data.path(),
            workspace.path(),
            &ProjectionDb::memory().unwrap(),
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("symbolic link or directory junction")
        );
    }

    #[test]
    fn rejects_builtin_skill_that_escapes_its_resource_root() {
        let builtin = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write_test_skill(outside.path(), "review", "OUTSIDE-INSTRUCTIONS");

        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("review"), builtin.path().join("review"))
            .unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(
            outside.path().join("review"),
            builtin.path().join("review"),
        )
        .is_err()
        {
            return;
        }

        let error = discover_skills(
            Some(builtin.path()),
            data.path(),
            workspace.path(),
            &ProjectionDb::memory().unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("escapes the Skill root"));
    }

    #[test]
    fn missing_builtin_skill_root_fails_closed() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let missing = data.path().join("missing-builtin-skills");

        let error = discover_skills(
            Some(&missing),
            data.path(),
            workspace.path(),
            &ProjectionDb::memory().unwrap(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("built-in Skill root"));
    }

    #[test]
    fn builtin_skills_participate_in_the_extension_revision() {
        let builtin = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let projection = ProjectionDb::memory().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let service = ExtensionService::with_builtin_skills(
            data.path().into(),
            Some(builtin.path().into()),
            projection,
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );
        let before = service.revision(workspace.path()).unwrap();

        write_test_skill(builtin.path(), "review", "BUILTIN-INSTRUCTIONS");

        let after = service.revision(workspace.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn bundled_workspace_review_skill_matches_the_runtime_contract() {
        let builtin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/resources/skills");
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();

        let skills = discover_skills(
            Some(&builtin),
            data.path(),
            workspace.path(),
            &ProjectionDb::memory().unwrap(),
        )
        .unwrap();
        let skill = skills
            .iter()
            .find(|skill| skill.metadata.name == "workspace-review")
            .expect("bundled workspace-review Skill");

        assert_eq!(skill.scope, "builtin");
        assert_eq!(skill.metadata.risk, ToolRisk::Read);
        assert!(skill.enabled);
        assert!(
            skill
                .body
                .contains("does not grant additional tool permissions")
        );
    }

    #[test]
    fn rejects_skill_metadata_that_exceeds_bounded_description() {
        let content = format!(
            "---\nname: review\ndescription: {}\ntriggers: [review]\nrisk: read\n---\nInstructions",
            "x".repeat(513)
        );
        let error = parse_skill(&content, Path::new("SKILL.md")).unwrap_err();
        assert!(error.to_string().contains("bounded Skill rules"));
    }

    #[test]
    fn missing_skill_category_defaults_to_other() {
        let content = "---\nname: review\ndescription: Review code\ntriggers: [review]\nrisk: read\n---\nInstructions";
        let (metadata, _) = parse_skill(content, Path::new("SKILL.md")).unwrap();

        assert_eq!(metadata.category, SkillCategory::Other);
    }

    #[test]
    fn rejects_unknown_skill_category() {
        let content = "---\nname: review\ndescription: Review code\ntriggers: [review]\nrisk: read\ncategory: imaginary\n---\nInstructions";
        let error = parse_skill(content, Path::new("SKILL.md")).unwrap_err();

        assert!(error.to_string().contains("metadata is invalid"));
    }

    #[test]
    fn accepts_skill_frontmatter_with_mixed_line_endings() {
        let content = "---\r\nname: review\r\ndescription: Review code\ntriggers: [review]\r\nrisk: read\r\nenabled: true\n---\r\nInstructions";
        let (metadata, body) = parse_skill(content, Path::new("SKILL.md")).unwrap();

        assert_eq!(metadata.name, "review");
        assert_eq!(body, "Instructions");
    }

    #[test]
    fn accepts_utf8_bom_before_skill_frontmatter() {
        let content = "\u{feff}---\nname: review\ndescription: Review code\ntriggers: [review]\nrisk: read\nenabled: true\n---\nInstructions";
        let (metadata, body) = parse_skill(content, Path::new("SKILL.md")).unwrap();

        assert_eq!(metadata.name, "review");
        assert_eq!(body, "Instructions");
    }

    #[test]
    fn rejects_non_bom_content_before_skill_frontmatter() {
        let content = " \n---\nname: review\ndescription: Review code\ntriggers: [review]\nrisk: read\nenabled: true\n---\nInstructions";
        let error = parse_skill(content, Path::new("SKILL.md")).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("must start with YAML frontmatter")
        );
    }

    #[cfg(windows)]
    #[test]
    fn hides_windows_verbatim_prefix_in_user_facing_paths() {
        assert_eq!(
            user_facing_path(Path::new(r"\\?\D:\code\k-coder\SKILL.md")),
            r"D:\code\k-coder\SKILL.md"
        );
        assert_eq!(
            user_facing_path(Path::new(r"\\?\UNC\server\share\SKILL.md")),
            r"\\server\share\SKILL.md"
        );
    }

    #[test]
    fn rejects_skill_frontmatter_without_a_closing_delimiter_line() {
        let content = "---\r\nname: review\r\ndescription: Review code\ntriggers: [review]\r\nrisk: read\r\nInstructions";
        let error = parse_skill(content, Path::new("SKILL.md")).unwrap_err();

        assert!(error.to_string().contains("frontmatter is not closed"));
    }

    #[test]
    fn malformed_existing_configuration_fails_closed() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("extensions.json");
        fs::write(&path, "{broken").unwrap();
        assert!(merge_configs(&[path], &[]).is_err());
    }

    #[test]
    fn dedicated_mcp_configuration_uses_global_then_project_scope_priority() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let global_extensions = data.path().join("extensions.json");
        let global_mcp = data.path().join("mcp.json");
        let project_extensions = workspace.path().join("extensions.json");
        let project_mcp = workspace.path().join("mcp.json");
        fs::write(
            &global_extensions,
            r#"{"mcpServers":[{"id":"shared","transport":"stdio","command":["global-legacy"]}],"hooks":[]}"#,
        )
        .unwrap();
        fs::write(
            &global_mcp,
            r#"{"mcpServers":[{"id":"shared","transport":"stdio","command":["global-mcp"]}]}"#,
        )
        .unwrap();
        fs::write(
            &project_extensions,
            r#"{"mcpServers":[{"id":"shared","transport":"stdio","command":["project-legacy"]}],"hooks":[]}"#,
        )
        .unwrap();
        fs::write(
            &project_mcp,
            r#"{"mcpServers":{"shared":{"type":"stdio","command":"project-mcp"}}}"#,
        )
        .unwrap();

        let config = merge_configs(
            &[global_extensions, project_extensions],
            &[global_mcp, project_mcp],
        )
        .unwrap();
        assert_eq!(config.mcp_servers.len(), 1);
        match &config.mcp_servers[0].transport {
            mcp::McpTransportConfig::Stdio { command, .. } => {
                assert_eq!(command, &["project-mcp"])
            }
            _ => panic!("expected stdio MCP configuration"),
        }
    }

    #[test]
    fn named_mcp_configuration_accepts_streamable_http_and_fixed_headers() {
        let config = parse_mcp_config(
            br#"{"mcpServers":{"dingtalk-docs":{"type":"streamable-http","url":"https://mcp.example.com/server/fixture","headers":{"Accept":"application/json, text/event-stream"}}}}"#,
            Path::new("mcp.json"),
        )
        .unwrap();

        assert_eq!(config.mcp_servers.len(), 1);
        assert_eq!(config.mcp_servers[0].id, "dingtalk-docs");
        match &config.mcp_servers[0].transport {
            mcp::McpTransportConfig::StreamableHttp {
                url,
                headers,
                secret_headers,
            } => {
                assert_eq!(url, "https://mcp.example.com/server/fixture");
                assert_eq!(
                    headers.get("Accept").map(String::as_str),
                    Some("application/json, text/event-stream")
                );
                assert!(secret_headers.is_empty());
            }
            _ => panic!("expected streamable HTTP MCP configuration"),
        }
    }

    #[test]
    fn dedicated_mcp_configuration_rejects_duplicates_before_writing() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let projection = ProjectionDb::memory().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            projection,
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );
        let content = r#"{"mcpServers":{"local":{"type":"stdio","command":"node"},"local":{"type":"stdio","command":"node"}}}"#;

        let error = service
            .save_mcp_config(workspace.path(), "project", content)
            .unwrap_err();

        assert!(error.to_string().contains("duplicate MCP server local"));
        assert!(!workspace.path().join(".k-coder/mcp.json").exists());
    }

    #[test]
    fn named_mcp_configuration_rejects_credentials_in_fixed_headers() {
        let error = parse_mcp_config(
            br#"{"mcpServers":{"remote":{"type":"streamable-http","url":"https://example.com/mcp","headers":{"Authorization":"Bearer plaintext"}}}}"#,
            Path::new("mcp.json"),
        )
        .unwrap_err();

        assert!(error.to_string().contains("use secret_headers"));
    }

    #[test]
    fn named_mcp_configuration_rejects_duplicate_headers_before_map_coercion() {
        for content in [
            br#"{"mcpServers":{"remote":{"type":"streamable-http","url":"https://example.com/mcp","headers":{"Accept":"application/json","Accept":"text/event-stream"}}}}"#.as_slice(),
            br#"{"mcpServers":{"remote":{"type":"streamable-http","url":"https://example.com/mcp","headers":{"Accept":"application/json","accept":"text/event-stream"}}}}"#.as_slice(),
        ] {
            let error = parse_mcp_config(content, Path::new("mcp.json")).unwrap_err();
            assert!(error.to_string().contains("duplicate HTTP header"));
        }
    }

    #[test]
    fn malformed_dedicated_mcp_configuration_is_returned_for_json_repair() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        fs::write(data.path().join("mcp.json"), "{broken").unwrap();
        let projection = ProjectionDb::memory().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            projection,
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );

        let view = service.mcp_config_view(workspace.path()).unwrap();

        assert_eq!(view.schema_version, 2);
        assert!(view.global.exists);
        assert_eq!(view.global.content, "{broken");
        assert!(view.global.error.is_some());
        assert!(merge_configs(&[], &[data.path().join("mcp.json")]).is_err());
    }

    #[test]
    fn dedicated_mcp_configuration_rejects_invalid_scope_and_oversized_content() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let projection = ProjectionDb::memory().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            projection,
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );

        assert!(
            service
                .save_mcp_config(workspace.path(), "workspace", r#"{"mcpServers":[]}"#)
                .is_err()
        );
        assert!(
            service
                .save_mcp_config(
                    workspace.path(),
                    "project",
                    &" ".repeat(MAX_CONFIG_BYTES + 1),
                )
                .is_err()
        );
        assert!(!workspace.path().join(".k-coder/mcp.json").exists());
    }

    #[test]
    fn dedicated_mcp_configuration_is_saved_and_returned_as_utf8_json() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let projection = ProjectionDb::memory().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        projection
            .set_setting("extension/mcp/local", "false")
            .unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            projection.clone(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );
        let content = r#"{"mcpServers":[{"id":"local","enabled":false,"timeoutMs":45000,"transport":"stdio","command":["node","server.mjs"],"secret_env":{"TOKEN":"local-token"}}]}"#;

        service
            .save_mcp_config(workspace.path(), "project", content)
            .unwrap();
        let view = service.mcp_config_view(workspace.path()).unwrap();

        assert!(view.project.exists);
        assert!(view.project.error.is_none());
        assert!(view.project.content.ends_with('\n'));
        let saved = serde_json::from_str::<serde_json::Value>(&view.project.content).unwrap();
        let local = &saved["mcpServers"]["local"];
        assert!(saved["mcpServers"].is_object());
        assert_eq!(local["type"], "stdio");
        assert_eq!(local["command"], "node");
        assert_eq!(local["args"], serde_json::json!(["server.mjs"]));
        assert!(local.get("id").is_none());
        assert!(workspace.path().join(".k-coder/mcp.json").is_file());
        assert_eq!(
            projection
                .setting("extension/mcp/local")
                .unwrap()
                .as_deref(),
            Some("true")
        );
        let audit = fs::read_to_string(data.path().join("extension-audit.jsonl")).unwrap();
        assert!(audit.contains("1 servers"));
        assert!(!audit.contains("local-token"));
    }

    #[test]
    fn project_mcp_configuration_rejects_link_escape() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), workspace.path().join(".k-coder")).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(outside.path(), workspace.path().join(".k-coder"))
            .is_err()
        {
            return;
        }
        let projection = ProjectionDb::memory().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            projection,
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );

        let error = service.mcp_config_view(workspace.path()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("escapes its configuration scope")
        );
    }

    #[tokio::test]
    async fn selected_skills_are_read_before_runtime_and_high_risk_requires_explicit_enable() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let read = workspace.path().join(".k-coder/skills/review");
        let write = workspace.path().join(".k-coder/skills/deploy");
        fs::create_dir_all(&read).unwrap();
        fs::create_dir_all(&write).unwrap();
        fs::write(read.join("SKILL.md"), "---\nname: review\ndescription: Review code\ntriggers: [review]\nrisk: read\nenabled: true\n---\nREVIEW-INSTRUCTIONS").unwrap();
        fs::write(write.join("SKILL.md"), "---\nname: deploy\ndescription: Deploy code\ntriggers: [deploy]\nrisk: external\nenabled: true\n---\nDEPLOY-INSTRUCTIONS").unwrap();
        let projection = ProjectionDb::memory().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            projection.clone(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );
        service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();
        let instructions = service
            .runtime_instructions("please review and deploy")
            .unwrap();
        assert!(instructions.contains("REVIEW-INSTRUCTIONS"));
        assert!(!instructions.contains("DEPLOY-INSTRUCTIONS"));
        let explicit_disabled = service.runtime_instructions("/deploy now").unwrap();
        assert!(!explicit_disabled.contains("DEPLOY-INSTRUCTIONS"));
        service.set_enabled("skill", "deploy", true).unwrap();
        service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();
        assert!(
            service
                .runtime_instructions("/deploy now")
                .unwrap()
                .contains("DEPLOY-INSTRUCTIONS")
        );
    }

    #[tokio::test]
    async fn plugin_skill_handlers_and_catalog_share_the_extension_runtime() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let plugin_root = write_test_plugin(data.path(), "review-package", "review-tools");
        let projection = ProjectionDb::memory().unwrap();
        let logger = StructuredLogger::new(data.path()).unwrap();
        let service = ExtensionService::new(
            data.path().into(),
            projection.clone(),
            Arc::new(mcp::OsMcpSecretStore::new()),
            logger,
        );

        let discovered = service.plugin_overview(true).unwrap();
        assert!(!discovered.plugins[0].enabled);
        service
            .set_plugin_enabled("review-tools@local", true)
            .unwrap();
        let prepared = service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();

        let names = prepared
            .handlers
            .iter()
            .map(|handler| handler.definition().name)
            .collect::<HashSet<_>>();
        assert!(names.contains("plugin_skill_read"));
        assert!(names.contains("plugin_resource_read"));
        let catalog = service.runtime_instructions("use @review-tools").unwrap();
        assert!(catalog.contains("plugin://review-tools@local"));
        assert!(catalog.contains("plugin_skill_read"));
        assert!(!catalog.contains("PLUGIN-REVIEW-BODY"));

        service
            .set_plugin_enabled("review-tools@local", false)
            .unwrap();
        let prepared = service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();
        assert!(prepared.handlers.iter().all(|handler| {
            !matches!(
                handler.definition().name.as_str(),
                "plugin_skill_read" | "plugin_resource_read"
            )
        }));
        assert!(
            !service
                .runtime_instructions("@review-tools")
                .unwrap()
                .contains("plugin://review-tools@local")
        );

        service
            .set_plugin_enabled("review-tools@local", true)
            .unwrap();
        fs::remove_dir_all(plugin_root).unwrap();
        let missing = service.plugin_overview(true).unwrap();
        assert!(missing.plugins.is_empty());
        assert_eq!(
            projection
                .setting("extension/plugin/review-tools@local")
                .unwrap()
                .as_deref(),
            Some("false")
        );
        assert!(service.overview().audit.iter().any(|entry| {
            entry.event == "plugin_auto_disabled"
                && entry.id == "review-tools@local"
                && entry.success
        }));
    }
}
