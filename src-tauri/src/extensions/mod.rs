pub mod hooks;
pub mod mcp;

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::logging::StructuredLogger;
use crate::persistence::ProjectionDb;
use crate::protocol::ToolRisk;
use crate::tools::{ToolError, ToolHandler, ToolHookRunner};

use self::hooks::{HookConfig, HookPipeline};
use self::mcp::{McpSecretStore, McpServerConfig};

const MAX_INSTRUCTION_FILE_BYTES: usize = 256 * 1024;
const MAX_RUNTIME_INSTRUCTION_BYTES: usize = 48 * 1024;
const MAX_SKILL_BYTES: usize = 256 * 1024;
const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_SELECTED_SKILLS: usize = 4;
const MAX_AUDIT_RECORDS: usize = 200;
const MAX_AUDIT_BYTES: u64 = 2 * 1024 * 1024;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionSource {
    pub path: String,
    pub scope: String,
    pub priority: u32,
    pub bytes: usize,
}

#[derive(Debug, Clone)]
struct LoadedInstruction {
    source: InstructionSource,
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillMetadata {
    name: String,
    description: String,
    triggers: Vec<String>,
    risk: ToolRisk,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
struct LoadedSkill {
    metadata: SkillMetadata,
    path: PathBuf,
    scope: String,
    body: String,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDiagnostic {
    pub name: String,
    pub description: String,
    pub path: String,
    pub scope: String,
    pub risk: ToolRisk,
    pub triggers: Vec<String>,
    pub enabled: bool,
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
    projection: ProjectionDb,
    secrets: Arc<dyn McpSecretStore>,
    logger: StructuredLogger,
    overview: Arc<RwLock<ExtensionOverview>>,
    instructions: Arc<RwLock<Vec<LoadedInstruction>>>,
    skills: Arc<RwLock<Vec<LoadedSkill>>>,
    audit: Arc<Mutex<Vec<ExtensionAudit>>>,
    audit_path: PathBuf,
}

impl ExtensionService {
    pub fn new(
        data_root: PathBuf,
        projection: ProjectionDb,
        secrets: Arc<dyn McpSecretStore>,
        logger: StructuredLogger,
    ) -> Self {
        let audit_path = data_root.join("extension-audit.jsonl");
        let audit = load_audit(&audit_path);
        Self {
            data_root,
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
        }
    }

    pub async fn prepare(
        &self,
        workspace: &Path,
        cancellation: CancellationToken,
    ) -> Result<PreparedExtensions, ExtensionError> {
        let workspace = workspace
            .canonicalize()
            .map_err(|error| ExtensionError::Io(error.to_string()))?;
        let config_paths = vec![
            self.data_root.join("extensions.json"),
            workspace.join(".k-coder").join("extensions.json"),
        ];
        let config = merge_configs(&config_paths)?;
        let instructions = discover_instructions(&self.data_root, &workspace)?;
        let skills = discover_skills(&self.data_root, &workspace, &self.projection)?;
        let mut handlers = Vec::<Arc<dyn ToolHandler>>::new();
        let mut risks = HashMap::new();
        let mut tool_names = HashSet::new();
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
        let mut paths = vec![
            self.data_root.join("extensions.json"),
            self.data_root.join("AGENTS.md"),
            workspace.join("AGENTS.md"),
            workspace.join(".k-coder").join("extensions.json"),
        ];
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
        Ok(hasher.finish())
    }

    pub fn runtime_instructions(&self, input: &str) -> Result<String, ExtensionError> {
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
                    && skill.metadata.triggers.iter().any(|trigger| {
                        !trigger.trim().is_empty() && lower_input.contains(&trigger.to_lowercase())
                    })
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
                    skill.path.display(),
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
        if output.len() > MAX_RUNTIME_INSTRUCTION_BYTES {
            return Err(ExtensionError::Config(format!(
                "combined runtime instructions exceed {MAX_RUNTIME_INSTRUCTION_BYTES} bytes"
            )));
        }
        Ok(output)
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

    pub fn set_enabled(&self, kind: &str, id: &str, enabled: bool) -> Result<(), ExtensionError> {
        if !matches!(kind, "skill" | "mcp" | "hook") || id.trim().is_empty() {
            return Err(ExtensionError::Config("invalid extension toggle".into()));
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
                .map(|path| path.to_string_lossy().to_string())
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
                    path: skill.path.to_string_lossy().to_string(),
                    scope: skill.scope.clone(),
                    risk: skill.metadata.risk,
                    triggers: skill.metadata.triggers.clone(),
                    enabled: skill.enabled,
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

fn merge_configs(paths: &[PathBuf]) -> Result<ExtensionConfig, ExtensionError> {
    let mut servers = HashMap::<String, McpServerConfig>::new();
    let mut hooks = HashMap::<String, HookConfig>::new();
    for path in paths {
        let Some(config) = read_config(path)? else {
            continue;
        };
        let mut local_servers = HashSet::new();
        for server in config.mcp_servers {
            server.validate()?;
            if !local_servers.insert(server.id.clone()) {
                return Err(ExtensionError::Config(format!(
                    "{} contains duplicate MCP server {}",
                    path.display(),
                    server.id
                )));
            }
            servers.insert(server.id.clone(), server);
        }
        let mut local_hooks = HashSet::new();
        for hook in config.hooks {
            hook.validate().map_err(ExtensionError::Config)?;
            if !local_hooks.insert(hook.id.clone()) {
                return Err(ExtensionError::Config(format!(
                    "{} contains duplicate hook {}",
                    path.display(),
                    hook.id
                )));
            }
            hooks.insert(hook.id.clone(), hook);
        }
    }
    let mut mcp_servers = servers.into_values().collect::<Vec<_>>();
    let mut hooks = hooks.into_values().collect::<Vec<_>>();
    mcp_servers.sort_by(|left, right| left.id.cmp(&right.id));
    hooks.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(ExtensionConfig { mcp_servers, hooks })
}

fn read_config(path: &Path) -> Result<Option<ExtensionConfig>, ExtensionError> {
    if !path.exists() {
        return Ok(None);
    }
    let metadata = path
        .metadata()
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    if !metadata.is_file() || metadata.len() as usize > MAX_CONFIG_BYTES {
        return Err(ExtensionError::Config(format!(
            "{} must be a file no larger than {MAX_CONFIG_BYTES} bytes",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|error| ExtensionError::Io(error.to_string()))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| ExtensionError::Config(format!("{}: {error}", path.display())))
}

fn discover_instructions(
    data_root: &Path,
    workspace: &Path,
) -> Result<Vec<LoadedInstruction>, ExtensionError> {
    let workspace = workspace
        .canonicalize()
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    let mut paths = vec![
        (data_root.join("AGENTS.md"), "global".to_string(), 100),
        (workspace.join("AGENTS.md"), "project".to_string(), 200),
    ];
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
                path.display()
            )));
        }
        result.push(LoadedInstruction {
            source: InstructionSource {
                path: path.to_string_lossy().to_string(),
                scope,
                priority,
                bytes: content.len(),
            },
            content,
        });
    }
    Ok(result)
}

fn discover_skills(
    data_root: &Path,
    workspace: &Path,
    projection: &ProjectionDb,
) -> Result<Vec<LoadedSkill>, ExtensionError> {
    let roots = [
        (data_root.join("skills"), "global"),
        (workspace.join(".k-coder").join("skills"), "project"),
    ];
    let mut selected = HashMap::<String, LoadedSkill>::new();
    for (root, scope) in roots {
        if !root.exists() {
            continue;
        }
        let canonical_root = root
            .canonicalize()
            .map_err(|error| ExtensionError::Io(error.to_string()))?;
        let mut directories = fs::read_dir(&canonical_root)
            .map_err(|error| ExtensionError::Io(error.to_string()))?
            .map(|entry| entry.map(|value| value.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ExtensionError::Io(error.to_string()))?;
        directories.sort();
        for directory in directories {
            if !directory.is_dir() {
                continue;
            }
            let file = directory.join("SKILL.md");
            if !file.exists() {
                continue;
            }
            let canonical = file
                .canonicalize()
                .map_err(|error| ExtensionError::Io(error.to_string()))?;
            if !canonical.starts_with(&canonical_root) {
                return Err(ExtensionError::Skill(format!(
                    "{} escapes the Skill root",
                    file.display()
                )));
            }
            let content = read_bounded_utf8(&canonical, MAX_SKILL_BYTES)?;
            let (metadata, body) = parse_skill(&content, &canonical)?;
            let directory_name = directory
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if metadata.name != directory_name || !valid_skill_name(&metadata.name) {
                return Err(ExtensionError::Skill(format!(
                    "{} name must match its directory and use lowercase kebab-case",
                    canonical.display()
                )));
            }
            let override_enabled = projection
                .setting(&format!("extension/skill/{}", metadata.name))
                .map_err(|error| ExtensionError::Config(error.to_string()))?;
            let enabled = match metadata.risk {
                ToolRisk::Read => override_enabled
                    .map(|value| value == "true")
                    .unwrap_or(metadata.enabled),
                ToolRisk::Write | ToolRisk::Delete | ToolRisk::External => {
                    override_enabled.as_deref() == Some("true")
                }
            };
            selected.insert(
                metadata.name.clone(),
                LoadedSkill {
                    metadata,
                    path: canonical,
                    scope: scope.into(),
                    body,
                    enabled,
                },
            );
        }
    }
    let mut skills = selected.into_values().collect::<Vec<_>>();
    skills.sort_by(|left, right| left.metadata.name.cmp(&right.metadata.name));
    Ok(skills)
}

fn parse_skill(content: &str, path: &Path) -> Result<(SkillMetadata, String), ExtensionError> {
    let body = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .ok_or_else(|| {
            ExtensionError::Skill(format!(
                "{} must start with YAML frontmatter",
                path.display()
            ))
        })?;
    let (frontmatter, body) = body
        .split_once("\n---\n")
        .or_else(|| body.split_once("\r\n---\r\n"))
        .ok_or_else(|| {
            ExtensionError::Skill(format!("{} frontmatter is not closed", path.display()))
        })?;
    let metadata: SkillMetadata = serde_yaml::from_str(frontmatter).map_err(|error| {
        ExtensionError::Skill(format!("{} metadata is invalid: {error}", path.display()))
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
            path.display()
        )));
    }
    Ok((metadata, body.trim().to_string()))
}

fn read_bounded_utf8(path: &Path, limit: usize) -> Result<String, ExtensionError> {
    let metadata = path
        .metadata()
        .map_err(|error| ExtensionError::Io(error.to_string()))?;
    if !metadata.is_file() || metadata.len() as usize > limit {
        return Err(ExtensionError::Config(format!(
            "{} must be a file no larger than {limit} bytes",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|error| ExtensionError::Io(error.to_string()))?;
    String::from_utf8(bytes)
        .map_err(|_| ExtensionError::Config(format!("{} must be UTF-8", path.display())))
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
    fn validates_skill_frontmatter_and_project_override() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let global = data.path().join("skills/review");
        let project = workspace.path().join(".k-coder/skills/review");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&project).unwrap();
        let skill = "---\nname: review\ndescription: Review code\ntriggers: [review]\nrisk: read\nenabled: true\n---\nRead carefully.";
        fs::write(global.join("SKILL.md"), skill).unwrap();
        fs::write(
            project.join("SKILL.md"),
            skill.replace("carefully", "strictly"),
        )
        .unwrap();
        let projection = ProjectionDb::memory().unwrap();
        let skills = discover_skills(data.path(), workspace.path(), &projection).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].scope, "project");
        assert!(skills[0].body.contains("strictly"));
    }

    #[test]
    fn malformed_existing_configuration_fails_closed() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("extensions.json");
        fs::write(&path, "{broken").unwrap();
        assert!(merge_configs(&[path]).is_err());
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
        service.set_enabled("skill", "deploy", true).unwrap();
        service
            .prepare(workspace.path(), CancellationToken::new())
            .await
            .unwrap();
        assert!(
            service
                .runtime_instructions("deploy now")
                .unwrap()
                .contains("DEPLOY-INSTRUCTIONS")
        );
    }
}
