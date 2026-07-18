#[cfg(not(test))]
const SERVICE_NAME: &str = "com.kcoder.app";
#[cfg(not(test))]
const API_KEY_ACCOUNT: &str = "default-provider-api-key";

pub trait CredentialStore: Send + Sync {
    fn get_api_key(&self) -> Result<Option<String>, CredentialError>;
    fn set_api_key(&self, api_key: &str) -> Result<(), CredentialError>;
    fn delete_api_key(&self) -> Result<(), CredentialError>;
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
    fn entry(&self) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(SERVICE_NAME, API_KEY_ACCOUNT)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }
}

#[cfg(not(test))]
impl CredentialStore for OsCredentialStore {
    fn get_api_key(&self) -> Result<Option<String>, CredentialError> {
        let _guard = self.access_lock.lock().map_err(|_| {
            CredentialError::Unavailable("credential lock was poisoned".to_string())
        })?;
        match self.entry()?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(CredentialError::Unavailable(error.to_string())),
        }
    }

    fn set_api_key(&self, api_key: &str) -> Result<(), CredentialError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(CredentialError::Empty);
        }
        let _guard = self.access_lock.lock().map_err(|_| {
            CredentialError::Unavailable("credential lock was poisoned".to_string())
        })?;
        self.entry()?
            .set_password(api_key)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }

    fn delete_api_key(&self) -> Result<(), CredentialError> {
        let _guard = self.access_lock.lock().map_err(|_| {
            CredentialError::Unavailable("credential lock was poisoned".to_string())
        })?;
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(CredentialError::Unavailable(error.to_string())),
        }
    }
}

#[cfg(test)]
impl CredentialStore for OsCredentialStore {
    fn get_api_key(&self) -> Result<Option<String>, CredentialError> {
        Err(CredentialError::Unavailable(
            "native credential access is disabled in tests".to_string(),
        ))
    }

    fn set_api_key(&self, _api_key: &str) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable(
            "native credential access is disabled in tests".to_string(),
        ))
    }

    fn delete_api_key(&self) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable(
            "native credential access is disabled in tests".to_string(),
        ))
    }
}
