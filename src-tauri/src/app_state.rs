use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::execution::{CommandRuntime, ExecutionError, NativePtyRuntime};
use crate::logging::StructuredLogger;
use crate::patch::{PatchError, PatchService};
use crate::policy::ApprovalManager;
use crate::protocol::{ApprovalAction, ApprovalResolution, ChangeSet, TurnState};
use crate::providers::{
    AnthropicMessagesProvider, CredentialError, CredentialStore, GoogleGeminiProvider,
    OpenAiChatCompletionsProvider, OpenAiResponsesProvider, OsCredentialStore, Provider,
    ProviderConfigError, ProviderConfigStore, ProviderConfigView, ProviderTransport,
    SaveProviderConfigRequest,
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
    workspace_root: PathBuf,
    tool_registry: ToolRegistry,
    patch_service: PatchService,
    approvals: Arc<ApprovalManager>,
    command_runtime: CommandRuntime,
    pty_runtime: NativePtyRuntime,
    logger: StructuredLogger,
    active_turns: Mutex<HashMap<String, CancellationToken>>,
    recovery_lock: Mutex<()>,
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
        let workspace_root =
            std::env::current_dir().map_err(|error| AppStateError::Workspace(error.to_string()))?;
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
        Ok(Self {
            started_at: Instant::now(),
            repository: Arc::new(JsonlThreadRepository::new(&data_root)?),
            provider_config: ProviderConfigStore::new(&data_root),
            credentials,
            workspace_root,
            tool_registry: ToolRegistry::workspace_tools_with_execution(
                patch_service.clone(),
                command_runtime.clone(),
            ),
            patch_service,
            approvals: Arc::new(ApprovalManager::new(Duration::from_secs(5 * 60))),
            command_runtime,
            pty_runtime,
            logger: StructuredLogger::new(&data_root)
                .map_err(|error| AppStateError::Logging(error.to_string()))?,
            active_turns: Mutex::new(HashMap::new()),
            recovery_lock: Mutex::new(()),
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
        self.workspace_root.clone()
    }

    pub fn tool_registry(&self) -> ToolRegistry {
        self.tool_registry.clone()
    }

    pub fn patch_service(&self) -> PatchService {
        self.patch_service.clone()
    }

    pub fn approvals(&self) -> Arc<ApprovalManager> {
        self.approvals.clone()
    }

    pub fn command_runtime(&self) -> CommandRuntime {
        self.command_runtime.clone()
    }

    pub fn pty_runtime(&self) -> NativePtyRuntime {
        self.pty_runtime.clone()
    }

    pub fn logger(&self) -> StructuredLogger {
        self.logger.clone()
    }

    pub fn provider_config(&self) -> Result<Option<ProviderConfigView>, AppStateError> {
        let Some(config) = self.provider_config.load()? else {
            return Ok(None);
        };
        Ok(Some(ProviderConfigView {
            schema_version: config.schema_version,
            kind: config.kind,
            transport: config.transport,
            base_url: config.base_url,
            model: config.model,
            has_api_key: self.credentials.get_api_key()?.is_some(),
        }))
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
        if let Some(api_key) = api_key {
            self.credentials.set_api_key(api_key)?;
        } else if self.credentials.get_api_key()?.is_none() {
            return Err(AppStateError::ProviderNotConfigured(
                "an API key is required".to_string(),
            ));
        }
        self.provider_config.save(&config)?;
        self.repository
            .projection()
            .set_setting(
                "provider",
                &serde_json::to_string(&config)
                    .map_err(|error| AppStateError::ProviderNotConfigured(error.to_string()))?,
            )
            .map_err(|error| AppStateError::Storage(StorageError::Io(error.to_string())))?;
        Ok(ProviderConfigView {
            schema_version: config.schema_version,
            kind: config.kind,
            transport: config.transport,
            base_url: config.base_url,
            model: config.model,
            has_api_key: true,
        })
    }

    pub fn delete_provider_api_key(&self) -> Result<(), AppStateError> {
        self.credentials.delete_api_key()?;
        Ok(())
    }

    pub fn build_provider(&self) -> Result<(Arc<dyn Provider>, String), AppStateError> {
        let config = self.provider_config.load()?.ok_or_else(|| {
            AppStateError::ProviderNotConfigured(
                "configure a provider before starting a turn".to_string(),
            )
        })?;
        let api_key = self.credentials.get_api_key()?.ok_or_else(|| {
            AppStateError::ProviderNotConfigured("the provider API key is missing".to_string())
        })?;
        let model = config.model.clone();
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
        Ok((provider, model))
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

        if let Some(approval) = detail
            .approvals
            .iter()
            .rev()
            .find(|approval| approval.resolution.is_none())
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
            .undo(self.workspace_root.clone(), change_set)
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
                .redo(self.workspace_root.clone(), undone_change.clone())
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
    #[error("provider is not configured: {0}")]
    ProviderNotConfigured(String),
    #[error("a turn is already active for thread {0}")]
    TurnAlreadyActive(String),
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
    use crate::protocol::{ApprovalRequest, ExpectedFileHash, ToolRisk};
    use crate::providers::{ProviderKind, ProviderTransport, SaveProviderConfigRequest};

    #[derive(Default)]
    struct FakeCredentials {
        api_key: StdMutex<Option<String>>,
    }

    impl CredentialStore for FakeCredentials {
        fn get_api_key(&self) -> Result<Option<String>, CredentialError> {
            Ok(self.api_key.lock().unwrap().clone())
        }

        fn set_api_key(&self, api_key: &str) -> Result<(), CredentialError> {
            *self.api_key.lock().unwrap() = Some(api_key.to_string());
            Ok(())
        }

        fn delete_api_key(&self) -> Result<(), CredentialError> {
            *self.api_key.lock().unwrap() = None;
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
                kind: ProviderKind::OpenAiCompatible,
                transport: ProviderTransport::OpenAiChatCompletions,
                base_url: "https://example.com/v1".to_string(),
                model: "test-model".to_string(),
                api_key: Some("super-secret".to_string()),
            })
            .expect("configuration should save");
        let serialized = serde_json::to_string(&view).expect("view should serialize");

        assert!(view.has_api_key);
        assert!(!serialized.contains("super-secret"));
        assert_eq!(
            credentials.get_api_key().unwrap().as_deref(),
            Some("super-secret")
        );
    }

    #[tokio::test]
    async fn allows_only_one_active_turn_per_thread() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let state =
            AppState::with_credentials(directory.path(), Arc::new(FakeCredentials::default()))
                .expect("state should initialize");

        state.begin_turn("thread").await.expect("first turn starts");
        assert!(matches!(
            state.begin_turn("thread").await,
            Err(AppStateError::TurnAlreadyActive(_))
        ));
        assert!(state.cancel_turn("thread").await);
        state.finish_turn("thread").await;
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
    async fn recovers_an_orphaned_approval_as_cancelled_once() {
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
        ] {
            state
                .repository()
                .append(StoredEvent::new(&thread.id, Some(turn_id.clone()), kind))
                .await
                .unwrap();
        }

        let detail = state.read_thread(&thread.id).await.unwrap();
        assert_eq!(detail.last_turn.unwrap().state, TurnState::Cancelled);
        assert_eq!(
            detail.approvals[0]
                .resolution
                .as_ref()
                .map(|resolution| resolution.action),
            Some(ApprovalAction::Cancelled)
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
