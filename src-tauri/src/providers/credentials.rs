#[cfg(not(test))]
const SERVICE_NAME: &str = "com.kcoder.app";
#[cfg(not(test))]
const LEGACY_API_KEY_ACCOUNT: &str = "default-provider-api-key";

pub trait CredentialStore: Send + Sync {
    fn get_api_key(&self, provider_id: &str) -> Result<Option<String>, CredentialError>;
    fn set_api_key(&self, provider_id: &str, api_key: &str) -> Result<(), CredentialError>;
    fn delete_api_key(&self, provider_id: &str) -> Result<(), CredentialError>;
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum CredentialError {
    #[error("API key is empty")]
    Empty,
    #[error("operating system credential store failed: {0}")]
    Unavailable(String),
}

#[derive(Debug, Default)]
pub struct OsCredentialStore {
    #[cfg(not(test))]
    access_lock: std::sync::Mutex<()>,
}

impl OsCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(not(test))]
    fn entry(&self, account: &str) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(SERVICE_NAME, account)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }

    #[cfg(not(test))]
    fn account(provider_id: &str) -> String {
        format!("provider-api-key:{provider_id}")
    }

    #[cfg(not(test))]
    fn read_entry(&self, account: &str) -> Result<Option<String>, CredentialError> {
        match self.entry(account)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(CredentialError::Unavailable(error.to_string())),
        }
    }

    #[cfg(not(test))]
    fn delete_entry(&self, account: &str) -> Result<(), CredentialError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(CredentialError::Unavailable(error.to_string())),
        }
    }
}

#[cfg(not(test))]
impl CredentialStore for OsCredentialStore {
    fn get_api_key(&self, provider_id: &str) -> Result<Option<String>, CredentialError> {
        let _guard = self.access_lock.lock().map_err(|_| {
            CredentialError::Unavailable("credential lock was poisoned".to_string())
        })?;
        let credential = self.read_entry(&Self::account(provider_id))?;
        if credential.is_some() || provider_id != "default" {
            return Ok(credential);
        }
        self.read_entry(LEGACY_API_KEY_ACCOUNT)
    }

    fn set_api_key(&self, provider_id: &str, api_key: &str) -> Result<(), CredentialError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(CredentialError::Empty);
        }
        let _guard = self.access_lock.lock().map_err(|_| {
            CredentialError::Unavailable("credential lock was poisoned".to_string())
        })?;
        self.entry(&Self::account(provider_id))?
            .set_password(api_key)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }

    fn delete_api_key(&self, provider_id: &str) -> Result<(), CredentialError> {
        let _guard = self.access_lock.lock().map_err(|_| {
            CredentialError::Unavailable("credential lock was poisoned".to_string())
        })?;
        self.delete_entry(&Self::account(provider_id))?;
        if provider_id == "default" {
            self.delete_entry(LEGACY_API_KEY_ACCOUNT)?;
        }
        Ok(())
    }
}

#[cfg(test)]
impl CredentialStore for OsCredentialStore {
    fn get_api_key(&self, _provider_id: &str) -> Result<Option<String>, CredentialError> {
        Err(CredentialError::Unavailable(
            "native credential access is disabled in tests".to_string(),
        ))
    }

    fn set_api_key(&self, _provider_id: &str, _api_key: &str) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable(
            "native credential access is disabled in tests".to_string(),
        ))
    }

    fn delete_api_key(&self, _provider_id: &str) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable(
            "native credential access is disabled in tests".to_string(),
        ))
    }
}
