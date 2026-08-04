use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::advanced::AdvancedServices;
use crate::execution::{CommandRuntime, ExecutionError, NativePtyRuntime};
use crate::extensions::mcp::OsMcpSecretStore;
use crate::extensions::{ExtensionError, ExtensionOverview, ExtensionService};
use crate::logging::StructuredLogger;
use crate::multi_agent::MultiAgentCoordinator;
use crate::patch::{PatchError, PatchService};
use crate::persistence::ProjectionDb;
use crate::policy::{ApprovalManager, UserInputManager};
use crate::protocol::{
    ApprovalAction, ApprovalMode, ApprovalResolution, ChangeSet, ReasoningEffort, TurnState,
    UserInputAction, UserInputResolution,
};
use crate::providers::{
    AnthropicMessagesProvider, CredentialError, CredentialStore, FallbackProvider, FallbackTarget,
    GoogleGeminiProvider, OpenAiChatCompletionsProvider, OpenAiResponsesProvider,
    OsCredentialStore, Provider, ProviderCatalogView, ProviderConfig, ProviderConfigError,
    ProviderConfigStore, ProviderConfigView, ProviderTransport, SaveProviderConfigRequest,
};
use crate::storage::{
    JsonlThreadRepository, StorageError, StoredEvent, StoredEventKind, ThreadRepository,
};
use crate::tools::ToolRegistry;

pub struct AppState {
    started_at: Instant,
    repository: Arc<JsonlThreadRepository>,
    provider_config: ProviderConfigStore,
    credentials: Arc<dyn CredentialStore>,
    data_root: PathBuf,
    workspace_root: RwLock<PathBuf>,
    tool_registry: RwLock<ToolRegistry>,
    patch_service: PatchService,
    approvals: Arc<ApprovalManager>,
    approval_mode: RwLock<ApprovalMode>,
    reasoning_effort: RwLock<ReasoningEffort>,
    user_inputs: Arc<UserInputManager>,
    command_runtime: RwLock<CommandRuntime>,
    pty_runtime: RwLock<NativePtyRuntime>,
    logger: StructuredLogger,
    active_turns: Mutex<HashMap<String, CancellationToken>>,
    recovery_lock: Mutex<()>,
    extensions: ExtensionService,
    extension_workspace: Mutex<Option<(PathBuf, u64)>>,
    subagents: MultiAgentCoordinator,
    advanced: AdvancedServices,
}

impl AppState {
    pub fn new(data_root: impl AsRef<Path>) -> Result<Self, AppStateError> {
        Self::with_credentials(data_root, Arc::new(OsCredentialStore::new()))
    }

