//! REST API request and response models
//!
//! These models define the JSON API contract for the funding-bridge service.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Request to deposit funds from external wallet to NEAR treasury
///
/// Triggers an automated transfer from a configured external wallet
/// (ETH/Arbitrum) to the bridge deposit address, which then credits
/// the NEAR treasury with OMFT tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositRequest {
    /// Unique identifier for this request
    pub request_id: String,

    /// Source chain to transfer from (e.g., "ethereum", "arbitrum", "eth:42161")
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
    /// Same request_id from the request
    pub request_id: String,

    /// Current status
    pub status: String,

    /// Source chain used
    pub source_chain: String,

    /// Transaction hash on source chain
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_tx_hash: Option<String>,

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
    /// Unique identifier for this request
    pub request_id: String,

    /// NEAR account to withdraw from
    pub source_account: String,

    /// Destination chain (e.g., "ethereum", "solana")
    pub destination_chain: String,

    /// Destination address on target chain
    pub destination_address: String,

    /// Asset identifier
    pub asset: String,

    /// Amount to withdraw
    pub amount: String,

    /// If true, log actions but don't execute
    #[serde(default)]
    pub dry_run: bool,

    /// Additional metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Response for withdraw request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawResponse {
    /// Same request_id from the request
    pub request_id: String,

    /// Current status
    pub status: WithdrawStatus,

    /// NEAR transaction hash (if initiated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_tx_hash: Option<String>,

    /// Bridge transaction ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_tx_id: Option<String>,

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

/// Request to check status of an operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusRequest {
    /// Request ID to check
    pub request_id: String,
}

/// Response with operation status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    /// Request ID
    pub request_id: String,

    /// Type of operation
    pub operation_type: OperationType,

    /// Current status (varies by operation type)
    pub status: String,

    /// Additional status details
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub details: HashMap<String, String>,
}

/// Type of operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationType {
    Deposit,
    Withdraw,
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
            request_id: "req-123".to_string(),
            source_chain: "ethereum".to_string(),
            asset: "USDC".to_string(),
            amount: "100.5".to_string(),
            dry_run: false,
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: DepositRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.request_id, "req-123");
        assert_eq!(parsed.asset, "USDC");
        assert_eq!(parsed.amount, "100.5");
    }

    #[test]
    fn test_deposit_response_serialization() {
        let resp = DepositResponse {
            request_id: "req-123".to_string(),
            status: "PENDING".to_string(),
            source_chain: "eth:42161".to_string(),
            source_tx_hash: Some("0xabc123".to_string()),
            bridge_deposit_address: Some("0xdef456".to_string()),
            error: None,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("PENDING"));
        assert!(json.contains("0xabc123"));
    }

    #[test]
    fn test_withdraw_request_serialization() {
        let req = WithdrawRequest {
            request_id: "req-456".to_string(),
            source_account: "user.near".to_string(),
            destination_chain: "ethereum".to_string(),
            destination_address: "0x123".to_string(),
            asset: "usdt".to_string(),
            amount: "500000".to_string(),
            dry_run: true,
            metadata: HashMap::new(),
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("req-456"));
        assert!(json.contains("ethereum"));
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
    fn test_operation_type_serialization() {
        let op = OperationType::Deposit;
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, "\"deposit\"");

        let op = OperationType::Withdraw;
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, "\"withdraw\"");
    }
}
