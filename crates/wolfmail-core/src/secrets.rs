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

// ── Fallback for desktops with no OS credential store ──────────────────────────────────────────────────────────
// Some Linux setups (a bare tiling WM, a minimal distro, a headless session) run no Secret Service daemon, so the
// keychain is unusable. Rather than leave the user unable to save a password, WolfMail keeps secrets in an
// encrypted file in its data directory, unlocked with a primary password at startup — the same model as
// Thunderbird's logins.json + primary password.
//
// Crypto (read from the crates, not recalled):
//   KDF   Argon2id v0x13 — argon2-0.6.0 src/params.rs:48/58/67 defaults m=19 MiB, t=2, p=1, which are also the
//         OWASP Password Storage Cheat Sheet's first Argon2id configuration. Salt 16 bytes (src/lib.rs:182
//         RECOMMENDED_SALT_LEN). The parameters are stored in the file so they can be raised later.
//   AEAD  XChaCha20-Poly1305 — chacha20poly1305-0.11.0 src/lib.rs "Usage": `XNonce::generate()` gives a random
//         24-byte nonce, large enough that random nonces never realistically repeat. A fresh nonce on every save.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, Generate, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use std::path::PathBuf;
use zeroize::Zeroizing;

const VAULT_VERSION: u32 = 1;
const SALT_LEN: usize = 16;

#[derive(serde::Serialize, serde::Deserialize)]
struct VaultFile {
    v: u32,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
}

struct Unlocked {
    key: Zeroizing<[u8; 32]>,
    salt: [u8; SALT_LEN],
    params: (u32, u32, u32),
    map: HashMap<String, String>,
}

/// Encrypted secrets file. Locked until `create` or `unlock` succeeds.
pub struct FileVault {
    path: PathBuf,
    open: Mutex<Option<Unlocked>>,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn derive_key(password: &str, salt: &[u8], m: u32, t: u32, p: u32) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(m, t, p, Some(32)).map_err(|e| ApiError::storage(format!("Invalid vault parameters: {e}")))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| ApiError::storage(format!("Could not derive the vault key: {e}")))?;
    Ok(key)
}

impl FileVault {
    pub fn new(path: PathBuf) -> Self {
        Self { path, open: Mutex::new(None) }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn is_unlocked(&self) -> bool {
        self.open.lock().is_some()
    }

    /// First run: choose the primary password. Refuses to overwrite an existing vault.
    pub fn create(&self, password: &str) -> Result<()> {
        if password.chars().count() < 8 {
            return Err(ApiError::invalid("password", "Use at least 8 characters for the primary password."));
        }
        if self.exists() {
            return Err(ApiError::new(ErrorKind::Invalid, "A password vault already exists — unlock it instead."));
        }
        let mut salt = [0u8; SALT_LEN];
        rand::fill(&mut salt);
        let (m, t, p) = (Params::DEFAULT_M_COST, Params::DEFAULT_T_COST, Params::DEFAULT_P_COST);
        let key = derive_key(password, &salt, m, t, p)?;
        let u = Unlocked { key, salt, params: (m, t, p), map: HashMap::new() };
        self.write(&u)?;
        *self.open.lock() = Some(u);
        Ok(())
    }

    pub fn unlock(&self, password: &str) -> Result<()> {
        let raw = std::fs::read(&self.path).map_err(|e| ApiError::storage(format!("Could not read the password vault: {e}")))?;
        let f: VaultFile = serde_json::from_slice(&raw).map_err(|e| ApiError::storage(format!("The password vault is damaged: {e}")))?;
        if f.v != VAULT_VERSION {
            return Err(ApiError::storage(format!("This password vault was written by a newer WolfMail (format {}).", f.v)));
        }
        let salt_v = b64().decode(&f.salt).map_err(|_| ApiError::storage("The password vault is damaged (salt)."))?;
        let nonce_v = b64().decode(&f.nonce).map_err(|_| ApiError::storage("The password vault is damaged (nonce)."))?;
        let ct = b64().decode(&f.ciphertext).map_err(|_| ApiError::storage("The password vault is damaged (data)."))?;
        let salt: [u8; SALT_LEN] = salt_v.as_slice().try_into().map_err(|_| ApiError::storage("The password vault is damaged (salt length)."))?;
        let nonce = XNonce::try_from(nonce_v.as_slice()).map_err(|_| ApiError::storage("The password vault is damaged (nonce length)."))?;
        let key = derive_key(password, &salt, f.m_cost, f.t_cost, f.p_cost)?;
        let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| ApiError::storage("Bad vault key length."))?;
        // An authentication failure here means the password is wrong (or the file was tampered with).
        let plain = Zeroizing::new(
            cipher
                .decrypt(&nonce, ct.as_ref())
                .map_err(|_| ApiError { kind: ErrorKind::Auth, message: "That primary password is not right.".into(), field: Some("password".into()), account_id: None })?,
        );
        let map: HashMap<String, String> =
            serde_json::from_slice(&plain).map_err(|e| ApiError::storage(format!("The password vault is damaged: {e}")))?;
        *self.open.lock() = Some(Unlocked { key, salt, params: (f.m_cost, f.t_cost, f.p_cost), map });
        Ok(())
    }