    pub fn with_credentials(
        data_root: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, AppStateError> {
        let data_root = data_root.as_ref().to_path_buf();
        let fallback =
            std::env::current_dir().map_err(|error| AppStateError::Workspace(error.to_string()))?;
        let projection = ProjectionDb::open(&data_root)
            .map_err(|error| AppStateError::Workspace(error.to_string()))?;
        let workspace_root = projection
            .setting("active_workspace")
            .map_err(|error| AppStateError::Workspace(error.to_string()))?
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .unwrap_or(fallback);
        Self::with_workspace_and_credentials(data_root, workspace_root, credentials)
    }

    pub fn with_workspace_and_credentials(
        data_root: impl AsRef<Path>,
        workspace_root: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, AppStateError> {
        let data_root = data_root.as_ref().to_path_buf();
        let workspace_root = workspace_root
            .as_ref()
            .canonicalize()
            .map_err(|error| AppStateError::Workspace(error.to_string()))?;
        if !workspace_root.is_dir() {
            return Err(AppStateError::Workspace(
                "workspace root is not a directory".to_string(),
            ));
        }
        let patch_service = PatchService::new();
        let command_runtime = CommandRuntime::with_recovery(&workspace_root, &data_root)?;
        let pty_runtime = NativePtyRuntime::new(&workspace_root)?;
        let repository = Arc::new(JsonlThreadRepository::new(&data_root)?);
        let approval_mode = repository
            .projection()
            .setting("approval_mode")
            .map_err(|error| AppStateError::Workspace(error.to_string()))?
            .and_then(|raw| serde_json::from_str::<ApprovalMode>(&raw).ok())
            .unwrap_or_default();
        let reasoning_effort = repository
            .projection()
            .setting("reasoning_effort")
            .map_err(|error| AppStateError::Workspace(error.to_string()))?
            .and_then(|raw| serde_json::from_str::<ReasoningEffort>(&raw).ok())
            .unwrap_or_default();
        let logger = StructuredLogger::new(&data_root)
            .map_err(|error| AppStateError::Logging(error.to_string()))?;
        let extensions = ExtensionService::new(
            data_root.clone(),
            repository.projection(),
            Arc::new(OsMcpSecretStore::new()),
            logger.clone(),
        );
        let subagents = MultiAgentCoordinator::new(&data_root)
            .map_err(|error| AppStateError::MultiAgent(error.to_string()))?;
        let advanced = AdvancedServices::new(&data_root).map_err(AppStateError::Advanced)?;
        let (advanced_handlers, advanced_risks) = advanced.tool_handlers(&workspace_root);
        let tool_registry = ToolRegistry::workspace_tools_with_execution(
            patch_service.clone(),
            command_runtime.clone(),
        )
        .with_additional_handlers(advanced_handlers, advanced_risks)?;
        Ok(Self {
            started_at: Instant::now(),
            repository,
            provider_config: ProviderConfigStore::new(&data_root),
            credentials,
            data_root: data_root.clone(),
            workspace_root: RwLock::new(workspace_root),
            tool_registry: RwLock::new(tool_registry),
            patch_service,
            approvals: Arc::new(ApprovalManager::new(Duration::from_secs(5 * 60))),
            approval_mode: RwLock::new(approval_mode),
            reasoning_effort: RwLock::new(reasoning_effort),
            user_inputs: Arc::new(UserInputManager::new(Duration::from_secs(10 * 60))),
            command_runtime: RwLock::new(command_runtime),
            pty_runtime: RwLock::new(pty_runtime),
            logger,
            active_turns: Mutex::new(HashMap::new()),
            recovery_lock: Mutex::new(()),
            extensions,
            extension_workspace: Mutex::new(None),
            subagents,
            advanced,
        })
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    pub fn repository(&self) -> Arc<JsonlThreadRepository> {
        self.repository.clone()
    }

    pub fn runtime_repository(&self) -> Arc<dyn ThreadRepository> {
        self.repository.clone()
    }

    pub fn workspace_root(&self) -> PathBuf {
        self.workspace_root
            .read()
            .expect("workspace lock poisoned")
            .clone()
    }

    pub fn tool_registry(&self) -> ToolRegistry {
        self.tool_registry
            .read()
            .expect("tool registry lock poisoned")
            .clone()
    }

    pub fn advanced(&self) -> AdvancedServices {
        self.advanced.clone()
    }

    pub fn patch_service(&self) -> PatchService {
        self.patch_service.clone()
    }

    pub fn approvals(&self) -> Arc<ApprovalManager> {
        self.approvals.clone()
    }

    pub fn approval_mode(&self) -> ApprovalMode {
        *self
            .approval_mode
            .read()
            .expect("approval mode lock poisoned")
    }

    pub async fn set_approval_mode(
        &self,
        mode: ApprovalMode,
    ) -> Result<ApprovalMode, AppStateError> {
        let active_turns = self.active_turns.lock().await;
        if !active_turns.is_empty() || self.subagents.has_active() {
            return Err(AppStateError::ApprovalModeBusy);
        }
        self.repository
            .projection()
            .set_setting(
                "approval_mode",
                &serde_json::to_string(&mode)
                    .map_err(|error| AppStateError::Workspace(error.to_string()))?,
            )
            .map_err(|error| AppStateError::Workspace(error.to_string()))?;
        *self
            .approval_mode
            .write()
            .map_err(|_| AppStateError::ApprovalModeLock)? = mode;
        drop(active_turns);
        Ok(mode)
    }

    pub fn reasoning_effort(&self) -> ReasoningEffort {
        *self
            .reasoning_effort
            .read()
            .expect("reasoning effort lock poisoned")
    }

    pub async fn set_reasoning_effort(
        &self,
        effort: ReasoningEffort,
    ) -> Result<ReasoningEffort, AppStateError> {
        self.repository
            .projection()
            .set_setting(
                "reasoning_effort",
                &serde_json::to_string(&effort)
                    .map_err(|error| AppStateError::Workspace(error.to_string()))?,
            )
            .map_err(|error| AppStateError::Workspace(error.to_string()))?;
        *self
            .reasoning_effort
            .write()
            .map_err(|_| AppStateError::ReasoningEffortLock)? = effort;
        Ok(effort)
    }

    pub fn user_inputs(&self) -> Arc<UserInputManager> {
        self.user_inputs.clone()
    }

    pub fn command_runtime(&self) -> CommandRuntime {
        self.command_runtime
            .read()
            .expect("command runtime lock poisoned")
            .clone()
    }

    pub fn pty_runtime(&self) -> NativePtyRuntime {
        self.pty_runtime
            .read()
            .expect("PTY runtime lock poisoned")
            .clone()
    }

    pub async fn switch_workspace(&self, path: impl AsRef<Path>) -> Result<PathBuf, AppStateError> {
        if !self.active_turns.lock().await.is_empty() || self.subagents.has_active() {
            return Err(AppStateError::Workspace(
                "stop active turns and subagents before switching workspace".into(),
            ));
        }
        let path = path
            .as_ref()
            .canonicalize()
            .map_err(|error| AppStateError::Workspace(error.to_string()))?;
        if !path.is_dir() {
            return Err(AppStateError::Workspace(
                "workspace root is not a directory".into(),
            ));
        }
        let command = CommandRuntime::with_recovery(&path, &self.data_root)?;
        let pty = NativePtyRuntime::new(&path)?;
        let (advanced_handlers, advanced_risks) = self.advanced.tool_handlers(&path);
        let tool_registry = ToolRegistry::workspace_tools_with_execution(
            self.patch_service.clone(),
            command.clone(),
        )
        .with_additional_handlers(advanced_handlers, advanced_risks)?;
        self.repository
            .projection()
            .set_setting("active_workspace", &path.to_string_lossy())
            .map_err(|error| AppStateError::Workspace(error.to_string()))?;
        *self
            .workspace_root
            .write()
            .map_err(|_| AppStateError::Workspace("workspace lock poisoned".into()))? =
            path.clone();
        *self
            .command_runtime
            .write()
            .map_err(|_| AppStateError::Workspace("command runtime lock poisoned".into()))? =
            command;
        *self
            .pty_runtime
            .write()
            .map_err(|_| AppStateError::Workspace("PTY runtime lock poisoned".into()))? = pty;
        *self
            .tool_registry
            .write()
            .map_err(|_| AppStateError::Workspace("tool registry lock poisoned".into()))? =
            tool_registry;
        *self.extension_workspace.lock().await = None;
        Ok(path)
    }

    pub async fn prepare_extensions(&self, force: bool) -> Result<(), AppStateError> {
        let workspace = self.workspace_root();
        let revision = self.extensions.revision(&workspace)?;
        let mut prepared_for = self.extension_workspace.lock().await;
        if !force
            && prepared_for
                .as_ref()
                .is_some_and(|(path, value)| path == &workspace && *value == revision)
        {
            return Ok(());
        }
        *prepared_for = None;
        let prepared = self
            .extensions
            .prepare(&workspace, CancellationToken::new())
            .await?;
        let (advanced_handlers, advanced_risks) = self.advanced.tool_handlers(&workspace);
        let registry = ToolRegistry::workspace_tools_with_execution(
            self.patch_service.clone(),
            self.command_runtime(),
        )
        .with_additional_handlers(advanced_handlers, advanced_risks)?
        .with_extensions(prepared.handlers, prepared.risks, prepared.hooks)?;
        *self
            .tool_registry
            .write()
            .map_err(|_| AppStateError::Workspace("tool registry lock poisoned".into()))? =
            registry;
        *prepared_for = Some((workspace, revision));
        Ok(())
    }

    pub fn extension_instructions(&self, input: &str) -> Result<String, AppStateError> {
        Ok(self.extensions.runtime_instructions(input)?)
    }

    pub fn extension_overview(&self) -> ExtensionOverview {
        self.extensions.overview()
    }

    pub async fn set_extension_enabled(
        &self,
        kind: &str,
        id: &str,
        enabled: bool,
    ) -> Result<(), AppStateError> {
        self.extensions.set_enabled(kind, id, enabled)?;
        self.prepare_extensions(true).await
    }

    pub async fn save_mcp_secret(
        &self,
        server: &str,
        name: &str,
        value: &str,
    ) -> Result<(), AppStateError> {
        self.extensions.save_secret(server, name, value)?;
        self.prepare_extensions(true).await
    }

    pub async fn delete_mcp_secret(&self, server: &str, name: &str) -> Result<(), AppStateError> {
        self.extensions.delete_secret(server, name)?;
        self.prepare_extensions(true).await
    }

    pub fn logger(&self) -> StructuredLogger {
        self.logger.clone()
    }

    pub fn subagents(&self) -> MultiAgentCoordinator {
        self.subagents.clone()
    }

    pub fn provider_config(&self) -> Result<Option<ProviderConfigView>, AppStateError> {
        let Some(config) = self.provider_config.load()? else {
            return Ok(None);
        };
        Ok(Some(self.provider_view(config)?))
    }

    pub fn provider_catalog(&self) -> Result<ProviderCatalogView, AppStateError> {
        let (active_provider_id, configs) = self.provider_config.list()?;
        let providers = configs
            .into_iter()
            .map(|config| self.provider_view(config))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ProviderCatalogView {
            schema_version: crate::protocol::PROTOCOL_VERSION,
            active_provider_id,
            providers,
        })
    }

    fn provider_view(&self, config: ProviderConfig) -> Result<ProviderConfigView, AppStateError> {
        let has_api_key = self.credentials.get_api_key(&config.id)?.is_some();
        Ok(ProviderConfigView {
            schema_version: config.schema_version,
            id: config.id,
            kind: config.kind,
            transport: config.transport,
            name: config.name,
            base_url: config.base_url,
            model: config.model,
            models: config.models,
            endpoints: config.endpoints,
            has_api_key,
        })
    }

    pub fn save_provider_config(
        &self,
        request: SaveProviderConfigRequest,
    ) -> Result<ProviderConfigView, AppStateError> {
        let config = request.public_config()?;
        let api_key = request
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let effective_activate = request.activate || self.provider_config.load()?.is_none();
        if let Some(api_key) = api_key {
            self.credentials.set_api_key(&config.id, api_key)?;
        } else if effective_activate && self.credentials.get_api_key(&config.id)?.is_none() {
            return Err(AppStateError::ProviderNotConfigured(
                "an API key is required".to_string(),
            ));
        }
        self.provider_config
            .save_provider(&config, effective_activate)?;
        if effective_activate {
            self.persist_active_provider(&config)?;
        }
        self.provider_view(config)
    }

    pub fn activate_provider(
        &self,
        provider_id: &str,
    ) -> Result<ProviderCatalogView, AppStateError> {
        if self.credentials.get_api_key(provider_id)?.is_none() {
            return Err(AppStateError::ProviderNotConfigured(
                "the provider API key is missing".to_string(),
            ));
        }
        let config = self.provider_config.activate(provider_id)?;
        self.persist_active_provider(&config)?;
        self.provider_catalog()
    }

    pub fn delete_provider(&self, provider_id: &str) -> Result<ProviderCatalogView, AppStateError> {
        let deleted_active_provider = self
            .provider_config
            .load()?
            .is_some_and(|provider| provider.id == provider_id);
        self.provider_config.delete(provider_id)?;
        self.credentials.delete_api_key(provider_id)?;
        if deleted_active_provider {
            let mut replacement = None;
            for provider in self.provider_config.list()?.1 {
                if self.credentials.get_api_key(&provider.id)?.is_some() {
                    replacement = Some(provider);
                    break;
                }
            }
            if let Some(replacement) = replacement {
                self.provider_config.activate(&replacement.id)?;
                self.persist_active_provider(&replacement)?;
            }
        }
        self.provider_catalog()
    }

    pub fn delete_provider_api_key(&self, provider_id: &str) -> Result<(), AppStateError> {
        self.credentials.delete_api_key(provider_id)?;
        Ok(())
    }

    fn persist_active_provider(&self, config: &ProviderConfig) -> Result<(), AppStateError> {
        self.repository
            .projection()
            .set_setting(
                "provider",
                &serde_json::to_string(config)
                    .map_err(|error| AppStateError::ProviderNotConfigured(error.to_string()))?,
            )
            .map_err(|error| AppStateError::Storage(StorageError::Io(error.to_string())))
    }

    pub fn provider_context_limit(&self) -> Result<usize, AppStateError> {
        Ok(self
            .provider_config
            .load()?
            .map(|config| config.active_model().context_window as usize)
            .unwrap_or(crate::context::DEFAULT_CONTEXT_LIMIT))
    }

    pub fn build_provider(&self) -> Result<(Arc<dyn Provider>, String, usize), AppStateError> {
        self.build_provider_for(None)
    }

    pub fn build_provider_for(
        &self,
        provider_id: Option<&str>,
    ) -> Result<(Arc<dyn Provider>, String, usize), AppStateError> {
        let config = if let Some(provider_id) = provider_id {
            self.provider_config
                .list()?
                .1
                .into_iter()
                .find(|provider| provider.id == provider_id)
        } else {
            self.provider_config.load()?
        }
        .ok_or_else(|| {
            AppStateError::ProviderNotConfigured(
                "configure a provider before starting a turn".to_string(),
            )
        })?;
        let api_key = self.credentials.get_api_key(&config.id)?.ok_or_else(|| {
            AppStateError::ProviderNotConfigured("the provider API key is missing".to_string())
        })?;
        let model = config.model.clone();
        let context_limit = config.active_model().context_window as usize;
        let mut target_specs = vec![(config.base_url.clone(), model.clone(), config.name.clone())];
        for fallback in config.fallback_models() {
            target_specs.push((
                config.base_url.clone(),
                fallback.id.clone(),
                format!("{} / {}", config.name, fallback.display_name),
            ));
        }
        for endpoint in config.enabled_endpoints() {
            target_specs.push((
                endpoint.base_url.clone(),
                model.clone(),
                endpoint.name.clone(),
            ));
            for fallback in config.fallback_models() {
                target_specs.push((
                    endpoint.base_url.clone(),
                    fallback.id.clone(),
                    format!("{} / {}", endpoint.name, fallback.display_name),
                ));
            }
        }
        let mut seen = std::collections::HashSet::new();
        target_specs.retain(|(base_url, model, _)| seen.insert((base_url.clone(), model.clone())));
        let mut targets = Vec::new();
        for (base_url, target_model, label) in target_specs {
            let mut target_config = config.clone();
            target_config.base_url = base_url;
            target_config.model = target_model.clone();
            targets.push(FallbackTarget {
                provider: Self::provider_for_config(target_config, api_key.clone())?,
                model: target_model,
                label,
            });
        }
        let provider = if targets.len() == 1 {
            targets.remove(0).provider
        } else {
            Arc::new(FallbackProvider::new(
                targets,
                self.advanced.metrics.clone(),
            )?) as Arc<dyn Provider>
        };
        Ok((provider, model, context_limit))
    }

    fn provider_for_config(
        config: ProviderConfig,
        api_key: String,
    ) -> Result<Arc<dyn Provider>, AppStateError> {
        let provider: Arc<dyn Provider> = match config.transport {
            ProviderTransport::OpenAiChatCompletions => {
                Arc::new(OpenAiChatCompletionsProvider::new(config, api_key)?)
            }
            ProviderTransport::OpenAiResponses => {
                Arc::new(OpenAiResponsesProvider::new(config, api_key)?)
            }
            ProviderTransport::AnthropicMessages => {
                Arc::new(AnthropicMessagesProvider::new(config, api_key)?)
            }
            ProviderTransport::GoogleGemini => {
                Arc::new(GoogleGeminiProvider::new(config, api_key)?)
            }
        };
        Ok(provider)
    }

    pub async fn begin_turn(&self, thread_id: &str) -> Result<CancellationToken, AppStateError> {
        let _recovery_guard = self.recovery_lock.lock().await;
        let mut active_turns = self.active_turns.lock().await;
        if active_turns.contains_key(thread_id) {
            return Err(AppStateError::TurnAlreadyActive(thread_id.to_string()));
        }
        let cancellation = CancellationToken::new();
        active_turns.insert(thread_id.to_string(), cancellation.clone());
        Ok(cancellation)
    }

    pub async fn finish_turn(&self, thread_id: &str) {
        self.active_turns.lock().await.remove(thread_id);
    }

    pub async fn cancel_turn(&self, thread_id: &str) -> bool {
        let active_turns = self.active_turns.lock().await;
        if let Some(cancellation) = active_turns.get(thread_id) {
            cancellation.cancel();
            self.subagents.cancel_for_parent(thread_id);
            true
        } else {
            false
        }
    }

    pub async fn is_turn_active(&self, thread_id: &str) -> bool {
        self.active_turns.lock().await.contains_key(thread_id)
    }

    pub async fn read_thread(
        &self,
        thread_id: &str,
    ) -> Result<crate::storage::ThreadDetail, AppStateError> {
        let _recovery_guard = self.recovery_lock.lock().await;
        let detail = self.repository.read_thread(thread_id).await?;
        let Some(last_turn) = &detail.last_turn else {
            return Ok(detail);
        };
        if !matches!(
            last_turn.state,
            TurnState::Queued
                | TurnState::Streaming
                | TurnState::AwaitingApproval
                | TurnState::RunningTool
        ) || self.active_turns.lock().await.contains_key(thread_id)
        {
            return Ok(detail);
        }

        for approval in detail
            .approvals
            .iter()
            .filter(|approval| approval.resolution.is_none())
        {
            self.repository
                .append(StoredEvent::new(
                    thread_id,
                    Some(last_turn.turn_id.clone()),
                    StoredEventKind::ApprovalResolved {
                        request_id: approval.request.id.clone(),
                        resolution: ApprovalResolution {
                            action: ApprovalAction::Cancelled,
                            patch: None,
                            selected_paths: Vec::new(),
                            expected_hashes: Vec::new(),
                        },
                    },
                ))
                .await?;
        }
        for input in detail
            .user_inputs
            .iter()
            .filter(|input| input.resolution.is_none())
        {
            self.repository
                .append(StoredEvent::new(
                    thread_id,
                    Some(last_turn.turn_id.clone()),
                    StoredEventKind::UserInputResolved {
                        request_id: input.request.id.clone(),
                        resolution: UserInputResolution {
                            action: UserInputAction::Cancelled,
                            answers: Vec::new(),
                        },
                    },
                ))
                .await?;
        }
        self.repository
            .append(StoredEvent::new(
                thread_id,
                Some(last_turn.turn_id.clone()),
                StoredEventKind::TurnCancelled,
            ))
            .await?;
        Ok(self.repository.read_thread(thread_id).await?)
    }

    pub async fn undo_change(
        &self,
        thread_id: &str,
        change_id: &str,
    ) -> Result<ChangeSet, AppStateError> {
        let events = self.repository.load(thread_id).await?;
        let mut change_set = None;
        let mut undone = false;
        for event in events {
            match event.kind {
                StoredEventKind::ChangeApplied { change_set: change } if change.id == change_id => {
                    change_set = Some(change);
                }
                StoredEventKind::ChangeUndone {
                    change_id: undone_id,
                } if undone_id == change_id => {
                    undone = true;
                }
                _ => {}
            }
        }
        if undone {
            return Err(AppStateError::ChangeAlreadyUndone(change_id.to_string()));
        }
        let change_set =
            change_set.ok_or_else(|| AppStateError::ChangeNotFound(change_id.to_string()))?;
        let undone_change = self
            .patch_service
            .undo(self.workspace_root(), change_set)
            .await?;
        if let Err(error) = self
            .repository
            .append(StoredEvent::new(
                thread_id,
                Some(undone_change.turn_id.clone()),
                StoredEventKind::ChangeUndone {
                    change_id: change_id.to_string(),
                },
            ))
            .await
        {
            let storage_error = error.to_string();
            if let Err(redo_error) = self
                .patch_service
                .redo(self.workspace_root(), undone_change.clone())
                .await
            {
                return Err(AppStateError::UndoAuditCompensation {
                    storage_error,
                    redo_error: redo_error.to_string(),
                });
            }
            return Err(error.into());
        }
        Ok(undone_change)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    #[error(transparent)]
    Execution(#[from] ExecutionError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    ProviderConfig(#[from] ProviderConfigError),
    #[error(transparent)]
    Provider(#[from] crate::providers::ProviderError),
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error(transparent)]
    Patch(#[from] PatchError),
    #[error(transparent)]
    Extension(#[from] ExtensionError),
    #[error(transparent)]
    Tool(#[from] crate::tools::ToolError),
    #[error("multi-agent runtime failed: {0}")]
    MultiAgent(String),
    #[error("advanced agent runtime failed: {0}")]
    Advanced(String),
    #[error("provider is not configured: {0}")]
    ProviderNotConfigured(String),
    #[error("a turn is already active for thread {0}")]
    TurnAlreadyActive(String),
    #[error("stop active turns and subagents before changing the approval mode")]
    ApprovalModeBusy,
    #[error("approval mode lock poisoned")]
    ApprovalModeLock,
    #[error("reasoning effort lock poisoned")]
    ReasoningEffortLock,
    #[error("workspace is invalid: {0}")]
    Workspace(String),
    #[error("structured logging failed: {0}")]
    Logging(String),
    #[error("change was not found: {0}")]
    ChangeNotFound(String),
    #[error("change was already undone: {0}")]
    ChangeAlreadyUndone(String),
    #[error(
        "undo audit failed: {storage_error}; restoring the applied change also failed: {redo_error}"
    )]
    UndoAuditCompensation {
        storage_error: String,
        redo_error: String,
    },
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::policy::PolicyDecision;
    use crate::protocol::{
        ApprovalRequest, ExpectedFileHash, ToolRisk, UserInputQuestion, UserInputRequest,
    };
    use crate::providers::{ProviderKind, ProviderTransport, SaveProviderConfigRequest};

    #[derive(Default)]
    struct FakeCredentials {
        api_keys: StdMutex<HashMap<String, String>>,
    }

    impl CredentialStore for FakeCredentials {
        fn get_api_key(&self, provider_id: &str) -> Result<Option<String>, CredentialError> {
            Ok(self.api_keys.lock().unwrap().get(provider_id).cloned())
        }

        fn set_api_key(&self, provider_id: &str, api_key: &str) -> Result<(), CredentialError> {
            self.api_keys
                .lock()
                .unwrap()
                .insert(provider_id.to_string(), api_key.to_string());
            Ok(())
        }

        fn delete_api_key(&self, provider_id: &str) -> Result<(), CredentialError> {
            self.api_keys.lock().unwrap().remove(provider_id);
            Ok(())
        }
    }

    #[test]
    fn provider_view_never_returns_the_api_key() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let credentials = Arc::new(FakeCredentials::default());
        let state = AppState::with_credentials(directory.path(), credentials.clone())
            .expect("state should initialize");

        let view = state
            .save_provider_config(SaveProviderConfigRequest {
                id: "primary".to_string(),
                kind: ProviderKind::OpenAiCompatible,
                transport: ProviderTransport::OpenAiChatCompletions,
                name: "测试供应商".to_string(),
                base_url: "https://example.com/v1".to_string(),
                model: "test-model".to_string(),
                models: vec![
                    crate::providers::ProviderModelConfig {
                        id: "test-model".to_string(),
                        display_name: "Test model".to_string(),
                        context_window: 128_000,
                        max_output_tokens: None,
                        supports_vision: false,
                        fallback: false,
                    },
                    crate::providers::ProviderModelConfig {
                        id: "test-model-fast".to_string(),
                        display_name: "Test model fast".to_string(),
                        context_window: 64_000,
                        max_output_tokens: None,
                        supports_vision: false,
                        fallback: true,
                    },
                ],
                endpoints: vec![],
                api_key: Some("super-secret".to_string()),
                activate: true,
            })
            .expect("configuration should save");
        let serialized = serde_json::to_string(&view).expect("view should serialize");

        assert!(view.has_api_key);
        assert_eq!(view.name, "测试供应商");
        assert_eq!(view.models.len(), 2);
        assert_eq!(view.models[0].id, "test-model");
        assert_eq!(view.models[0].display_name, "Test model");
        assert_eq!(view.models[0].context_window, 128_000);
        assert_eq!(state.provider_context_limit().unwrap(), 128_000);
        assert!(!serialized.contains("super-secret"));
        assert_eq!(
            credentials.get_api_key("primary").unwrap().as_deref(),
            Some("super-secret")
        );
    }

    #[test]
    fn provider_catalog_switches_configs_without_sharing_credentials() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let credentials = Arc::new(FakeCredentials::default());
        let state = AppState::with_credentials(directory.path(), credentials)
            .expect("state should initialize");

        for (id, name, activate) in [("first", "First", true), ("second", "Second", false)] {
            state
                .save_provider_config(SaveProviderConfigRequest {
                    id: id.to_string(),
                    kind: ProviderKind::OpenAiCompatible,
                    transport: ProviderTransport::OpenAiChatCompletions,
                    name: name.to_string(),
                    base_url: format!("https://{id}.example.com/v1"),
                    model: "test-model".to_string(),
                    models: vec![],
                    endpoints: vec![],
                    api_key: Some(format!("{id}-secret")),
                    activate,
                })
                .expect("provider should save");
        }

        let catalog = state.provider_catalog().unwrap();
        assert_eq!(catalog.active_provider_id.as_deref(), Some("first"));
        assert_eq!(catalog.providers.len(), 2);
        assert!(
            catalog
                .providers
                .iter()
                .all(|provider| provider.has_api_key)
        );

        state
            .save_provider_config(SaveProviderConfigRequest {
                id: "pending".to_string(),
                kind: ProviderKind::OpenAiCompatible,
                transport: ProviderTransport::OpenAiChatCompletions,
                name: "Pending".to_string(),
                base_url: "https://pending.example.com/v1".to_string(),
                model: "test-model".to_string(),
                models: vec![],
                endpoints: vec![],
                api_key: None,
                activate: false,
            })
            .expect("an inactive provider may be saved before its key is configured");
        assert!(matches!(
            state.activate_provider("pending"),
            Err(AppStateError::ProviderNotConfigured(_))
        ));

        let switched = state.activate_provider("second").unwrap();
        assert_eq!(switched.active_provider_id.as_deref(), Some("second"));
        assert_eq!(state.provider_config().unwrap().unwrap().name, "Second");

        let after_first_delete = state.delete_provider("second").unwrap();
        assert_eq!(
            after_first_delete.active_provider_id.as_deref(),
            Some("first")
        );
        let after_second_delete = state.delete_provider("first").unwrap();
        assert_eq!(after_second_delete.active_provider_id, None);
        assert_eq!(after_second_delete.providers.len(), 1);
    }

    #[tokio::test]
    async fn enforces_turn_exclusivity_per_thread() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let state =
            AppState::with_credentials(directory.path(), Arc::new(FakeCredentials::default()))
                .expect("state should initialize");

        state.begin_turn("thread").await.expect("first turn starts");
        assert!(matches!(
            state.begin_turn("thread").await,
            Err(AppStateError::TurnAlreadyActive(_))
        ));
        state
            .begin_turn("other-thread")
            .await
            .expect("a different thread may run concurrently");
        assert!(state.is_turn_active("thread").await);
        assert!(state.is_turn_active("other-thread").await);
        assert!(state.cancel_turn("thread").await);
        state.finish_turn("thread").await;
        state.finish_turn("other-thread").await;
    }

    #[tokio::test]
    async fn approval_mode_persists_and_cannot_change_during_a_turn() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let credentials = Arc::new(FakeCredentials::default());
        let state = AppState::with_credentials(directory.path(), credentials.clone())
            .expect("state should initialize");

        assert_eq!(state.approval_mode(), ApprovalMode::Ask);
        state.begin_turn("thread").await.expect("turn starts");
        assert!(matches!(
            state.set_approval_mode(ApprovalMode::FullAccess).await,
            Err(AppStateError::ApprovalModeBusy)
        ));
        state.finish_turn("thread").await;
        assert_eq!(
            state
                .set_approval_mode(ApprovalMode::FullAccess)
                .await
                .unwrap(),
            ApprovalMode::FullAccess
        );

        let restored = AppState::with_credentials(directory.path(), credentials)
            .expect("state should restore");
        assert_eq!(restored.approval_mode(), ApprovalMode::FullAccess);
    }

    #[tokio::test]
    async fn reasoning_effort_persists_and_applies_to_future_turns() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let credentials = Arc::new(FakeCredentials::default());
        let state = AppState::with_credentials(directory.path(), credentials.clone())
            .expect("state should initialize");

        assert_eq!(state.reasoning_effort(), ReasoningEffort::Medium);
        assert_eq!(
            state
                .set_reasoning_effort(ReasoningEffort::High)
                .await
                .unwrap(),
            ReasoningEffort::High
        );
        let restored = AppState::with_credentials(directory.path(), credentials)
            .expect("state should restore");
        assert_eq!(restored.reasoning_effort(), ReasoningEffort::High);

        state.begin_turn("thread").await.unwrap();
        assert_eq!(
            state
                .set_reasoning_effort(ReasoningEffort::Low)
                .await
                .unwrap(),
            ReasoningEffort::Low
        );
        state.cancel_turn("thread").await;
        state.finish_turn("thread").await;
    }

