//! Error types for funding-bridge service
//!
//! This module defines all error types used throughout the service.
//! Uses thiserror for ergonomic error handling.

use thiserror::Error;

/// Main error type for funding operations
#[derive(Debug, Error)]
pub enum FundingError {
    /// Asset not supported by the service
    #[error("Asset not supported: {0}")]
    UnsupportedAsset(String),

    /// Insufficient funds across all configured treasuries
    #[error("Insufficient funds: required {required}, available {available}")]
    InsufficientFunds { required: u128, available: u128 },

    /// Requested chain is not configured or enabled
    #[error("Chain not configured: {0}")]
    ChainNotConfigured(String),

    /// Invalid request parameters
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Invalid NEAR account ID format
    #[error("Invalid account ID: {0}")]
    InvalidAccountId(String),

    /// Invalid amount (zero or negative)
    #[error("Invalid amount: {0}")]
    InvalidAmount(String),

    /// Bridge API error
    #[error("Bridge API error: {0}")]
    BridgeError(#[from] BridgeError),

    /// Chain-specific error
    #[error("Chain error ({chain}): {source}")]
    ChainError {
        chain: String,
        #[source]
        source: ChainError,
    },

    /// NEAR RPC error
    #[error("NEAR RPC error: {0}")]
    NearRpcError(String),

    /// HTTP request error
    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    /// JSON serialization/deserialization error
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Bridge API specific errors
#[derive(Debug, Error)]
pub enum BridgeError {
    /// Bridge API returned an error
    #[error("Bridge API error: {0}")]
    ApiError(String),

    /// Deposit address request failed
    #[error("Failed to get deposit address: {0}")]
    DepositAddressFailed(String),

    /// Deposit timeout
    #[error("Deposit timed out after {0} seconds")]
    DepositTimeout(u64),

    /// Withdrawal status check failed
    #[error("Failed to get withdrawal status: {0}")]
    WithdrawalStatusFailed(String),

    /// Unsupported network
    #[error("Unsupported network: {0}")]
    UnsupportedNetwork(String),

    /// Unsupported token
    #[error("Unsupported token: {0}")]
    UnsupportedToken(String),

    /// HTTP error
    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    /// JSON error
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// Chain-specific errors
#[derive(Debug, Error)]
pub enum ChainError {
    /// Failed to query balance
    #[error("Failed to get balance: {0}")]
    BalanceQueryFailed(String),

    /// Failed to send transaction
    #[error("Failed to send transaction: {0}")]
    TransactionFailed(String),

    /// Insufficient balance
    #[error("Insufficient balance: required {required}, available {available}")]
    InsufficientBalance { required: u128, available: u128 },

    /// Invalid address format
    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    /// RPC error
    #[error("RPC error: {0}")]
    RpcError(String),

    /// Chain not available
    #[error("Chain not available: {0}")]
    ChainUnavailable(String),

    /// Configuration error (invalid keys, URLs, etc.)
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Unsupported asset type
    #[error("Unsupported asset: {0}")]
    UnsupportedAsset(String),

    /// Invalid recipient address
    #[error("Invalid recipient: {0}")]
    InvalidRecipient(String),

    /// Invalid amount
    #[error("Invalid amount: {0}")]
    InvalidAmount(String),
}

/// Result type alias for funding operations
pub type FundingResult<T> = Result<T, FundingError>;

/// Result type alias for bridge operations
pub type BridgeResult<T> = Result<T, BridgeError>;

/// Result type alias for chain operations
pub type ChainResult<T> = Result<T, ChainError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn test_funding_error_display() {
        let error = FundingError::UnsupportedAsset("btc".to_string());
        assert_eq!(error.to_string(), "Asset not supported: btc");
    }

    #[test]
    fn test_insufficient_funds_error() {
        let error = FundingError::InsufficientFunds {
            required: 1000,
            available: 500,
        };
        assert_eq!(
            error.to_string(),
            "Insufficient funds: required 1000, available 500"
        );
    }

    #[test]
    fn test_chain_not_configured_error() {
        let error = FundingError::ChainNotConfigured("polygon".to_string());
        assert_eq!(error.to_string(), "Chain not configured: polygon");
    }

    #[test]
    fn test_invalid_request_error() {
        let error = FundingError::InvalidRequest("amount must be positive".to_string());
        assert_eq!(
            error.to_string(),
            "Invalid request: amount must be positive"
        );
    }

    #[test]
    fn test_bridge_error_display() {
        let error = BridgeError::ApiError("rate limit exceeded".to_string());
        assert_eq!(error.to_string(), "Bridge API error: rate limit exceeded");
    }

    #[test]
    fn test_bridge_deposit_timeout() {
        let error = BridgeError::DepositTimeout(900);
        assert_eq!(error.to_string(), "Deposit timed out after 900 seconds");
    }

    #[test]
    fn test_chain_error_display() {
        let error = ChainError::BalanceQueryFailed("connection timeout".to_string());
        assert_eq!(
            error.to_string(),
            "Failed to get balance: connection timeout"
        );
    }

    #[test]
    fn test_chain_insufficient_balance() {
        let error = ChainError::InsufficientBalance {
            required: 2000,
            available: 1000,
        };
        assert_eq!(
            error.to_string(),
            "Insufficient balance: required 2000, available 1000"
        );
    }

    #[test]
    fn test_error_source() {
        let chain_error = ChainError::RpcError("timeout".to_string());
        let funding_error = FundingError::ChainError {
            chain: "ethereum".to_string(),
            source: chain_error,
        };

        assert!(funding_error.source().is_some());
        assert_eq!(
            funding_error.to_string(),
            "Chain error (ethereum): RPC error: timeout"
        );
    }

    #[test]
    fn test_bridge_error_conversion() {
        let bridge_error = BridgeError::UnsupportedNetwork("bitcoin".to_string());
        let funding_error: FundingError = bridge_error.into();

        match funding_error {
            FundingError::BridgeError(_) => {}
            _ => panic!("Expected BridgeError variant"),
        }
    }

    #[test]
    fn test_result_type_aliases() {
        fn returns_funding_result() -> FundingResult<u128> {
            Ok(1000)
        }

        fn returns_bridge_result() -> BridgeResult<String> {
            Ok("success".to_string())
        }

        fn returns_chain_result() -> ChainResult<bool> {
            Ok(true)
        }

        assert_eq!(returns_funding_result().unwrap(), 1000);
        assert_eq!(returns_bridge_result().unwrap(), "success");
        assert!(returns_chain_result().unwrap());
    }

    #[test]
    fn test_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FundingError>();
        assert_send_sync::<BridgeError>();
        assert_send_sync::<ChainError>();
    }
}
