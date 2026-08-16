pub mod hooks;
pub mod mcp;
pub mod plugins;

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
use crate::protocol::{PluginOverview, PluginState, ToolRisk};
use crate::tools::{ToolError, ToolHandler, ToolHookRunner};

use self::hooks::{HookConfig, HookPipeline};
use self::mcp::{McpSecretStore, McpServerConfig};
use self::plugins::PluginHost;

const MAX_INSTRUCTION_FILE_BYTES: usize = 256 * 1024;
const MAX_RUNTIME_INSTRUCTION_BYTES: usize = 48 * 1024;
const MAX_SKILL_BYTES: usize = 256 * 1024;
const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_SELECTED_SKILLS: usize = 4;
const MAX_AUDIT_RECORDS: usize = 200;
const MAX_AUDIT_BYTES: u64 = 2 * 1024 * 1024;
const MCP_CONFIG_FILE_NAME: &str = "mcp.json";

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpConfigFile {
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
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
    builtin_skills_root: Option<PathBuf>,
    projection: ProjectionDb,
    secrets: Arc<dyn McpSecretStore>,
    logger: StructuredLogger,
    overview: Arc<RwLock<ExtensionOverview>>,
    instructions: Arc<RwLock<Vec<LoadedInstruction>>>,
    skills: Arc<RwLock<Vec<LoadedSkill>>>,
    audit: Arc<Mutex<Vec<ExtensionAudit>>>,
    audit_path: PathBuf,
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
        paths.extend([
            self.data_root.join("AGENTS.md"),
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
            .filter(|skill| skill.enabled && skill_is_selected(&lower_input, skill))
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
            schema_version: 1,
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
    "{\n  \"mcpServers\": []\n}\n".into()
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
                    user_facing_path(&file)
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
                    user_facing_path(&canonical)
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
    Ok((metadata, body.trim().to_string()))
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
            r#"{"mcpServers":[{"id":"shared","transport":"stdio","command":["project-mcp"]}]}"#,
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
        let content = r#"{"mcpServers":[{"id":"local","transport":"stdio","command":["node"]},{"id":"local","transport":"stdio","command":["node"]}]}"#;

        let error = service
            .save_mcp_config(workspace.path(), "project", content)
            .unwrap_err();

        assert!(error.to_string().contains("duplicate MCP server local"));
        assert!(!workspace.path().join(".k-coder/mcp.json").exists());
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
        assert!(view.project.content.contains("\"local\""));
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