    #[tokio::test]
    async fn switching_workspace_updates_every_runtime_and_persists_the_selection() {
        let data = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let state = AppState::with_workspace_and_credentials(
            data.path(),
            first.path(),
            Arc::new(FakeCredentials::default()),
        )
        .unwrap();

        let switched = state.switch_workspace(second.path()).await.unwrap();
        assert_eq!(state.workspace_root(), switched);
        assert_eq!(state.command_runtime().root(), switched);
        assert_eq!(state.pty_runtime().root(), switched);
        assert_eq!(
            state
                .repository()
                .projection()
                .setting("active_workspace")
                .unwrap()
                .as_deref(),
            Some(switched.to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    async fn changed_extension_configuration_is_reloaded_and_fails_closed() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = AppState::with_workspace_and_credentials(
            data.path(),
            workspace.path(),
            Arc::new(FakeCredentials::default()),
        )
        .unwrap();
        state.prepare_extensions(false).await.unwrap();
        std::fs::create_dir_all(workspace.path().join(".k-coder")).unwrap();
        std::fs::write(workspace.path().join(".k-coder/extensions.json"), "{broken").unwrap();
        assert!(state.prepare_extensions(false).await.is_err());
        assert!(state.prepare_extensions(false).await.is_err());
        std::fs::write(
            workspace.path().join(".k-coder/extensions.json"),
            r#"{"mcpServers":[],"hooks":[]}"#,
        )
        .unwrap();
        state.prepare_extensions(false).await.unwrap();
    }

    #[tokio::test]
    async fn extension_refresh_keeps_repository_search_authorized() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = AppState::with_workspace_and_credentials(
            data.path(),
            workspace.path(),
            Arc::new(FakeCredentials::default()),
        )
        .unwrap();

        state.prepare_extensions(false).await.unwrap();
        let authorization = state
            .tool_registry()
            .authorization(
                "search_repository",
                &serde_json::json!({ "query": "needle" }),
            )
            .unwrap();

        assert_eq!(authorization.decision, PolicyDecision::Allow);
        assert_eq!(authorization.risk, ToolRisk::Read);
    }

    #[tokio::test]
    async fn undo_restores_the_snapshot_and_persists_the_audit_event() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("review.txt"), "before\n").unwrap();
        let state = AppState::with_workspace_and_credentials(
            data.path(),
            workspace.path(),
            Arc::new(FakeCredentials::default()),
        )
        .unwrap();
        let thread = state.repository().create_thread().await.unwrap();
        let patch =
            "*** Begin Patch\n*** Update File: review.txt\n@@\n-before\n+after\n*** End Patch\n";
        let preview = state
            .patch_service()
            .preview_patch(workspace.path(), patch)
            .unwrap();
        let expected_hashes = preview
            .files
            .iter()
            .map(|file| ExpectedFileHash {
                path: file.path.clone(),
                before_hash: file.before_hash.clone(),
            })
            .collect();
        let selected_paths = preview.files.iter().map(|file| file.path.clone()).collect();
        let change = state
            .patch_service()
            .apply_patch(
                workspace.path().to_path_buf(),
                thread.id.clone(),
                "turn-1".to_string(),
                "call-1".to_string(),
                patch.to_string(),
                selected_paths,
                expected_hashes,
            )
            .await
            .unwrap();
        state
            .repository()
            .append(StoredEvent::new(
                &thread.id,
                Some("turn-1".to_string()),
                StoredEventKind::ChangeApplied {
                    change_set: change.clone(),
                },
            ))
            .await
            .unwrap();