    fn write(&self, u: &Unlocked) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(&u.map).map_err(|e| ApiError::storage(e.to_string()))?);
        let cipher = XChaCha20Poly1305::new_from_slice(u.key.as_ref()).map_err(|_| ApiError::storage("Bad vault key length."))?;
        let nonce = XNonce::generate();
        let ct = cipher.encrypt(&nonce, plain.as_ref()).map_err(|_| ApiError::storage("Could not encrypt the password vault."))?;
        let f = VaultFile {
            v: VAULT_VERSION,
            m_cost: u.params.0,
            t_cost: u.params.1,
            p_cost: u.params.2,
            salt: b64().encode(u.salt),
            nonce: b64().encode(nonce.as_slice()),
            ciphertext: b64().encode(&ct),
        };
        let json = serde_json::to_vec_pretty(&f).map_err(|e| ApiError::storage(e.to_string()))?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| ApiError::storage(format!("Could not create {}: {e}", dir.display())))?;
        }
        // Write-then-rename so a crash mid-save never leaves a half-written vault.
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &json).map_err(|e| ApiError::storage(format!("Could not save the password vault: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| ApiError::storage(format!("Could not save the password vault: {e}")))?;
        Ok(())
    }

    fn locked_err() -> ApiError {
        ApiError::storage("Your saved passwords are locked. Enter your WolfMail primary password to unlock them.")
    }
}

impl SecretStore for FileVault {
    fn get(&self, key: &str) -> Result<Option<String>> {
        let g = self.open.lock();
        let u = g.as_ref().ok_or_else(Self::locked_err)?;
        Ok(u.map.get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        let mut g = self.open.lock();
        let u = g.as_mut().ok_or_else(Self::locked_err)?;
        u.map.insert(key.to_string(), value.to_string());
        self.write(u)
    }
    fn delete(&self, key: &str) -> Result<()> {
        let mut g = self.open.lock();
        let u = g.as_mut().ok_or_else(Self::locked_err)?;
        if u.map.remove(key).is_some() {
            self.write(u)?;
        }
        Ok(())
    }
}

/// Where secrets live on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretsBackend {
    Keychain,
    File,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsStatus {
    pub backend: SecretsBackend,
    /// File backend only: a vault exists (false → the UI asks the user to choose a primary password).
    pub initialized: bool,
    pub locked: bool,
    /// Why the keychain was not used, shown to the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Picks the OS keychain when it works and the encrypted file otherwise. The choice is made once at startup and
/// also sticks to "file" whenever a vault already exists, so secrets never end up split across both.
pub struct AutoStore {
    backend: SecretsBackend,
    reason: Option<String>,
    vault: FileVault,
}

/// Probe the OS store with a read of a key that is never written: `NoEntry` (or a value) means it works; any other
/// error (no Secret Service on D-Bus, locked collection that can't be prompted, …) means it doesn't.
fn keychain_usable() -> std::result::Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, "__wolfmail_probe__").map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

impl AutoStore {
    pub fn detect(data_dir: &std::path::Path) -> Self {
        let vault = FileVault::new(data_dir.join("secrets.vault"));
        if vault.exists() {
            return Self { backend: SecretsBackend::File, reason: Some("Passwords are kept in WolfMail's encrypted vault on this computer.".into()), vault };
        }
        match keychain_usable() {
            Ok(()) => Self { backend: SecretsBackend::Keychain, reason: None, vault },
            Err(e) => Self {
                backend: SecretsBackend::File,
                reason: Some(format!("This computer has no system keychain WolfMail can use ({e}), so passwords are kept in an encrypted vault instead.")),
                vault,
            },
        }
    }

    pub fn status(&self) -> SecretsStatus {
        match self.backend {
            SecretsBackend::Keychain => SecretsStatus { backend: self.backend, initialized: true, locked: false, reason: None },
            SecretsBackend::File => SecretsStatus {
                backend: self.backend,
                initialized: self.vault.exists(),
                locked: !self.vault.is_unlocked(),
                reason: self.reason.clone(),
            },
        }
    }

    pub fn create_vault(&self, password: &str) -> Result<()> {
        self.vault.create(password)
    }

    pub fn unlock_vault(&self, password: &str) -> Result<()> {
        self.vault.unlock(password)
    }
}

impl SecretStore for AutoStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        match self.backend {
            SecretsBackend::Keychain => KeyringStore.get(key),
            SecretsBackend::File => self.vault.get(key),
        }
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        match self.backend {
            SecretsBackend::Keychain => KeyringStore.set(key, value),
            SecretsBackend::File => self.vault.set(key, value),
        }
    }
    fn delete(&self, key: &str) -> Result<()> {
        match self.backend {
            SecretsBackend::Keychain => KeyringStore.delete(key),
            SecretsBackend::File => self.vault.delete(key),
        }
    }
}

