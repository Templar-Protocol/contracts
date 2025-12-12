//! Error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum MonitorError {
    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Market error: {0}")]
    Market(String),

    #[error("Telegram error: {0}")]
    Telegram(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, MonitorError>;