        let undone = state.undo_change(&thread.id, &change.id).await.unwrap();
        assert!(undone.undone);
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("review.txt")).unwrap(),
            "before\n"
        );
        let detail = state.repository().read_thread(&thread.id).await.unwrap();
        assert!(detail.changes[0].undone);
        assert!(matches!(
            state.undo_change(&thread.id, &change.id).await,
            Err(AppStateError::ChangeAlreadyUndone(_))
        ));
    }

    #[tokio::test]
    async fn recovers_all_orphaned_approvals_as_cancelled_once() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = AppState::with_workspace_and_credentials(
            data.path(),
            workspace.path(),
            Arc::new(FakeCredentials::default()),
        )
        .unwrap();
        let thread = state.repository().create_thread().await.unwrap();
        let turn_id = "interrupted-turn".to_string();
        let request = ApprovalRequest {
            id: "interrupted-approval".to_string(),
            thread_id: thread.id.clone(),
            turn_id: turn_id.clone(),
            tool_call_id: "call".to_string(),
            tool_name: "apply_patch".to_string(),
            reason: "review".to_string(),
            auto_approved: false,
            risk: ToolRisk::Write,
            arguments: serde_json::json!({ "patch": "strict patch" }),
            preview: None,
            created_at_ms: 1,
            expires_at_ms: 2,
        };
        for kind in [
            StoredEventKind::TurnStarted,
            StoredEventKind::ApprovalRequested {
                request: request.clone(),
            },
            StoredEventKind::ApprovalRequested {
                request: ApprovalRequest {
                    id: "interrupted-approval-2".to_string(),
                    tool_call_id: "call-2".to_string(),
                    ..request.clone()
                },
            },
            StoredEventKind::UserInputRequested {
                request: UserInputRequest {
                    id: "interrupted-input".to_string(),
                    thread_id: thread.id.clone(),
                    turn_id: turn_id.clone(),
                    tool_call_id: "call-input".to_string(),
                    questions: vec![UserInputQuestion {
                        question: "继续吗".to_string(),
                        options: vec!["继续".to_string(), "停止".to_string()],
                    }],
                    created_at_ms: 1,
                    expires_at_ms: 2,
                },
            },
        ] {
            state
                .repository()
                .append(StoredEvent::new(&thread.id, Some(turn_id.clone()), kind))
                .await
                .unwrap();
        }

        let detail = state.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.last_turn.unwrap().state, TurnState::Cancelled);
        assert_eq!(detail.approvals.len(), 2);
        assert!(detail.approvals.iter().all(|approval| {
            approval
                .resolution
                .as_ref()
                .is_some_and(|resolution| resolution.action == ApprovalAction::Cancelled)
        }));
        assert_eq!(detail.user_inputs.len(), 1);
        assert!(
            detail.user_inputs[0]
                .resolution
                .as_ref()
                .is_some_and(|resolution| resolution.action == UserInputAction::Cancelled)
        );
        let event_count = state.repository().load(&thread.id).await.unwrap().len();
        let detail = state.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.last_turn.unwrap().state, TurnState::Cancelled);
        assert_eq!(
            state.repository().load(&thread.id).await.unwrap().len(),
            event_count
        );
    }
}
