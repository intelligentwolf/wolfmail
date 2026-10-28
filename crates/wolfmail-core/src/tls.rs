//! TLS for IMAP (and the shared crypto provider for reqwest/lettre): rustls with the OS trust store via
//! rustls-platform-verifier, so corporate/self-installed CAs work the same way they do in the user's browser.
use crate::error::{ApiError, ErrorKind, Result};
use rustls::ClientConfig;
use rustls_platform_verifier::ConfigVerifierExt;
use std::sync::{Arc, OnceLock};

/// Install ring as the process-wide rustls provider exactly once (reqwest is built with `rustls-no-provider`).
pub fn install_crypto_provider() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub fn client_config() -> Result<Arc<ClientConfig>> {
    install_crypto_provider();
    static CFG: OnceLock<std::result::Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CFG.get_or_init(|| ClientConfig::with_platform_verifier().map(Arc::new).map_err(|e| e.to_string()))
        .clone()
        .map_err(|e| ApiError::new(ErrorKind::Tls, format!("Could not load the system certificate store: {e}")))
}

pub fn tls_error(host: &str, e: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        ErrorKind::Tls,
        format!("The secure connection to {host} failed: {e}. The server's certificate may be invalid or expired."),
    )
}
