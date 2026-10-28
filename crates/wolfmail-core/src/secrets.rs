//! Secrets (passwords, OAuth refresh tokens, the IntelligentWolf device token) live in the OS credential store:
//! Secret Service on Linux, Keychain on macOS, Credential Manager on Windows — via the `keyring` crate (v1 mode).
//! They never touch the SQLite file and are never logged.
use crate::error::{ApiError, ErrorKind, Result};
use parking_lot::Mutex;
use std::collections::HashMap;

/// Source: docs/IW-INTEGRATION.md "Token stored in the OS keychain (service `WolfMail` …)".
pub const SERVICE: &str = "WolfMail";

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

pub struct KeyringStore;

impl KeyringStore {
    fn entry(key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, key).map_err(|e| {
            ApiError::new(
                ErrorKind::Storage,
                format!("The system keychain is not available ({e}). On Linux, install and unlock a Secret Service provider such as GNOME Keyring or KWallet."),
            )
        })
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        match Self::entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(ApiError::storage(format!("Could not read from the system keychain: {e}"))),
        }
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        Self::entry(key)?
            .set_password(value)
            .map_err(|e| ApiError::storage(format!("Could not save to the system keychain: {e}")))
    }
    fn delete(&self, key: &str) -> Result<()> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(ApiError::storage(format!("Could not remove from the system keychain: {e}"))),
        }
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.map.lock().get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.map.lock().insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.map.lock().remove(key);
        Ok(())
    }
}

pub fn password_key(account_id: &str, which: &str) -> String {
    format!("password:{account_id}:{which}")
}
pub fn oauth_key(email: &str) -> String {
    format!("oauth:{}", email.to_ascii_lowercase())
}
pub const IW_DEVICE_KEY: &str = "iw-device";
