//! OS keyring abstraction for API keys and sync tokens.
//!
//! Secrets live only in the platform keyring (Linux Secret Service via `dbus-secret-service` with
//! pure-Rust crypto, Windows Credential Manager via `windows-native`) and are never written to
//! `config.json`, the SQLite database, logs, or `Debug` output.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

/// Keyring service name used for all Twitty credentials.
pub const SECRET_SERVICE: &str = "twitty";

/// A secret string that never leaks through [`fmt::Debug`] and zeroes its buffer on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue {
    inner: String,
}

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            inner: value.into(),
        }
    }

    /// Borrow the raw secret solely at the call site that must transmit it (e.g. an API header).
    pub fn expose(&self) -> &str {
        &self.inner
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        // Safe memory scrubbing without `unsafe`: overwrite the string's backing UTF-8 buffer
        // with NUL bytes before freeing.
        let mut raw = std::mem::take(&mut self.inner).into_bytes();
        raw.fill(0);
        std::hint::black_box(&raw);
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue([REDACTED])")
    }
}

/// Errors returned by [`SecretStore`] implementations. Never embeds secret values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretError {
    /// The account identifier was empty or invalid.
    InvalidAccount,
    /// Platform keyring backend returned an error.
    Backend(String),
    /// No system keyring backend is available on this platform or session.
    Unavailable(String),
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAccount => write!(f, "secret account name must not be empty"),
            Self::Backend(msg) => write!(f, "keyring error: {msg}"),
            Self::Unavailable(msg) => write!(f, "keyring unavailable: {msg}"),
        }
    }
}

impl std::error::Error for SecretError {}

/// Abstract credential store keyed by `(service = "twitty", account)`.
pub trait SecretStore: Send + Sync + fmt::Debug {
    /// Retrieve the secret for `account`, returning `Ok(None)` when absent.
    fn get(&self, account: &str) -> Result<Option<SecretValue>, SecretError>;

    /// Store or overwrite `secret` for `account`.
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError>;

    /// Delete the secret for `account`. Returns `Ok(true)` if an entry existed, `Ok(false)` if not.
    fn delete(&self, account: &str) -> Result<bool, SecretError>;
}

fn validate_account(account: &str) -> Result<(), SecretError> {
    if account.trim().is_empty() {
        return Err(SecretError::InvalidAccount);
    }
    Ok(())
}

/// In-memory [`SecretStore`] for unit and integration tests.
///
/// Its [`fmt::Debug`] implementation intentionally omits all stored secret values.
#[derive(Default)]
pub struct InMemorySecretStore {
    service: String,
    entries: Mutex<HashMap<String, SecretValue>>,
}

impl InMemorySecretStore {
    pub fn new() -> Self {
        Self::with_service(SECRET_SERVICE)
    }

    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }
}

impl fmt::Debug for InMemorySecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.entries.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("InMemorySecretStore")
            .field("service", &self.service)
            .field("entries_count", &count)
            .finish()
    }
}

impl SecretStore for InMemorySecretStore {
    fn get(&self, account: &str) -> Result<Option<SecretValue>, SecretError> {
        validate_account(account)?;
        let guard = self
            .entries
            .lock()
            .map_err(|_| SecretError::Backend("secret store lock poisoned".into()))?;
        Ok(guard.get(account).cloned())
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        validate_account(account)?;
        let mut guard = self
            .entries
            .lock()
            .map_err(|_| SecretError::Backend("secret store lock poisoned".into()))?;
        guard.insert(account.to_owned(), SecretValue::new(secret));
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<bool, SecretError> {
        validate_account(account)?;
        let mut guard = self
            .entries
            .lock()
            .map_err(|_| SecretError::Backend("secret store lock poisoned".into()))?;
        Ok(guard.remove(account).is_some())
    }
}

/// Platform keyring backed by Linux Secret Service (D-Bus, synchronous, pure-Rust crypto) or
/// Windows Credential Manager.
pub struct SystemSecretStore {
    service: String,
}

impl Default for SystemSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemSecretStore {
    pub fn new() -> Self {
        Self::with_service(SECRET_SERVICE)
    }

    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }
}

impl fmt::Debug for SystemSecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemSecretStore")
            .field("service", &self.service)
            .finish()
    }
}

#[cfg(any(target_os = "linux", windows))]
mod system_impl {
    use super::{validate_account, SecretError, SecretStore, SecretValue, SystemSecretStore};

    fn map_keyring_err(err: keyring::Error) -> SecretError {
        match err {
            keyring::Error::NoStorageAccess(inner) => SecretError::Unavailable(inner.to_string()),
            other => SecretError::Backend(other.to_string()),
        }
    }

    impl SecretStore for SystemSecretStore {
        fn get(&self, account: &str) -> Result<Option<SecretValue>, SecretError> {
            validate_account(account)?;
            let entry = keyring::Entry::new(&self.service, account).map_err(map_keyring_err)?;
            match entry.get_password() {
                Ok(secret) => Ok(Some(SecretValue::new(secret))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(err) => Err(map_keyring_err(err)),
            }
        }

        fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
            validate_account(account)?;
            let entry = keyring::Entry::new(&self.service, account).map_err(map_keyring_err)?;
            entry.set_password(secret).map_err(map_keyring_err)
        }

        fn delete(&self, account: &str) -> Result<bool, SecretError> {
            validate_account(account)?;
            let entry = keyring::Entry::new(&self.service, account).map_err(map_keyring_err)?;
            match entry.delete_credential() {
                Ok(()) => Ok(true),
                Err(keyring::Error::NoEntry) => Ok(false),
                Err(err) => Err(map_keyring_err(err)),
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
impl SecretStore for SystemSecretStore {
    fn get(&self, account: &str) -> Result<Option<SecretValue>, SecretError> {
        validate_account(account)?;
        Err(SecretError::Unavailable(
            "system keyring is not configured for this platform".into(),
        ))
    }

    fn set(&self, account: &str, _secret: &str) -> Result<(), SecretError> {
        validate_account(account)?;
        Err(SecretError::Unavailable(
            "system keyring is not configured for this platform".into(),
        ))
    }

    fn delete(&self, account: &str) -> Result<bool, SecretError> {
        validate_account(account)?;
        Err(SecretError::Unavailable(
            "system keyring is not configured for this platform".into(),
        ))
    }
}
