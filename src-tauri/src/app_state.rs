use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::advanced::AdvancedServices;
use crate::agent::mailbox::{MailboxTurn, QueuedTurnSteerError, ThreadMailbox, TurnControl};
use crate::agent::thread_operation::{ThreadOperationGate, ThreadOperationGuard};
use crate::execution::{BundledTools, CommandRuntime, ExecutionError, NativePtyRuntime};
use crate::extensions::mcp::OsMcpSecretStore;
use crate::extensions::{ExtensionError, ExtensionOverview, ExtensionService};
use crate::logging::StructuredLogger;
use crate::multi_agent::MultiAgentCoordinator;
use crate::patch::{PatchError, PatchService};
use crate::persistence::ProjectionDb;
use crate::policy::{ApprovalManager, UserInputManager};
use crate::protocol::{
    AgentItemStatus, AgentItemType, ApprovalAction, ApprovalMode, ApprovalResolution, ChangeSet,
    HistorySortDirection, ReasoningEffort, ThreadHistorySnapshot, ThreadItemsPage, ThreadTurnsPage,
    TurnItemsView, TurnState, UserInputAction, UserInputResolution,
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
    bundled_tools: Option<BundledTools>,
    logger: StructuredLogger,
    active_turns: Mutex<HashMap<String, ActiveTurn>>,
    thread_mailbox: ThreadMailbox,
    thread_operations: ThreadOperationGate,
    recovery_lock: Mutex<()>,
    extensions: ExtensionService,
    extension_workspace: Mutex<Option<(PathBuf, u64)>>,
    subagents: MultiAgentCoordinator,
    advanced: AdvancedServices,
}

#[derive(Debug)]
struct ActiveTurn {
    turn_id: String,
    cancellation: CancellationToken,
    control: Arc<TurnControl>,
}

impl AppState {
    pub fn new(data_root: impl AsRef<Path>) -> Result<Self, AppStateError> {
        Self::with_credentials(data_root, Arc::new(OsCredentialStore::new()))
    }

    pub fn new_with_builtin_skills(
        data_root: impl AsRef<Path>,
        builtin_skills_root: impl AsRef<Path>,
    ) -> Result<Self, AppStateError> {
        Self::with_credentials_and_builtin_skills(
            data_root,
            Arc::new(OsCredentialStore::new()),
            Some(builtin_skills_root.as_ref().to_path_buf()),
            None,
        )
    }

    pub fn new_with_builtin_resources(
        data_root: impl AsRef<Path>,
        builtin_skills_root: impl AsRef<Path>,
        bundled_tools_root: Option<PathBuf>,
    ) -> Result<Self, AppStateError> {
        Self::with_credentials_and_builtin_skills(
            data_root,
            Arc::new(OsCredentialStore::new()),
            Some(builtin_skills_root.as_ref().to_path_buf()),
            bundled_tools_root,
        )
    }

