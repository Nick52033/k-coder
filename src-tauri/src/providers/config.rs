use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::protocol::PROTOCOL_VERSION;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    OpenAiCompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub schema_version: u32,
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
}

impl ProviderConfig {
    pub fn validate(mut self) -> Result<Self, ProviderConfigError> {
        if self.schema_version != PROTOCOL_VERSION {
            return Err(ProviderConfigError::Invalid(format!(
                "unsupported schema version {}",
                self.schema_version
            )));
        }
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        self.model = self.model.trim().to_string();

        if self.model.is_empty() || self.model.len() > 200 {
            return Err(ProviderConfigError::Invalid(
                "model must contain between 1 and 200 characters".to_string(),
            ));
        }

        let url = Url::parse(&self.base_url)
            .map_err(|_| ProviderConfigError::Invalid("base URL is invalid".to_string()))?;
        if url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ProviderConfigError::Invalid(
                "base URL must not contain credentials, a query, or a fragment".to_string(),
            ));
        }

        let is_loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback) {
            return Err(ProviderConfigError::Invalid(
                "base URL must use HTTPS; HTTP is only allowed for loopback hosts".to_string(),
            ));
        }

        Ok(self)
    }

    pub fn chat_completions_url(&self) -> Result<Url, ProviderConfigError> {
        Url::parse(&format!("{}/chat/completions", self.base_url)).map_err(|_| {
            ProviderConfigError::Invalid("chat completions URL is invalid".to_string())
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderConfigRequest {
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl SaveProviderConfigRequest {
    pub fn public_config(&self) -> Result<ProviderConfig, ProviderConfigError> {
        ProviderConfig {
            schema_version: PROTOCOL_VERSION,
            kind: self.kind,
            base_url: self.base_url.clone(),
            model: self.model.clone(),
        }
        .validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigView {
    pub schema_version: u32,
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub has_api_key: bool,
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
    path: PathBuf,
}

impl ProviderConfigStore {
    pub fn new(data_root: impl AsRef<Path>) -> Self {
        Self {
            path: data_root.as_ref().join("provider.json"),
        }
    }

    pub fn load(&self) -> Result<Option<ProviderConfig>, ProviderConfigError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(ProviderConfigError::Io(error.to_string())),
        };
        let config: ProviderConfig = serde_json::from_slice(&bytes)
            .map_err(|error| ProviderConfigError::Invalid(error.to_string()))?;
        config.validate().map(Some)
    }

    pub fn save(&self, config: &ProviderConfig) -> Result<(), ProviderConfigError> {
        let parent = self.path.parent().ok_or_else(|| {
            ProviderConfigError::Io("configuration path has no parent".to_string())
        })?;
        fs::create_dir_all(parent).map_err(|error| ProviderConfigError::Io(error.to_string()))?;

        let temp_path = self.path.with_extension("json.tmp");
        let serialized = serde_json::to_vec_pretty(config)
            .map_err(|error| ProviderConfigError::Invalid(error.to_string()))?;
        let mut file = fs::File::create(&temp_path)
            .map_err(|error| ProviderConfigError::Io(error.to_string()))?;
        file.write_all(&serialized)
            .and_then(|_| file.sync_all())
            .map_err(|error| ProviderConfigError::Io(error.to_string()))?;
        replace_file(&temp_path, &self.path)?;
        Ok(())
    }
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
            kind: ProviderKind::OpenAiCompatible,
            base_url: base_url.to_string(),
            model: " test-model ".to_string(),
        }
    }

    #[test]
    fn validates_and_normalizes_provider_configuration() {
        let validated = config("https://example.com/v1/")
            .validate()
            .expect("configuration should be valid");

        assert_eq!(validated.schema_version, PROTOCOL_VERSION);
        assert_eq!(validated.base_url, "https://example.com/v1");
        assert_eq!(validated.model, "test-model");
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
        let raw = fs::read_to_string(directory.path().join("provider.json"))
            .expect("configuration file should be readable");

        assert_eq!(loaded, config);
        assert!(!raw.to_ascii_lowercase().contains("api_key"));
        assert!(!raw.to_ascii_lowercase().contains("apikey"));
    }

    #[test]
    fn rejects_an_unknown_configuration_schema() {
        let mut unknown = config("https://example.com/v1");
        unknown.schema_version = PROTOCOL_VERSION + 1;

        assert!(unknown.validate().is_err());
    }
}
