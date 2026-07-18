use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::providers::{
    CredentialError, CredentialStore, OpenAiChatCompletionsProvider, OsCredentialStore, Provider,
    ProviderConfigError, ProviderConfigStore, ProviderConfigView, SaveProviderConfigRequest,
};
use crate::storage::{JsonlThreadRepository, StorageError, ThreadRepository};

pub struct AppState {
    started_at: Instant,
    repository: Arc<JsonlThreadRepository>,
    provider_config: ProviderConfigStore,
    credentials: Arc<dyn CredentialStore>,
    active_turns: Mutex<HashMap<String, CancellationToken>>,
}

impl AppState {
    pub fn new(data_root: impl AsRef<Path>) -> Result<Self, AppStateError> {
        Self::with_credentials(data_root, Arc::new(OsCredentialStore::new()))
    }

    pub fn with_credentials(
        data_root: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, AppStateError> {
        Ok(Self {
            started_at: Instant::now(),
            repository: Arc::new(JsonlThreadRepository::new(&data_root)?),
            provider_config: ProviderConfigStore::new(data_root),
            credentials,
            active_turns: Mutex::new(HashMap::new()),
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

    pub fn provider_config(&self) -> Result<Option<ProviderConfigView>, AppStateError> {
        let Some(config) = self.provider_config.load()? else {
            return Ok(None);
        };
        Ok(Some(ProviderConfigView {
            schema_version: config.schema_version,
            kind: config.kind,
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
        Ok(ProviderConfigView {
            schema_version: config.schema_version,
            kind: config.kind,
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
        let provider = OpenAiChatCompletionsProvider::new(config, api_key)
            .map_err(|error| AppStateError::ProviderNotConfigured(error.to_string()))?;
        Ok((Arc::new(provider), model))
    }

    pub async fn begin_turn(&self, thread_id: &str) -> Result<CancellationToken, AppStateError> {
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
}

#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    ProviderConfig(#[from] ProviderConfigError),
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error("provider is not configured: {0}")]
    ProviderNotConfigured(String),
    #[error("a turn is already active for thread {0}")]
    TurnAlreadyActive(String),
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::providers::{ProviderKind, SaveProviderConfigRequest};

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
}