    pub fn with_credentials(
        data_root: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, AppStateError> {
        Self::with_credentials_and_builtin_skills(data_root, credentials, None, None)
    }

    fn with_credentials_and_builtin_skills(
        data_root: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
        builtin_skills_root: Option<PathBuf>,
        bundled_tools_root: Option<PathBuf>,
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
        Self::with_workspace_credentials_and_builtin_skills(
            data_root,
            workspace_root,
            credentials,
            builtin_skills_root,
            bundled_tools_root,
        )
    }

    pub fn with_workspace_and_credentials(
        data_root: impl AsRef<Path>,
        workspace_root: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, AppStateError> {
        Self::with_workspace_credentials_and_builtin_skills(
            data_root,
            workspace_root,
            credentials,
            None,
            None,
        )
    }

    fn with_workspace_credentials_and_builtin_skills(
        data_root: impl AsRef<Path>,
        workspace_root: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
        builtin_skills_root: Option<PathBuf>,
        bundled_tools_root: Option<PathBuf>,
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
        let bundled_tools = bundled_tools_root.map(BundledTools::new).transpose()?;
        let command_runtime = CommandRuntime::with_recovery_and_bundled_tools(
            &workspace_root,
            &data_root,
            bundled_tools.clone(),
        )?;
        let pty_runtime =
            NativePtyRuntime::new_with_bundled_tools(&workspace_root, bundled_tools.clone())?;
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
        let extensions = ExtensionService::with_builtin_skills(
            data_root.clone(),
            builtin_skills_root,
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
            bundled_tools,
            logger,
            active_turns: Mutex::new(HashMap::new()),
            thread_mailbox: ThreadMailbox::default(),
            thread_operations: ThreadOperationGate::default(),
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

    pub fn thread_mailbox(&self) -> &ThreadMailbox {
        &self.thread_mailbox
    }

    pub async fn enqueue_thread_turn(&self, item: MailboxTurn) -> bool {
        let thread_id = item.handle.thread_id.clone();
        let _operation_guard = self.thread_operations.lock(&thread_id).await;
        self.thread_mailbox.enqueue(item).await
    }

    pub async fn next_thread_turn(
        &self,
        thread_id: &str,
    ) -> Option<(MailboxTurn, ThreadOperationGuard)> {
        let operation_guard = self.thread_operations.lock(thread_id).await;
        self.thread_mailbox
            .next(thread_id)
            .await
            .map(|item| (item, operation_guard))
    }

    pub async fn remove_queued_turn(&self, thread_id: &str, turn_id: &str) -> bool {
        let _operation_guard = self.thread_operations.lock(thread_id).await;
        self.thread_mailbox.remove(thread_id, turn_id).await
    }

    pub async fn clear_thread_mailbox(&self, thread_id: &str) -> usize {
        let _operation_guard = self.thread_operations.lock(thread_id).await;
        self.thread_mailbox.clear(thread_id).await
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
        let command = CommandRuntime::with_recovery_and_bundled_tools(
            &path,
            &self.data_root,
            self.bundled_tools.clone(),
        )?;
        let pty = NativePtyRuntime::new_with_bundled_tools(&path, self.bundled_tools.clone())?;
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
        self.build_provider_for_capability(None, false)
    }

    pub fn build_provider_for(
        &self,
        provider_id: Option<&str>,
    ) -> Result<(Arc<dyn Provider>, String, usize), AppStateError> {
        self.build_provider_for_capability(provider_id, false)
    }

    pub fn build_vision_provider(
        &self,
    ) -> Result<(Arc<dyn Provider>, String, usize), AppStateError> {
        self.build_provider_for_capability(None, true)
    }

    pub fn active_model_supports_vision(&self) -> Result<bool, AppStateError> {
        Ok(self
            .provider_config
            .load()?
            .map(|config| config.active_model().supports_vision)
            .unwrap_or(false))
    }

    fn build_provider_for_capability(
        &self,
        provider_id: Option<&str>,
        requires_vision: bool,
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
        let target_specs = provider_target_specs(&config, requires_vision);
        if target_specs.is_empty() {
            return Err(AppStateError::ProviderNotConfigured(
                "the active model does not support image input".to_string(),
            ));
        }
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
        let workspace = self.workspace_root();
        self.begin_turn_in_workspace(thread_id, &workspace).await
    }

    pub async fn begin_turn_in_workspace(
        &self,
        thread_id: &str,
        expected_workspace: &Path,
    ) -> Result<CancellationToken, AppStateError> {
        let (cancellation, _) = self
            .begin_turn_with_id_in_workspace(
                thread_id,
                &Uuid::new_v4().to_string(),
                expected_workspace,
            )
            .await?;
        Ok(cancellation)
    }

    pub async fn begin_turn_with_id_in_workspace(
        &self,
        thread_id: &str,
        turn_id: &str,
        expected_workspace: &Path,
    ) -> Result<(CancellationToken, Arc<TurnControl>), AppStateError> {
        let operation_guard = self.thread_operations.lock(thread_id).await;
        self.begin_turn_with_id_in_workspace_locked(
            thread_id,
            turn_id,
            expected_workspace,
            &operation_guard,
        )
        .await
    }

    pub async fn begin_turn_with_id_in_workspace_locked(
        &self,
        thread_id: &str,
        turn_id: &str,
        expected_workspace: &Path,
        _operation_guard: &ThreadOperationGuard,
    ) -> Result<(CancellationToken, Arc<TurnControl>), AppStateError> {
        let _recovery_guard = self.recovery_lock.lock().await;
        let mut active_turns = self.active_turns.lock().await;
        if active_turns.contains_key(thread_id) {
            return Err(AppStateError::TurnAlreadyActive(thread_id.to_string()));
        }
        let current_workspace = self.workspace_root();
        if current_workspace != expected_workspace {
            return Err(AppStateError::ThreadWorkspaceMismatch {
                thread_id: thread_id.to_string(),
                expected: expected_workspace.to_path_buf(),
                actual: current_workspace,
            });
        }
        let cancellation = CancellationToken::new();
        let control = TurnControl::new();
        active_turns.insert(
            thread_id.to_string(),
            ActiveTurn {
                turn_id: turn_id.to_string(),
                cancellation: cancellation.clone(),
                control: control.clone(),
            },
        );
        Ok((cancellation, control))
    }

    pub async fn fork_thread(
        &self,
        thread_id: &str,
        last_turn_id: Option<&str>,
    ) -> Result<crate::storage::ThreadSummary, AppStateError> {
        let _operation_guard = self.thread_operations.lock(thread_id).await;
        if self.active_turns.lock().await.contains_key(thread_id) {
            return Err(AppStateError::ThreadOperationBusy(thread_id.to_string()));
        }
        if !self
            .thread_mailbox
            .snapshot(thread_id, None)
            .await
            .pending
            .is_empty()
        {
            return Err(AppStateError::ThreadMailboxNotEmpty(thread_id.to_string()));
        }
        Ok(self.repository.fork_thread(thread_id, last_turn_id).await?)
    }

    pub async fn rollback_thread(
        &self,
        thread_id: &str,
        num_turns: u32,
    ) -> Result<ThreadHistorySnapshot, AppStateError> {
        let _operation_guard = self.thread_operations.lock(thread_id).await;
        if self.active_turns.lock().await.contains_key(thread_id) {
            return Err(AppStateError::ThreadOperationBusy(thread_id.to_string()));
        }
        if !self
            .thread_mailbox
            .snapshot(thread_id, None)
            .await
            .pending
            .is_empty()
        {
            return Err(AppStateError::ThreadMailboxNotEmpty(thread_id.to_string()));
        }
        Ok(self
            .repository
            .rollback_thread(thread_id, num_turns)
            .await?)
    }

    pub async fn resume_thread_history(
        &self,
        thread_id: &str,
    ) -> Result<ThreadHistorySnapshot, AppStateError> {
        let _operation_guard = self.thread_operations.lock(thread_id).await;
        self.read_thread_history(thread_id).await
    }

    pub async fn resolve_thread_workspace(
        &self,
        thread_id: &str,
    ) -> Result<Option<PathBuf>, AppStateError> {
        let current_workspace = self.workspace_root();
        let detail = self.repository.read_thread(thread_id).await?;
        if !detail.summary.in_project {
            return Ok(None);
        }
        let Some(bound_path) = detail.summary.workspace_path else {
            self.repository
                .bind_thread_workspace(thread_id, &current_workspace)
                .await?;
            return Ok(Some(current_workspace));
        };
        let bound_workspace = PathBuf::from(&bound_path).canonicalize().map_err(|error| {
            AppStateError::Workspace(format!(
                "bound workspace {bound_path} cannot be resolved: {error}"
            ))
        })?;
        if bound_workspace != current_workspace {
            return Err(AppStateError::ThreadWorkspaceMismatch {
                thread_id: thread_id.to_string(),
                expected: bound_workspace,
                actual: current_workspace,
            });
        }
        Ok(Some(current_workspace))
    }

    pub async fn ensure_thread_workspace(&self, thread_id: &str) -> Result<PathBuf, AppStateError> {
        self.resolve_thread_workspace(thread_id)
            .await?
            .ok_or_else(|| AppStateError::ThreadHasNoWorkspace(thread_id.to_string()))
    }

    pub async fn finish_turn(&self, thread_id: &str) {
        if let Some(active) = self.active_turns.lock().await.remove(thread_id) {
            active.control.close();
        }
    }

    pub async fn cancel_turn(&self, thread_id: &str) -> bool {
        let active_turns = self.active_turns.lock().await;
        if let Some(active) = active_turns.get(thread_id) {
            active.control.close();
            active.cancellation.cancel();
            self.subagents.cancel_for_parent(thread_id);
            true
        } else {
            false
        }
    }

    pub async fn is_turn_active(&self, thread_id: &str) -> bool {
        self.active_turns.lock().await.contains_key(thread_id)
    }

    pub async fn active_turn_id(&self, thread_id: &str) -> Option<String> {
        self.active_turns
            .lock()
            .await
            .get(thread_id)
            .map(|active| active.turn_id.clone())
    }

    pub async fn steer_turn(
        &self,
        thread_id: &str,
        expected_turn_id: &str,
        message: crate::protocol::ChatMessage,
    ) -> Result<String, AppStateError> {
        let active_turns = self.active_turns.lock().await;
        let active = active_turns
            .get(thread_id)
            .ok_or_else(|| AppStateError::NoActiveTurn(thread_id.to_string()))?;
        if active.turn_id != expected_turn_id {
            return Err(AppStateError::ExpectedTurnMismatch {
                expected: expected_turn_id.to_string(),
                actual: active.turn_id.clone(),
            });
        }
        active
            .control
            .steer(message)
            .map_err(|_| AppStateError::NoActiveTurn(thread_id.to_string()))?;
        Ok(active.turn_id.clone())
    }

    pub async fn steer_queued_message(
        &self,
        thread_id: &str,
        expected_turn_id: &str,
        queued_turn_id: &str,
        message: crate::protocol::ChatMessage,
    ) -> Result<String, AppStateError> {
        // Keep the active Turn stable through mailbox acceptance. Mailbox code
        // never acquires active_turns, so this is the single lock order.
        let active_turns = self.active_turns.lock().await;
        let active = active_turns
            .get(thread_id)
            .ok_or_else(|| AppStateError::NoActiveTurn(thread_id.to_string()))?;
        if active.turn_id != expected_turn_id {
            return Err(AppStateError::ExpectedTurnMismatch {
                expected: expected_turn_id.to_string(),
                actual: active.turn_id.clone(),
            });
        }
        self.thread_mailbox
            .steer_message(thread_id, queued_turn_id, active.control.as_ref(), message)
            .await
            .map_err(|error| match error {
                QueuedTurnSteerError::NotFound => AppStateError::QueuedTurnNotFound {
                    thread_id: thread_id.to_string(),
                    turn_id: queued_turn_id.to_string(),
                },
                QueuedTurnSteerError::NotMessage => AppStateError::QueuedTurnNotMessage {
                    thread_id: thread_id.to_string(),
                    turn_id: queued_turn_id.to_string(),
                },
                QueuedTurnSteerError::TurnClosed => {
                    AppStateError::NoActiveTurn(thread_id.to_string())
                }
            })?;
        Ok(active.turn_id.clone())
    }

    pub async fn interrupt_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<(), AppStateError> {
        let active_turns = self.active_turns.lock().await;
        let active = active_turns
            .get(thread_id)
            .ok_or_else(|| AppStateError::NoActiveTurn(thread_id.to_string()))?;
        if active.turn_id != turn_id {
            return Err(AppStateError::ExpectedTurnMismatch {
                expected: turn_id.to_string(),
                actual: active.turn_id.clone(),
            });
        }
        active.control.close();
        active.cancellation.cancel();
        self.subagents.cancel_for_parent(thread_id);
        Ok(())
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
        let events = self.repository.load(thread_id).await?;
        for (item_id, item_type) in active_turn_items(&events, &last_turn.turn_id) {
            self.repository
                .append(StoredEvent::new(
                    thread_id,
                    Some(last_turn.turn_id.clone()),
                    StoredEventKind::ItemCompleted {
                        item_id,
                        item_type,
                        status: AgentItemStatus::Cancelled,
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

    pub async fn read_thread_history(
        &self,
        thread_id: &str,
    ) -> Result<ThreadHistorySnapshot, AppStateError> {
        self.read_thread(thread_id).await?;
        Ok(self.repository.read_thread_history(thread_id).await?)
    }

    pub async fn list_thread_turns(
        &self,
        thread_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
        sort_direction: HistorySortDirection,
        items_view: TurnItemsView,
    ) -> Result<ThreadTurnsPage, AppStateError> {
        self.read_thread(thread_id).await?;
        Ok(self
            .repository
            .list_thread_turns(thread_id, cursor, limit, sort_direction, items_view)
            .await?)
    }

    pub async fn list_thread_items(
        &self,
        thread_id: &str,
        turn_id: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u32>,
        sort_direction: HistorySortDirection,
    ) -> Result<ThreadItemsPage, AppStateError> {
        self.read_thread(thread_id).await?;
        Ok(self
            .repository
            .list_thread_items(thread_id, turn_id, cursor, limit, sort_direction)
            .await?)
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

fn active_turn_items(events: &[StoredEvent], turn_id: &str) -> Vec<(String, AgentItemType)> {
    let mut active = Vec::<(String, AgentItemType)>::new();
    for event in events {
        if event.turn_id.as_deref() != Some(turn_id) {
            continue;
        }
        match &event.kind {
            StoredEventKind::ItemStarted { item_id, item_type } => {
                let item = (item_id.clone(), *item_type);
                if !active.contains(&item) {
                    active.push(item);
                }
            }
            StoredEventKind::ItemCompleted {
                item_id, item_type, ..
            } => active.retain(|active_item| active_item != &(item_id.clone(), *item_type)),
            _ => {}
        }
    }
    active
}

fn provider_target_specs(
    config: &ProviderConfig,
    requires_vision: bool,
) -> Vec<(String, String, String)> {
    let active_model = config.active_model();
    let mut candidates = vec![(
        config.base_url.clone(),
        active_model.id.clone(),
        config.name.clone(),
        active_model.supports_vision,
    )];
    for fallback in config.fallback_models() {
        candidates.push((
            config.base_url.clone(),
            fallback.id.clone(),
            format!("{} / {}", config.name, fallback.display_name),
            fallback.supports_vision,
        ));
    }
    for endpoint in config.enabled_endpoints() {
        candidates.push((
            endpoint.base_url.clone(),
            active_model.id.clone(),
            endpoint.name.clone(),
            active_model.supports_vision,
        ));
        for fallback in config.fallback_models() {
            candidates.push((
                endpoint.base_url.clone(),
                fallback.id.clone(),
                format!("{} / {}", endpoint.name, fallback.display_name),
                fallback.supports_vision,
            ));
        }
    }
    if requires_vision {
        candidates.retain(|(_, _, _, supports_vision)| *supports_vision);
    }
    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|(base_url, model, _, _)| seen.insert((base_url.clone(), model.clone())))
        .map(|(base_url, model, label, _)| (base_url, model, label))
        .collect()
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
    #[error("no active turn for thread {0}")]
    NoActiveTurn(String),
    #[error("expected active turn id {expected}, but found {actual}")]
    ExpectedTurnMismatch { expected: String, actual: String },
    #[error("queued turn {turn_id} was not found for thread {thread_id}")]
    QueuedTurnNotFound { thread_id: String, turn_id: String },
    #[error("queued turn {turn_id} for thread {thread_id} is not a message")]
    QueuedTurnNotMessage { thread_id: String, turn_id: String },
    #[error("thread operation conflicts with an active turn for {0}")]
    ThreadOperationBusy(String),
    #[error("thread mailbox is not empty for {0}")]
    ThreadMailboxNotEmpty(String),
    #[error(
        "thread {thread_id} belongs to workspace {expected}, but the active workspace is {actual}"
    )]
    ThreadWorkspaceMismatch {
        thread_id: String,
        expected: PathBuf,
        actual: PathBuf,
    },
    #[error("thread {0} is not associated with a project workspace")]
    ThreadHasNoWorkspace(String),
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
    use crate::agent::mailbox::{MailboxTurn, MailboxTurnKind};
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
        assert!(!state.active_model_supports_vision().unwrap());
        assert!(matches!(
            state.build_vision_provider(),
            Err(AppStateError::ProviderNotConfigured(_))
        ));
    }

    #[test]
    fn vision_requests_exclude_text_only_fallback_models() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let credentials = Arc::new(FakeCredentials::default());
        let state = AppState::with_credentials(directory.path(), credentials)
            .expect("state should initialize");

        state
            .save_provider_config(SaveProviderConfigRequest {
                id: "vision".to_string(),
                kind: ProviderKind::OpenAiCompatible,
                transport: ProviderTransport::OpenAiChatCompletions,
                name: "Vision provider".to_string(),
                base_url: "https://example.com/v1".to_string(),
                model: "vision-model".to_string(),
                models: vec![
                    crate::providers::ProviderModelConfig {
                        id: "vision-model".to_string(),
                        display_name: "Vision model".to_string(),
                        context_window: 128_000,
                        max_output_tokens: None,
                        supports_vision: true,
                        fallback: false,
                    },
                    crate::providers::ProviderModelConfig {
                        id: "text-fallback".to_string(),
                        display_name: "Text fallback".to_string(),
                        context_window: 64_000,
                        max_output_tokens: None,
                        supports_vision: false,
                        fallback: true,
                    },
                ],
                endpoints: vec![crate::providers::ProviderEndpointConfig {
                    id: "secondary".to_string(),
                    name: "Secondary".to_string(),
                    base_url: "https://secondary.example.com/v1".to_string(),
                    enabled: true,
                }],
                api_key: Some("secret".to_string()),
                activate: true,
            })
            .expect("configuration should save");

        let config = state.provider_config.load().unwrap().unwrap();
        let all_targets = provider_target_specs(&config, false);
        let vision_targets = provider_target_specs(&config, true);
        assert_eq!(all_targets.len(), 4);
        assert_eq!(vision_targets.len(), 2);
        assert!(
            vision_targets
                .iter()
                .all(|(_, model, _)| model == "vision-model")
        );
        assert!(state.active_model_supports_vision().unwrap());
        assert!(state.build_vision_provider().is_ok());
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
    async fn lifecycle_operations_wait_for_dequeued_turn_registration() {
        let directory = tempfile::tempdir().unwrap();
        let state = Arc::new(
            AppState::with_credentials(directory.path(), Arc::new(FakeCredentials::default()))
                .unwrap(),
        );
        state
            .enqueue_thread_turn(MailboxTurn {
                handle: crate::protocol::TurnHandle {
                    schema_version: crate::protocol::PROTOCOL_VERSION,
                    thread_id: "thread".into(),
                    turn_id: "turn-dequeued".into(),
                    state: TurnState::Queued,
                },
                kind: MailboxTurnKind::Retry,
                started: None,
            })
            .await;
        let (_, operation_guard) = state
            .next_thread_turn("thread")
            .await
            .expect("queued turn should be dequeued with the operation gate held");

        let fork_state = state.clone();
        let fork = tokio::spawn(async move { fork_state.fork_thread("thread", None).await });
        tokio::task::yield_now().await;
        assert!(!fork.is_finished());

        let workspace = state.workspace_root();
        state
            .begin_turn_with_id_in_workspace_locked(
                "thread",
                "turn-dequeued",
                &workspace,
                &operation_guard,
            )
            .await
            .unwrap();
        drop(operation_guard);

        assert!(matches!(
            fork.await.unwrap(),
            Err(AppStateError::ThreadOperationBusy(thread_id)) if thread_id == "thread"
        ));
        state.finish_turn("thread").await;
    }

    #[tokio::test]
    async fn lifecycle_operations_reject_pending_mailbox_turns() {
        let directory = tempfile::tempdir().unwrap();
        let state =
            AppState::with_credentials(directory.path(), Arc::new(FakeCredentials::default()))
                .unwrap();
        let thread = state.repository().create_thread().await.unwrap();
        state
            .enqueue_thread_turn(MailboxTurn {
                handle: crate::protocol::TurnHandle {
                    schema_version: crate::protocol::PROTOCOL_VERSION,
                    thread_id: thread.id.clone(),
                    turn_id: "turn-pending".into(),
                    state: TurnState::Queued,
                },
                kind: MailboxTurnKind::Retry,
                started: None,
            })
            .await;

        assert!(matches!(
            state.fork_thread(&thread.id, None).await,
            Err(AppStateError::ThreadMailboxNotEmpty(thread_id)) if thread_id == thread.id
        ));
        assert!(matches!(
            state.rollback_thread(&thread.id, 1).await,
            Err(AppStateError::ThreadMailboxNotEmpty(thread_id)) if thread_id == thread.id
        ));
    }

    #[tokio::test]
    async fn steer_and_interrupt_require_the_exact_active_turn_id() {
        let directory = tempfile::tempdir().unwrap();
        let state =
            AppState::with_credentials(directory.path(), Arc::new(FakeCredentials::default()))
                .unwrap();
        let workspace = state.workspace_root();
        let (_, control) = state
            .begin_turn_with_id_in_workspace("thread", "turn-current", &workspace)
            .await
            .unwrap();
        let message = crate::protocol::ChatMessage {
            schema_version: crate::protocol::PROTOCOL_VERSION,
            id: "steer-message".into(),
            role: crate::protocol::MessageRole::User,
            content: Vec::new(),
            created_at_ms: 1,
        };

        assert!(matches!(
            state
                .steer_turn("thread", "turn-stale", message.clone())
                .await,
            Err(AppStateError::ExpectedTurnMismatch { .. })
        ));
        assert_eq!(
            state
                .steer_turn("thread", "turn-current", message.clone())
                .await
                .unwrap(),
            "turn-current"
        );
        assert_eq!(control.take_pending(), vec![message]);
        assert!(matches!(
            state.interrupt_turn("thread", "turn-stale").await,
            Err(AppStateError::ExpectedTurnMismatch { .. })
        ));
        state
            .interrupt_turn("thread", "turn-current")
            .await
            .unwrap();
        assert!(state.cancel_turn("thread").await);
        state.finish_turn("thread").await;
        assert!(matches!(
            state.interrupt_turn("thread", "turn-current").await,
            Err(AppStateError::NoActiveTurn(_))
        ));
    }

    #[tokio::test]
    async fn queued_steer_consumes_only_the_matching_message() {
        let directory = tempfile::tempdir().unwrap();
        let state =
            AppState::with_credentials(directory.path(), Arc::new(FakeCredentials::default()))
                .unwrap();
        let workspace = state.workspace_root();
        let (_, control) = state
            .begin_turn_with_id_in_workspace("thread", "turn-current", &workspace)
            .await
            .unwrap();
        for (turn_id, kind) in [
            (
                "queued-message",
                MailboxTurnKind::Message {
                    request: crate::agent::RunTurnRequest {
                        thread_id: "thread".into(),
                        input: "queued input".into(),
                        agent_mode: None,
                    },
                    attachments: Vec::new(),
                },
            ),
            ("queued-retry", MailboxTurnKind::Retry),
        ] {
            state
                .thread_mailbox()
                .enqueue(MailboxTurn {
                    handle: crate::protocol::TurnHandle {
                        schema_version: crate::protocol::PROTOCOL_VERSION,
                        thread_id: "thread".into(),
                        turn_id: turn_id.into(),
                        state: TurnState::Queued,
                    },
                    kind,
                    started: None,
                })
                .await;
        }
        let message = crate::protocol::ChatMessage {
            schema_version: crate::protocol::PROTOCOL_VERSION,
            id: "steered-message".into(),
            role: crate::protocol::MessageRole::User,
            content: Vec::new(),
            created_at_ms: 1,
        };

        let stale_result = state
            .steer_queued_message("thread", "turn-stale", "queued-message", message.clone())
            .await;
        assert!(matches!(
            stale_result,
            Err(AppStateError::ExpectedTurnMismatch { .. })
        ));
        assert_eq!(
            state
                .thread_mailbox()
                .snapshot("thread", Some("turn-current".into()))
                .await
                .pending
                .len(),
            2
        );
        let accepted_turn = state
            .steer_queued_message("thread", "turn-current", "queued-message", message.clone())
            .await
            .unwrap();
        assert_eq!(accepted_turn, "turn-current");
        assert_eq!(control.take_pending(), vec![message]);
        let snapshot = state
            .thread_mailbox()
            .snapshot("thread", Some("turn-current".into()))
            .await;
        assert_eq!(snapshot.pending.len(), 1);
        assert_eq!(snapshot.pending[0].turn_id, "queued-retry");
        assert!(matches!(
            state
                .steer_queued_message(
                    "thread",
                    "turn-current",
                    "queued-retry",
                    crate::protocol::ChatMessage {
                        schema_version: crate::protocol::PROTOCOL_VERSION,
                        id: "invalid-retry-steer".into(),
                        role: crate::protocol::MessageRole::User,
                        content: Vec::new(),
                        created_at_ms: 2,
                    },
                )
                .await,
            Err(AppStateError::QueuedTurnNotMessage { .. })
        ));
        assert_eq!(
            state
                .thread_mailbox()
                .snapshot("thread", Some("turn-current".into()))
                .await
                .pending
                .len(),
            1
        );
        state.finish_turn("thread").await;
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
    async fn binds_legacy_threads_and_rejects_a_different_active_workspace() {
        let data = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let state = AppState::with_workspace_and_credentials(
            data.path(),
            first.path(),
            Arc::new(FakeCredentials::default()),
        )
        .unwrap();
        let thread = state.repository().create_thread().await.unwrap();

        let bound = state.ensure_thread_workspace(&thread.id).await.unwrap();
        assert_eq!(bound, first.path().canonicalize().unwrap());
        assert_eq!(
            state
                .repository()
                .read_thread(&thread.id)
                .await
                .unwrap()
                .summary
                .workspace_path
                .as_deref(),
            Some(bound.to_string_lossy().as_ref())
        );

        state.switch_workspace(second.path()).await.unwrap();
        assert!(matches!(
            state.ensure_thread_workspace(&thread.id).await,
            Err(AppStateError::ThreadWorkspaceMismatch { .. })
        ));
        assert!(matches!(
            state.begin_turn_in_workspace(&thread.id, &bound).await,
            Err(AppStateError::ThreadWorkspaceMismatch { .. })
        ));
        assert!(!state.is_turn_active(&thread.id).await);
    }

    #[tokio::test]
    async fn leaves_standalone_threads_without_a_workspace() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = AppState::with_workspace_and_credentials(
            data.path(),
            workspace.path(),
            Arc::new(FakeCredentials::default()),
        )
        .unwrap();
        let thread = state.repository().create_standalone_thread().await.unwrap();

        assert_eq!(
            state.resolve_thread_workspace(&thread.id).await.unwrap(),
            None
        );
        assert!(matches!(
            state.ensure_thread_workspace(&thread.id).await,
            Err(AppStateError::ThreadHasNoWorkspace(_))
        ));
        assert!(
            state
                .repository()
                .read_thread(&thread.id)
                .await
                .unwrap()
                .summary
                .workspace_path
                .is_none()
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
            StoredEventKind::ItemStarted {
                item_id: request.id.clone(),
                item_type: AgentItemType::Approval,
            },
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
            StoredEventKind::ItemStarted {
                item_id: "interrupted-input".to_string(),
                item_type: AgentItemType::UserInput,
            },
            StoredEventKind::UserInputRequested {
                request: UserInputRequest {
                    id: "interrupted-input".to_string(),
                    thread_id: thread.id.clone(),
                    turn_id: turn_id.clone(),
                    tool_call_id: "call-input".to_string(),
                    kind: crate::protocol::UserInputRequestKind::ModelQuestion,
                    questions: vec![UserInputQuestion {
                        question: "继续吗".to_string(),
                        options: vec!["继续".to_string(), "停止".to_string()],
                    }],
                    created_at_ms: 1,
                    expires_at_ms: 2,
                },
            },
            StoredEventKind::ItemStarted {
                item_id: "interrupted-compaction".to_string(),
                item_type: AgentItemType::ContextCompaction,
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
        let events = state.repository().load(&thread.id).await.unwrap();
        for (item_id, item_type) in [
            ("interrupted-approval", AgentItemType::Approval),
            ("interrupted-input", AgentItemType::UserInput),
            ("interrupted-compaction", AgentItemType::ContextCompaction),
        ] {
            assert!(events.iter().any(|event| matches!(
                &event.kind,
                StoredEventKind::ItemCompleted {
                    item_id: completed_item_id,
                    item_type: completed_item_type,
                    status: AgentItemStatus::Cancelled,
                } if completed_item_id == item_id && completed_item_type == &item_type
            )));
        }
        assert!(!events.iter().any(|event| matches!(
            &event.kind,
            StoredEventKind::ItemCompleted {
                item_id,
                item_type: AgentItemType::Approval,
                ..
            } if item_id == "interrupted-approval-2"
        )));
        let event_count = events.len();
        let detail = state.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.last_turn.unwrap().state, TurnState::Cancelled);
        assert_eq!(
            state.repository().load(&thread.id).await.unwrap().len(),
            event_count
        );
    }
}
