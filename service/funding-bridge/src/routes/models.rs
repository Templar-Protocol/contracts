//! REST API request and response models
//!
//! These models define the JSON API contract for the funding-bridge service.

use serde::{Deserialize, Serialize};

/// Request to deposit funds from external wallet to NEAR treasury
///
/// Triggers an automated transfer from a configured external wallet
/// (ETH/Arbitrum/Solana) to the bridge deposit address, which then credits
/// the NEAR treasury with OMFT tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositRequest {
    /// Source chain to transfer from (e.g., "ethereum", "arbitrum", "eth:42161", "solana")
    pub source_chain: String,

    /// Asset to transfer (e.g., "USDC", "USDT")
    pub asset: String,

    /// Amount to transfer (in human-readable format, e.g., "100.5")
    pub amount: String,

    /// If true, log actions but don't execute
    #[serde(default)]
    pub dry_run: bool,
}

/// Response for deposit request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositResponse {
    /// Transaction hash on source chain (use this for status tracking)
    pub source_tx_hash: String,

    /// Current status
    pub status: String,

    /// Source chain used
    pub source_chain: String,

    /// Bridge deposit address used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_deposit_address: Option<String>,

    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Request to withdraw funds from NEAR to external chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawRequest {
    /// Destination chain (e.g., "ethereum", "solana")
    pub destination_chain: String,

    /// Destination address on target chain
    pub destination_address: String,

    /// Asset identifier
    pub asset: String,

    /// Amount to withdraw (in smallest units)
    pub amount: String,

    /// If true, log actions but don't execute
    #[serde(default)]
    pub dry_run: bool,
}

/// Response for withdraw request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawResponse {
    /// NEAR transaction hash (use this for status tracking via Bridge API)
    pub source_tx_hash: String,

    /// Current status
    pub status: WithdrawStatus,

    /// Destination transaction hash (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_tx_hash: Option<String>,

    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Status of a withdrawal operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WithdrawStatus {
    /// Withdrawal completed
    Completed,

    /// Withdrawal is pending
    Pending,

    /// Withdrawal failed
    Failed,
}

/// Response for withdrawal status check (from Bridge API)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawalStatusResponse {
    /// NEAR transaction hash
    pub near_tx_hash: String,

    /// Current status
    pub status: String,

    /// Destination chain
    pub chain: String,

    /// Destination transaction hash (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_tx_hash: Option<String>,

    /// Amount transferred
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
}

/// Response for deposit status check (from Bridge API)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositStatusResponse {
    /// Source chain transaction hash
    pub tx_hash: String,

    /// Current status
    pub status: String,

    /// Source chain
    pub chain: String,

    /// NEAR transaction hash (when completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near_tx_hash: Option<String>,

    /// Amount deposited
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
}

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Service is healthy
    pub healthy: bool,

    /// Service version
    pub version: String,

    /// Available chains
    pub chains: Vec<ChainStatus>,

    /// RPC connectivity status (if RPC is configured)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_status: Option<ServiceStatus>,

    /// Bridge API connectivity status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_api_status: Option<ServiceStatus>,
}

/// Status of a chain handler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainStatus {
    /// Chain name
    pub name: String,

    /// Is chain available
    pub available: bool,

    /// Chain priority (0 = highest)
    pub priority: u8,
}

/// Status of an external service dependency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// Service is reachable
    pub reachable: bool,

    /// Response latency in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,

    /// Error message if unreachable
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deposit_request_serialization() {
        let req = DepositRequest {
            source_chain: "ethereum".to_string(),
            asset: "USDC".to_string(),
            amount: "100.5".to_string(),
            dry_run: false,
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: DepositRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.source_chain, "ethereum");
        assert_eq!(parsed.asset, "USDC");
        assert_eq!(parsed.amount, "100.5");
    }

    #[test]
    fn test_deposit_response_serialization() {
        let resp = DepositResponse {
            source_tx_hash: "0xabc123".to_string(),
            status: "PENDING".to_string(),
            source_chain: "eth:42161".to_string(),
            bridge_deposit_address: Some("0xdef456".to_string()),
            error: None,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("PENDING"));
        assert!(json.contains("0xabc123"));
        assert!(json.contains("0xdef456"));
    }

    #[test]
    fn test_withdraw_request_serialization() {
        let req = WithdrawRequest {
            destination_chain: "ethereum".to_string(),
            destination_address: "0x123".to_string(),
            asset: "usdt".to_string(),
            amount: "500000".to_string(),
            dry_run: true,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("ethereum"));
        assert!(json.contains("0x123"));
        assert!(json.contains("500000"));
    }

    #[test]
    fn test_withdraw_response_serialization() {
        let resp = WithdrawResponse {
            source_tx_hash: "7abc123def".to_string(),
            status: WithdrawStatus::Pending,
            destination_tx_hash: None,
            error: None,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("7abc123def"));
        assert!(json.contains("PENDING"));
    }

    #[test]
    fn test_health_response() {
        let health = HealthResponse {
            healthy: true,
            version: "0.1.0".to_string(),
            chains: vec![
                ChainStatus {
                    name: "near".to_string(),
                    available: true,
                    priority: 0,
                },
                ChainStatus {
                    name: "ethereum".to_string(),
                    available: false,
                    priority: 2,
                },
            ],
            rpc_status: None,
            bridge_api_status: None,
        };

        let json = serde_json::to_string(&health).unwrap();
        assert!(json.contains("0.1.0"));
        assert!(json.contains("near"));
        // Optional fields should be omitted when None
        assert!(!json.contains("rpc_status"));
        assert!(!json.contains("bridge_api_status"));
    }

    #[test]
    fn test_withdraw_status_serialization() {
        let status = WithdrawStatus::Completed;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"COMPLETED\"");

        let status = WithdrawStatus::Failed;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"FAILED\"");
    }
}
