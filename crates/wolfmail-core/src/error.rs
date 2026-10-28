// Source: docs/API.md "Errors" — the wire shape of every failure the UI shows.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    Auth,
    Network,
    Tls,
    Server,
    NotFound,
    Invalid,
    Storage,
    OauthConfig,
    Cancelled,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct ApiError {
    pub kind: ErrorKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

pub type Result<T> = std::result::Result<T, ApiError>;

impl ApiError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), field: None, account_id: None }
    }
    pub fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Invalid, message: message.into(), field: Some(field.into()), account_id: None }
    }
    pub fn not_found(what: &str) -> Self {
        Self::new(ErrorKind::NotFound, format!("{what} was not found."))
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }
    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Storage, message)
    }
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, message)
    }
    pub fn server(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Server, message)
    }
    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Auth, message)
    }
    pub fn with_account(mut self, id: &str) -> Self {
        self.account_id = Some(id.to_string());
        self
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => ApiError::not_found("The item"),
            other => ApiError::storage(format!("Local storage error: {other}")),
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::internal(format!("Data encoding error: {e}"))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        match e.kind() {
            K::ConnectionRefused | K::ConnectionReset | K::ConnectionAborted | K::TimedOut | K::NotConnected
            | K::BrokenPipe | K::UnexpectedEof | K::HostUnreachable | K::NetworkUnreachable => {
                ApiError::network(format!("Connection problem: {e}"))
            }
            K::NotFound => ApiError::not_found("The file"),
            _ => ApiError::internal(format!("I/O error: {e}")),
        }
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        // reqwest's Display never includes request bodies or Authorization headers.
        if e.is_timeout() || e.is_connect() {
            ApiError::network(format!("Could not reach the server: {e}"))
        } else {
            ApiError::server(format!("Server request failed: {e}"))
        }
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(e: tokio::task::JoinError) -> Self {
        ApiError::internal(format!("Background task failed: {e}"))
    }
}
