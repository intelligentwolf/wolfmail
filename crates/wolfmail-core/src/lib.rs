//! WolfMail engine: storage, IMAP sync, MIME, SMTP outbox, OAuth, autodetect, import, IntelligentWolf client.
// Source: docs/API.md is the contract this crate implements.
pub mod db;
pub mod error;
pub mod imap;
pub mod mime;
pub mod model;
pub mod msgstore;
pub mod search;
pub mod secrets;
pub mod threading;
pub mod tls;
pub mod utf7;
pub mod smtp;
pub mod autodetect;
pub mod import;
pub mod oauth;
pub mod iw;
pub mod providers;
pub mod engine;
