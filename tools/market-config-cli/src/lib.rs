pub mod calculator;
pub mod config;
pub mod contract;
pub mod curve;
pub mod logger;
pub mod oracle;
pub mod output;
pub mod rpc;
pub mod ui;

pub use calculator::InterestRateCalculator;
pub use config::{ConfigBuilder, ConfigValidator};
pub use contract::ContractReader;
use near_jsonrpc_client::{errors::JsonRpcError, methods::query::RpcQueryError};
pub use oracle::PriceValidator;
pub use output::ConfigFormatter;
pub use templar_common::market::MarketConfiguration;
pub use ui::prompt::wizard::MarketPrompter as ConfigEditor;
pub use ui::prompt::wizard::MarketPrompter as InteractivePrompt;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("Configuration validation error: {0}")]
    Validation(String),

    #[error("Contract interaction error: {0}")]
    Contract(String),

    #[error("Oracle error: {0}")]
    Oracle(String),

    #[error("Prompt error: {0}")]
    Prompt(String),

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

    /// User interrupted an interactive prompt
    #[error("Interrupted by user")]
    Interrupted,

    /// Error already reported to user; suppress default error printing
    #[error("{0}")]
    Silent(String),
}

pub type CliResult<T = ()> = Result<T, CliError>;
