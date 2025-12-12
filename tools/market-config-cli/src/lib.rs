pub mod calculator;
pub mod common;
pub mod config;
pub mod contract;
pub mod curve;
pub mod editor;
pub mod interactive;
pub mod logger;
pub mod oracle;
pub mod output;
pub mod rpc;

pub use calculator::InterestRateCalculator;
pub use config::{ConfigBuilder, ConfigValidator};
pub use contract::ContractReader;
pub use editor::ConfigEditor;
pub use interactive::InteractivePrompt;
use near_jsonrpc_client::{errors::JsonRpcError, methods::query::RpcQueryError};
pub use oracle::PriceValidator;
pub use output::ConfigFormatter;
pub use templar_common::market::MarketConfiguration;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("Configuration validation error: {0}")]
    Validation(String),

    #[error("Contract interaction error: {0}")]
    Contract(String),

    #[error("Oracle error: {0}")]
    Oracle(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("NEAR RPC error: {0}")]
    NearRpc(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Invalid output: {0}")]
    InvalidOutput(String),

    /// Got wrong response kind from RPC
    #[error("Got wrong response kind from RPC: {0}")]
    WrongResponseKind(String),

    /// Failed to query view method
    #[error("Failed to query view method: {0}")]
    ViewMethodError(#[from] JsonRpcError<RpcQueryError>),

    /// Other errors
    #[error("Other error: {0}")]
    Other(String),
}

pub type CliResult<T = ()> = Result<T, CliError>;
