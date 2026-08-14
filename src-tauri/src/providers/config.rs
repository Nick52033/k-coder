use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

use crate::protocol::PROTOCOL_VERSION;

pub const DEFAULT_MODEL_CONTEXT_WINDOW: u32 = 128_000;
const MIN_MODEL_CONTEXT_WINDOW: u32 = 1_024;
const MAX_MODEL_CONTEXT_WINDOW: u32 = 10_000_000;
const LEGACY_PROVIDER_CATALOG_SCHEMA_VERSION: u32 = 1;
const PROVIDER_CATALOG_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelConfig {
    pub id: String,
    pub display_name: String,
    pub context_window: u32,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub fallback: bool,
}

impl ProviderModelConfig {
    fn legacy(id: String) -> Self {
        Self {
            display_name: id.clone(),
            id,
            context_window: DEFAULT_MODEL_CONTEXT_WINDOW,
            max_output_tokens: None,
            supports_vision: false,
            fallback: false,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ProviderModelConfigCompat {
    Legacy(String),
    Structured {
        id: String,
        #[serde(rename = "displayName")]
        display_name: String,
        #[serde(rename = "contextWindow")]
        context_window: u32,
        #[serde(default, rename = "maxOutputTokens")]
        max_output_tokens: Option<u32>,
        #[serde(default, rename = "supportsVision")]
        supports_vision: bool,
        #[serde(default)]
        fallback: bool,
    },
}

fn deserialize_models<'de, D>(deserializer: D) -> Result<Vec<ProviderModelConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let models = Vec::<ProviderModelConfigCompat>::deserialize(deserializer)?;
    Ok(models
        .into_iter()
        .map(|model| match model {
            ProviderModelConfigCompat::Legacy(id) => ProviderModelConfig::legacy(id),
            ProviderModelConfigCompat::Structured {
                id,
                display_name,
                context_window,
                max_output_tokens,
                supports_vision,
                fallback,
            } => ProviderModelConfig {
                id,
                display_name,
                context_window,
                max_output_tokens,
                supports_vision,
                fallback,
            },
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEndpointConfig {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    OpenAiCompatible,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderTransport {
    #[default]
    OpenAiChatCompletions,
    OpenAiResponses,
    AnthropicMessages,
    GoogleGemini,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub schema_version: u32,
    #[serde(default = "default_provider_id")]
    pub id: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub transport: ProviderTransport,
    #[serde(default = "default_provider_name")]
    pub name: String,
    pub base_url: String,
    pub model: String,
    #[serde(default, deserialize_with = "deserialize_models")]
    pub models: Vec<ProviderModelConfig>,
    #[serde(default)]
    pub endpoints: Vec<ProviderEndpointConfig>,
}

impl ProviderConfig {
    pub fn validate(mut self) -> Result<Self, ProviderConfigError> {
        if self.schema_version != PROTOCOL_VERSION {
            return Err(ProviderConfigError::Invalid(format!(
                "unsupported schema version {}",
                self.schema_version
            )));
        }
        self.id = self.id.trim().to_string();
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        self.name = self.name.trim().to_string();
        self.model = self.model.trim().to_string();

        if self.id.is_empty()
            || self.id.len() > 80
            || !self.id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
        {
            return Err(ProviderConfigError::Invalid(
                "provider ID must contain 1 to 80 ASCII letters, digits, dots, hyphens, or underscores"
                    .to_string(),
            ));
        }
        if self.name.is_empty() || self.name.len() > 80 {
            return Err(ProviderConfigError::Invalid(
                "provider name must contain between 1 and 80 characters".to_string(),
            ));
        }
        if self.model.is_empty() || self.model.len() > 200 {
            return Err(ProviderConfigError::Invalid(
                "model must contain between 1 and 200 characters".to_string(),
            ));
        }

        let mut models = Vec::new();
        for mut configured_model in self.models {
            configured_model.id = configured_model.id.trim().to_string();
            configured_model.display_name = configured_model.display_name.trim().to_string();
            if configured_model.id.is_empty() {
                continue;
            }
            if configured_model.id.len() > 200 {
                return Err(ProviderConfigError::Invalid(
                    "each model ID must contain between 1 and 200 characters".to_string(),
                ));
            }
            if configured_model.display_name.is_empty() || configured_model.display_name.len() > 120
            {
                return Err(ProviderConfigError::Invalid(
                    "each model display name must contain between 1 and 120 characters".to_string(),
                ));
            }
            if !(MIN_MODEL_CONTEXT_WINDOW..=MAX_MODEL_CONTEXT_WINDOW)
                .contains(&configured_model.context_window)
            {
                return Err(ProviderConfigError::Invalid(format!(
                    "each model context window must contain between {MIN_MODEL_CONTEXT_WINDOW} and {MAX_MODEL_CONTEXT_WINDOW} tokens"
                )));
            }
            if !models
                .iter()
                .any(|model: &ProviderModelConfig| model.id == configured_model.id)
            {
                models.push(configured_model);
            }
        }
        if !models.iter().any(|model| model.id == self.model) {
            models.insert(0, ProviderModelConfig::legacy(self.model.clone()));
        }
        if models.len() > 64 {
            return Err(ProviderConfigError::Invalid(
                "a provider may contain at most 64 models".to_string(),
            ));
        }
        self.models = models;

        validate_base_url(&self.base_url, "base URL")?;
        let mut endpoint_ids = std::collections::HashSet::new();
        for endpoint in &mut self.endpoints {
            endpoint.id = endpoint.id.trim().to_string();
            endpoint.name = endpoint.name.trim().to_string();
            endpoint.base_url = endpoint.base_url.trim().trim_end_matches('/').to_string();
            if endpoint.id.is_empty()
                || endpoint.id.len() > 80
                || !endpoint_ids.insert(endpoint.id.clone())
            {
                return Err(ProviderConfigError::Invalid(
                    "endpoint IDs must be unique and contain 1 to 80 characters".into(),
                ));
            }
            if endpoint.name.is_empty() || endpoint.name.len() > 80 {
                return Err(ProviderConfigError::Invalid(
                    "endpoint names must contain 1 to 80 characters".into(),
                ));
            }
            validate_base_url(&endpoint.base_url, "endpoint base URL")?;
        }
        if self.endpoints.len() > 8 {
            return Err(ProviderConfigError::Invalid(
                "a provider may contain at most 8 alternate endpoints".into(),
            ));
        }

        Ok(self)
    }

    pub fn fallback_models(&self) -> impl Iterator<Item = &ProviderModelConfig> {
        self.models
            .iter()
            .filter(|model| model.fallback && model.id != self.model)
    }

    pub fn enabled_endpoints(&self) -> impl Iterator<Item = &ProviderEndpointConfig> {
        self.endpoints.iter().filter(|endpoint| endpoint.enabled)
    }

    pub fn active_model(&self) -> &ProviderModelConfig {
        self.models
            .iter()
            .find(|model| model.id == self.model)
            .expect("validated provider config must contain its active model")
    }

    pub fn chat_completions_url(&self) -> Result<Url, ProviderConfigError> {
        self.endpoint_url("chat/completions", "chat completions")
    }

    pub fn responses_url(&self) -> Result<Url, ProviderConfigError> {
        self.endpoint_url("responses", "responses")
    }

    pub fn anthropic_messages_url(&self) -> Result<Url, ProviderConfigError> {
        // Anthropic Messages API 端点路径
        // 如果 base_url 已经包含 /v1，则只添加 /messages
        // 否则添加 /v1/messages
        let path = if self.base_url.ends_with("/v1") || self.base_url.contains("/v1/") {
            "messages"
        } else {
            "v1/messages"
        };
        self.endpoint_url(path, "Anthropic messages")
    }

    pub fn gemini_stream_url(&self) -> Result<Url, ProviderConfigError> {
        self.gemini_content_url("streamGenerateContent", true)
    }

    pub fn gemini_generate_url(&self) -> Result<Url, ProviderConfigError> {
        self.gemini_content_url("generateContent", false)
    }

    fn gemini_content_url(&self, operation: &str, sse: bool) -> Result<Url, ProviderConfigError> {
        let mut url = Url::parse(&self.base_url)
            .map_err(|_| ProviderConfigError::Invalid("Gemini URL is invalid".to_string()))?;
        let model = self.model.strip_prefix("models/").unwrap_or(&self.model);
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                ProviderConfigError::Invalid("Gemini base URL cannot be a base".to_string())
            })?;
            segments.pop_if_empty();
            segments.push("models");
            segments.push(&format!("{model}:{operation}"));
        }
        if sse {
            url.query_pairs_mut().append_pair("alt", "sse");
        }
        Ok(url)
    }

    fn endpoint_url(&self, path: &str, name: &str) -> Result<Url, ProviderConfigError> {
        Url::parse(&format!("{}/{path}", self.base_url))
            .map_err(|_| ProviderConfigError::Invalid(format!("{name} URL is invalid")))
    }
}

fn validate_base_url(value: &str, label: &str) -> Result<(), ProviderConfigError> {
    let url = Url::parse(value)
        .map_err(|_| ProviderConfigError::Invalid(format!("{label} is invalid")))?;
    if url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ProviderConfigError::Invalid(format!(
            "{label} must not contain credentials, a query, or a fragment"
        )));
    }

    let is_loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback) {
        return Err(ProviderConfigError::Invalid(format!(
            "{label} must use HTTPS; HTTP is only allowed for loopback hosts"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderConfigRequest {
    #[serde(default = "default_provider_id")]
    pub id: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub transport: ProviderTransport,
    #[serde(default = "default_provider_name")]
    pub name: String,
    pub base_url: String,
    pub model: String,
    #[serde(default, deserialize_with = "deserialize_models")]
    pub models: Vec<ProviderModelConfig>,
    #[serde(default)]
    pub endpoints: Vec<ProviderEndpointConfig>,
    pub api_key: Option<String>,
    #[serde(default = "default_true")]
    pub activate: bool,
}

impl SaveProviderConfigRequest {
    pub fn public_config(&self) -> Result<ProviderConfig, ProviderConfigError> {
        ProviderConfig {
            schema_version: PROTOCOL_VERSION,
            id: self.id.clone(),
            kind: self.kind,
            transport: self.transport,
            name: self.name.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            models: self.models.clone(),
            endpoints: self.endpoints.clone(),
        }
        .validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigView {
    pub schema_version: u32,
    pub id: String,
    pub kind: ProviderKind,
    pub transport: ProviderTransport,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub models: Vec<ProviderModelConfig>,
    pub endpoints: Vec<ProviderEndpointConfig>,
    pub has_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalogView {
    pub schema_version: u32,
    pub active_provider_id: Option<String>,
    pub providers: Vec<ProviderConfigView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProviderCatalog {
    schema_version: u32,
    active_provider_id: Option<String>,
    providers: Vec<ProviderConfig>,
}

impl ProviderCatalog {
    fn empty() -> Self {
        Self {
            schema_version: PROVIDER_CATALOG_SCHEMA_VERSION,
            active_provider_id: None,
            providers: Vec::new(),
        }
    }

    fn validate(mut self) -> Result<Self, ProviderConfigError> {
        let migrate_legacy_vision_support = match self.schema_version {
            LEGACY_PROVIDER_CATALOG_SCHEMA_VERSION => true,
            PROVIDER_CATALOG_SCHEMA_VERSION => false,
            _ => {
                return Err(ProviderConfigError::Invalid(format!(
                    "unsupported provider catalog schema version {}",
                    self.schema_version
                )));
            }
        };
        if self.providers.len() > 32 {
            return Err(ProviderConfigError::Invalid(
                "a provider catalog may contain at most 32 providers".to_string(),
            ));
        }
        let mut ids = std::collections::HashSet::new();
        let mut providers = Vec::with_capacity(self.providers.len());
        for provider in self.providers {
            let mut provider = provider.validate()?;
            if migrate_legacy_vision_support {
                for model in &mut provider.models {
                    model.supports_vision |= legacy_model_supports_vision(&model.id);
                }
            }
            if !ids.insert(provider.id.clone()) {
                return Err(ProviderConfigError::Invalid(
                    "provider IDs must be unique".to_string(),
                ));
            }
            providers.push(provider);
        }
        self.schema_version = PROVIDER_CATALOG_SCHEMA_VERSION;
        self.providers = providers;
        if !self
            .active_provider_id
            .as_ref()
            .is_some_and(|id| self.providers.iter().any(|provider| &provider.id == id))
        {
            self.active_provider_id = None;
        }
        Ok(self)
    }

    fn active(&self) -> Option<&ProviderConfig> {
        let active_id = self.active_provider_id.as_ref()?;
        self.providers
            .iter()
            .find(|provider| &provider.id == active_id)
    }
}

fn default_provider_id() -> String {
    "default".to_string()
}

fn default_provider_name() -> String {
    "自定义供应商".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderConfigError {
    #[error("provider configuration is invalid: {0}")]
    Invalid(String),
    #[error("provider configuration I/O failed: {0}")]
    Io(String),
}

#[derive(Debug, Clone)]
pub struct ProviderConfigStore {
    catalog_path: PathBuf,
    legacy_path: PathBuf,
}

impl ProviderConfigStore {
    pub fn new(data_root: impl AsRef<Path>) -> Self {
        Self {
            catalog_path: data_root.as_ref().join("providers.json"),
            legacy_path: data_root.as_ref().join("provider.json"),
        }
    }

    pub fn load(&self) -> Result<Option<ProviderConfig>, ProviderConfigError> {
        Ok(self.load_catalog()?.active().cloned())
    }

    pub fn save(&self, config: &ProviderConfig) -> Result<(), ProviderConfigError> {
        self.save_provider(config, true)
    }

    pub fn list(&self) -> Result<(Option<String>, Vec<ProviderConfig>), ProviderConfigError> {
        let catalog = self.load_catalog()?;
        Ok((catalog.active_provider_id, catalog.providers))
    }

    pub fn save_provider(
        &self,
        config: &ProviderConfig,
        activate: bool,
    ) -> Result<(), ProviderConfigError> {
        let config = config.clone().validate()?;
        let mut catalog = self.load_catalog()?;
        if let Some(existing) = catalog
            .providers
            .iter_mut()
            .find(|provider| provider.id == config.id)
        {
            *existing = config.clone();
        } else {
            catalog.providers.push(config.clone());
        }
        if activate {
            catalog.active_provider_id = Some(config.id.clone());
        }
        self.save_catalog(&catalog.validate()?)
    }

    pub fn activate(&self, provider_id: &str) -> Result<ProviderConfig, ProviderConfigError> {
        let mut catalog = self.load_catalog()?;
        let config = catalog
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .cloned()
            .ok_or_else(|| ProviderConfigError::Invalid("provider does not exist".to_string()))?;
        catalog.active_provider_id = Some(config.id.clone());
        self.save_catalog(&catalog)?;
        Ok(config)
    }

    pub fn delete(&self, provider_id: &str) -> Result<(), ProviderConfigError> {
        let mut catalog = self.load_catalog()?;
        let previous_len = catalog.providers.len();
        catalog
            .providers
            .retain(|provider| provider.id != provider_id);
        if catalog.providers.len() == previous_len {
            return Err(ProviderConfigError::Invalid(
                "provider does not exist".to_string(),
            ));
        }
        if catalog.active_provider_id.as_deref() == Some(provider_id) {
            catalog.active_provider_id = None;
        }
        self.save_catalog(&catalog.validate()?)
    }

    fn load_catalog(&self) -> Result<ProviderCatalog, ProviderConfigError> {
        match fs::read(&self.catalog_path) {
            Ok(bytes) => {
                let catalog: ProviderCatalog = serde_json::from_slice(&bytes)
                    .map_err(|error| ProviderConfigError::Invalid(error.to_string()))?;
                catalog.validate()
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.load_legacy_catalog()
            }
            Err(error) => Err(ProviderConfigError::Io(error.to_string())),
        }
    }

    fn load_legacy_catalog(&self) -> Result<ProviderCatalog, ProviderConfigError> {
        let bytes = match fs::read(&self.legacy_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ProviderCatalog::empty());
            }
            Err(error) => return Err(ProviderConfigError::Io(error.to_string())),
        };
        let config: ProviderConfig = serde_json::from_slice(&bytes)
            .map_err(|error| ProviderConfigError::Invalid(error.to_string()))?;
        let config = config.validate()?;
        ProviderCatalog {
            schema_version: LEGACY_PROVIDER_CATALOG_SCHEMA_VERSION,
            active_provider_id: Some(config.id.clone()),
            providers: vec![config],
        }
        .validate()
    }

    fn save_catalog(&self, catalog: &ProviderCatalog) -> Result<(), ProviderConfigError> {
        let parent = self.catalog_path.parent().ok_or_else(|| {
            ProviderConfigError::Io("configuration path has no parent".to_string())
        })?;
        fs::create_dir_all(parent).map_err(|error| ProviderConfigError::Io(error.to_string()))?;

        let temp_path = self.catalog_path.with_extension("json.tmp");
        let serialized = serde_json::to_vec_pretty(catalog)
            .map_err(|error| ProviderConfigError::Invalid(error.to_string()))?;
        let mut file = fs::File::create(&temp_path)
            .map_err(|error| ProviderConfigError::Io(error.to_string()))?;
        file.write_all(&serialized)
            .and_then(|_| file.sync_all())
            .map_err(|error| ProviderConfigError::Io(error.to_string()))?;
        replace_file(&temp_path, &self.catalog_path)?;
        Ok(())
    }
}

fn legacy_model_supports_vision(model_id: &str) -> bool {
    let model_id = model_id.trim().to_ascii_lowercase();
    ["gpt-4o", "gpt-4.1", "gpt-5"].iter().any(|family| {
        model_id == *family
            || model_id
                .strip_prefix(family)
                .is_some_and(|suffix| matches!(suffix.as_bytes().first(), Some(b'-' | b'.')))
    })
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), ProviderConfigError> {
    // std::fs::rename cannot replace an existing file on Windows.
    #[cfg(target_os = "windows")]
    if destination.exists() {
        fs::remove_file(destination).map_err(|error| ProviderConfigError::Io(error.to_string()))?;
    }
    fs::rename(source, destination).map_err(|error| ProviderConfigError::Io(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(base_url: &str) -> ProviderConfig {
        ProviderConfig {
            schema_version: PROTOCOL_VERSION,
            id: default_provider_id(),
            kind: ProviderKind::OpenAiCompatible,
            transport: ProviderTransport::OpenAiChatCompletions,
            name: default_provider_name(),
            base_url: base_url.to_string(),
            model: " test-model ".to_string(),
            models: Vec::new(),
            endpoints: Vec::new(),
        }
    }

    #[test]
    fn validates_and_normalizes_provider_configuration() {
        let validated = config("https://example.com/v1/")
            .validate()
            .expect("configuration should be valid");

        assert_eq!(validated.schema_version, PROTOCOL_VERSION);
        assert_eq!(validated.name, "自定义供应商");
        assert_eq!(validated.base_url, "https://example.com/v1");
        assert_eq!(validated.model, "test-model");
        assert_eq!(
            validated.models,
            vec![ProviderModelConfig::legacy("test-model".to_string())]
        );
        assert_eq!(
            validated
                .chat_completions_url()
                .expect("endpoint should build")
                .as_str(),
            "https://example.com/v1/chat/completions"
        );
    }

    #[test]
    fn rejects_insecure_remote_and_credentialed_urls() {
        assert!(config("http://example.com/v1").validate().is_err());
        assert!(
            config("https://user:secret@example.com/v1")
                .validate()
                .is_err()
        );
        assert!(config("http://localhost:8080/v1").validate().is_ok());
    }

    #[test]
    fn persists_only_public_provider_configuration() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let store = ProviderConfigStore::new(directory.path());
        let config = config("https://example.com/v1")
            .validate()
            .expect("configuration should be valid");

        store.save(&config).expect("configuration should save");
        let loaded = store
            .load()
            .expect("configuration should load")
            .expect("configuration should exist");
        let raw = fs::read_to_string(directory.path().join("providers.json"))
            .expect("configuration file should be readable");

        assert_eq!(loaded, config);
        assert!(raw.contains("\"displayName\""));
        assert!(raw.contains("\"contextWindow\""));
        assert!(!raw.to_ascii_lowercase().contains("api_key"));
        assert!(!raw.to_ascii_lowercase().contains("apikey"));
    }

    #[test]
    fn migrates_a_legacy_single_provider_and_switches_catalog_entries() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let store = ProviderConfigStore::new(directory.path());
        let first = config("https://first.example.com/v1")
            .validate()
            .expect("first provider should validate");
        fs::write(
            directory.path().join("provider.json"),
            serde_json::to_vec(&first).unwrap(),
        )
        .unwrap();

        assert_eq!(store.load().unwrap(), Some(first.clone()));
        let mut second = config("https://second.example.com/v1");
        second.id = "second".to_string();
        second.name = "Second".to_string();
        let second = second.validate().unwrap();
        store.save_provider(&second, false).unwrap();
        let (active_id, providers) = store.list().unwrap();
        assert_eq!(active_id.as_deref(), Some("default"));
        assert_eq!(providers.len(), 2);

        store.activate("second").unwrap();
        assert_eq!(store.load().unwrap(), Some(second));
    }

    #[test]
    fn migrates_known_multimodal_models_in_legacy_catalogs() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let store = ProviderConfigStore::new(directory.path());
        let mut provider = config("https://example.com/v1");
        provider.model = "gpt-5.6-sol".into();
        provider.models = vec![
            ProviderModelConfig {
                id: "gpt-5.6-sol".into(),
                display_name: "GPT-5.6 Sol".into(),
                context_window: 200_000,
                max_output_tokens: None,
                supports_vision: false,
                fallback: false,
            },
            ProviderModelConfig {
                id: "deepseek-text".into(),
                display_name: "DeepSeek Text".into(),
                context_window: 128_000,
                max_output_tokens: None,
                supports_vision: false,
                fallback: false,
            },
            ProviderModelConfig {
                id: "o3-mini".into(),
                display_name: "O3 Mini".into(),
                context_window: 128_000,
                max_output_tokens: None,
                supports_vision: false,
                fallback: false,
            },
        ];
        let provider = provider.validate().expect("provider should validate");
        let legacy = ProviderCatalog {
            schema_version: LEGACY_PROVIDER_CATALOG_SCHEMA_VERSION,
            active_provider_id: Some(provider.id.clone()),
            providers: vec![provider],
        };
        fs::write(
            directory.path().join("providers.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();

        let mut loaded = store.load().unwrap().expect("active provider should load");

        assert!(loaded.models[0].supports_vision);
        assert!(!loaded.models[1].supports_vision);
        assert!(!loaded.models[2].supports_vision);
        store.save_provider(&loaded, true).unwrap();
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.path().join("providers.json")).unwrap())
                .unwrap();
        assert_eq!(persisted["schemaVersion"], PROVIDER_CATALOG_SCHEMA_VERSION);

        loaded.models[0].supports_vision = false;
        store.save_provider(&loaded, true).unwrap();
        let reloaded = store
            .load()
            .unwrap()
            .expect("active provider should reload");
        assert!(!reloaded.models[0].supports_vision);
    }

    #[test]
    fn rejects_an_unknown_configuration_schema() {
        let mut unknown = config("https://example.com/v1");
        unknown.schema_version = PROTOCOL_VERSION + 1;

        assert!(unknown.validate().is_err());
    }

    #[test]
    fn rejects_provider_ids_that_are_unsafe_for_credential_accounts() {
        let mut invalid = config("https://example.com/v1");
        invalid.id = "../shared credential".to_string();

        assert!(invalid.validate().is_err());
    }

    #[test]
    fn old_configuration_defaults_to_chat_completions() {
        let config: ProviderConfig = serde_json::from_str::<ProviderConfig>(
            r#"{"schemaVersion":1,"kind":"open_ai_compatible","baseUrl":"https://example.com/v1","model":"test"}"#,
        )
        .expect("legacy configuration should deserialize")
        .validate()
        .expect("legacy configuration should validate");

        assert_eq!(config.transport, ProviderTransport::OpenAiChatCompletions);
        assert_eq!(config.name, "自定义供应商");
        assert_eq!(
            config.models,
            vec![ProviderModelConfig::legacy("test".to_string())]
        );
    }

    #[test]
    fn normalizes_provider_models_and_preserves_the_active_model() {
        let mut config = config("https://example.com/v1");
        config.name = " OpenAI 团队 ".to_string();
        config.model = "gpt-4.1".to_string();
        config.models = vec![
            ProviderModelConfig {
                id: " gpt-4o ".to_string(),
                display_name: " GPT-4o ".to_string(),
                context_window: 128_000,
                max_output_tokens: None,
                supports_vision: false,
                fallback: false,
            },
            ProviderModelConfig {
                id: "gpt-4o".to_string(),
                display_name: "duplicate".to_string(),
                context_window: 64_000,
                max_output_tokens: None,
                supports_vision: false,
                fallback: false,
            },
            ProviderModelConfig::legacy("".to_string()),
        ];

        let validated = config.validate().expect("models should normalize");

        assert_eq!(validated.name, "OpenAI 团队");
        assert_eq!(validated.models[0].id, "gpt-4.1");
        assert_eq!(validated.models[0].display_name, "gpt-4.1");
        assert_eq!(
            validated.models[0].context_window,
            DEFAULT_MODEL_CONTEXT_WINDOW
        );
        assert_eq!(validated.models[1].id, "gpt-4o");
        assert_eq!(validated.models[1].display_name, "GPT-4o");
    }

    #[test]
    fn migrates_legacy_string_model_lists() {
        let config: ProviderConfig = serde_json::from_str::<ProviderConfig>(
            r#"{"schemaVersion":1,"kind":"open_ai_compatible","baseUrl":"https://example.com/v1","model":"gpt-4.1","models":["gpt-4.1","gpt-4o"]}"#,
        )
        .expect("legacy model list should deserialize")
        .validate()
        .expect("legacy model list should validate");

        assert_eq!(config.models.len(), 2);
        assert_eq!(config.models[1].id, "gpt-4o");
        assert_eq!(config.models[1].display_name, "gpt-4o");
        assert_eq!(
            config.models[1].context_window,
            DEFAULT_MODEL_CONTEXT_WINDOW
        );
    }

    #[test]
    fn rejects_invalid_structured_model_metadata() {
        let mut invalid = config("https://example.com/v1");
        invalid.models = vec![ProviderModelConfig {
            id: "test-model".to_string(),
            display_name: "Test model".to_string(),
            context_window: 512,
            max_output_tokens: None,
            supports_vision: false,
            fallback: false,
        }];

        assert!(invalid.validate().is_err());
    }

    #[test]
    fn validates_alternate_endpoints_and_fallback_models() {
        let mut configured = config("https://example.com/v1");
        configured.model = "primary".into();
        configured.models = vec![
            ProviderModelConfig::legacy("primary".into()),
            ProviderModelConfig {
                id: "fallback".into(),
                display_name: "Fallback".into(),
                context_window: 64_000,
                max_output_tokens: None,
                supports_vision: false,
                fallback: true,
            },
        ];
        configured.endpoints = vec![ProviderEndpointConfig {
            id: "secondary".into(),
            name: "Secondary".into(),
            base_url: "https://secondary.example.com/v1/".into(),
            enabled: true,
        }];
        let validated = configured.validate().unwrap();
        assert_eq!(validated.fallback_models().count(), 1);
        assert_eq!(validated.enabled_endpoints().count(), 1);
        assert_eq!(
            validated.endpoints[0].base_url,
            "https://secondary.example.com/v1"
        );

        let mut invalid = config("https://example.com/v1");
        invalid.endpoints = vec![ProviderEndpointConfig {
            id: "secondary".into(),
            name: "Secondary".into(),
            base_url: "http://secondary.example.com/v1".into(),
            enabled: true,
        }];
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn builds_transport_specific_endpoints() {
        let config = config("https://example.com/v1")
            .validate()
            .expect("configuration should validate");

        assert_eq!(
            config.responses_url().unwrap().as_str(),
            "https://example.com/v1/responses"
        );
        assert_eq!(
            config.anthropic_messages_url().unwrap().as_str(),
            "https://example.com/v1/messages"
        );
        assert_eq!(
            config.gemini_stream_url().unwrap().as_str(),
            "https://example.com/v1/models/test-model:streamGenerateContent?alt=sse"
        );
        assert_eq!(
            config.gemini_generate_url().unwrap().as_str(),
            "https://example.com/v1/models/test-model:generateContent"
        );
    }

    #[test]
    fn builds_anthropic_messages_url_with_and_without_v1() {
        // baseUrl 包含 /v1 的情况
        let config_with_v1 = config("https://api.zicc.cc/v1")
            .validate()
            .expect("configuration should validate");
        assert_eq!(
            config_with_v1.anthropic_messages_url().unwrap().as_str(),
            "https://api.zicc.cc/v1/messages"
        );

        // baseUrl 不包含 /v1 的情况
        let config_without_v1 = config("https://api.zicc.cc")
            .validate()
            .expect("configuration should validate");
        assert_eq!(
            config_without_v1.anthropic_messages_url().unwrap().as_str(),
            "https://api.zicc.cc/v1/messages"
        );
    }
}