#[cfg(test)]
mod vault_tests {
    use super::*;

    #[test]
    fn vault_round_trip_wrong_password_and_tamper() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.vault");
        let v = FileVault::new(path.clone());
        assert!(v.get("a").is_err(), "locked vault must refuse reads");
        assert_eq!(v.create("short").unwrap_err().kind, ErrorKind::Invalid);
        v.create("correct horse battery").unwrap();
        v.set("password:acct:imap", "s3cret").unwrap();
        v.set("oauth:me@example.com", "refresh-token").unwrap();
        v.delete("oauth:me@example.com").unwrap();
        assert!(v.create("another password").is_err(), "must not overwrite an existing vault");

        // The plaintext must not appear in the file.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("s3cret") && !raw.contains("password:acct"));

        let v2 = FileVault::new(path.clone());
        assert_eq!(v2.unlock("wrong password!").unwrap_err().kind, ErrorKind::Auth);
        assert!(!v2.is_unlocked());
        v2.unlock("correct horse battery").unwrap();
        assert_eq!(v2.get("password:acct:imap").unwrap().as_deref(), Some("s3cret"));
        assert_eq!(v2.get("oauth:me@example.com").unwrap(), None);

        // Flip one ciphertext byte: authentication must fail rather than return garbage.
        let mut f: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let mut ct = b64().decode(f["ciphertext"].as_str().unwrap()).unwrap();
        ct[0] ^= 1;
        f["ciphertext"] = serde_json::Value::String(b64().encode(&ct));
        std::fs::write(&path, serde_json::to_vec(&f).unwrap()).unwrap();
        assert!(FileVault::new(path).unlock("correct horse battery").is_err());
    }

    #[test]
    fn existing_vault_forces_file_backend() {
        let dir = tempfile::tempdir().unwrap();
        FileVault::new(dir.path().join("secrets.vault")).create("correct horse battery").unwrap();
        let s = AutoStore::detect(dir.path());
        let st = s.status();
        assert_eq!(st.backend, SecretsBackend::File);
        assert!(st.initialized && st.locked);
        s.unlock_vault("correct horse battery").unwrap();
        assert!(!s.status().locked);
    }
}
