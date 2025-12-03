//! NEAR Intents Bridge API models
//!
//! Type definitions for Bridge API requests and responses matching the real
//! NEAR Intents Bridge at https://bridge.chaindefuser.com/rpc

use serde::{Deserialize, Serialize};

/// JSON-RPC request wrapper
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest<T> {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: Vec<T>,
}

impl<T> JsonRpcRequest<T> {
    pub fn new(method: &str, params: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: method.to_string(),
            params: vec![params],
        }
    }
}

/// JSON-RPC response wrapper
#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
pub struct JsonRpcResponse<T> {
    pub result: Option<T>,
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcError {
    /// Standard JSON-RPC 2.0 error with code and message
    Standard { code: i64, message: String },
    /// Non-standard error (just a string)
    Simple(String),
}

impl JsonRpcError {
    pub fn message(&self) -> &str {
        match self {
            Self::Standard { message, .. } => message,
            Self::Simple(msg) => msg,
        }
    }

    pub fn code(&self) -> i64 {
        match self {
            Self::Standard { code, .. } => *code,
            Self::Simple(_) => -32000, // Generic error code
        }
    }
}

// ============================================================================
// Chain Format Helper
// ============================================================================

/// Represents a chain in NEAR Intents format (e.g., "eth:1", "eth:42161")
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChainId {
    pub chain_type: String,
    pub chain_id: String,
}

impl ChainId {
    /// Create a new chain ID
    pub fn new(chain_type: &str, chain_id: &str) -> Self {
        Self {
            chain_type: chain_type.to_string(),
            chain_id: chain_id.to_string(),
        }
    }

    /// Ethereum Mainnet
    pub fn ethereum_mainnet() -> Self {
        Self::new("eth", "1")
    }

    /// Arbitrum One
    pub fn arbitrum() -> Self {
        Self::new("eth", "42161")
    }

    /// Base
    pub fn base() -> Self {
        Self::new("eth", "8453")
    }

    /// Parse from string format "type:id"
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        if parts.len() == 2 {
            Some(Self::new(parts[0], parts[1]))
        } else {
            None
        }
    }
}

impl std::fmt::Display for ChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.chain_type, self.chain_id)
    }
}

impl Serialize for ChainId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ChainId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom("invalid chain format"))
    }
}

// ============================================================================
// Supported Tokens
// ============================================================================

#[derive(Debug, Serialize)]
pub struct SupportedTokensParams {
    pub chains: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SupportedTokensResult {
    pub tokens: Vec<TokenInfo>,
}

/// Token information from NEAR Intents Bridge
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TokenInfo {
    /// Defuse asset identifier (e.g., "eth:1:native" or "eth:1:0xa0b86991...")
    pub defuse_asset_identifier: String,

    /// NEAR token ID that represents this asset on NEAR
    pub near_token_id: String,

    /// Number of decimals for the token
    pub decimals: u8,

    /// Human-readable asset name (e.g., "ETH", "USDC")
    pub asset_name: String,

    /// Minimum deposit amount in smallest units
    #[serde(default)]
    pub min_deposit_amount: Option<String>,

    /// Withdrawal fee in smallest units
    #[serde(default)]
    pub withdrawal_fee: Option<String>,

    /// Full token ID for intents.near (NEP-245 format if applicable)
    #[serde(default)]
    pub intents_token_id: Option<String>,

    /// Multi-token ID component (for NEP-245 tokens)
    #[serde(default)]
    pub multi_token_id: Option<String>,
}

impl TokenInfo {
    /// Get the chain ID from defuse_asset_identifier
    pub fn chain(&self) -> Option<String> {
        let parts: Vec<&str> = self.defuse_asset_identifier.split(':').collect();
        if parts.len() >= 2 {
            Some(format!("{}:{}", parts[0], parts[1]))
        } else {
            None
        }
    }

    /// Get the token address from defuse_asset_identifier
    pub fn token_address(&self) -> Option<String> {
        let parts: Vec<&str> = self.defuse_asset_identifier.split(':').collect();
        if parts.len() >= 3 {
            Some(parts[2].to_string())
        } else {
            None
        }
    }

    /// Check if this is a native token (ETH, SOL, etc.)
    pub fn is_native(&self) -> bool {
        self.defuse_asset_identifier.ends_with(":native")
    }

    /// Check if this is a NEP-245 multi-token
    pub fn is_nep245(&self) -> bool {
        self.intents_token_id
            .as_ref()
            .map(|id| id.starts_with("nep245:"))
            .unwrap_or(false)
            || self.multi_token_id.is_some()
    }

    /// Get the full token ID for withdrawals
    /// For NEP-245 tokens on intents.near, use the intents_token_id format
    /// For NEP-141 tokens, use the near_token_id
    pub fn withdrawal_token_id(&self) -> String {
        if let Some(intents_id) = &self.intents_token_id {
            // Use the full intents_token_id (e.g., "nep245:v2_1.omni.hot.tg:1100_111...")
            intents_id.clone()
        } else if let Some(multi_token_id) = &self.multi_token_id {
            // Fallback: construct from parts
            format!("{}:{}", self.near_token_id, multi_token_id)
        } else {
            // NEP-141 token
            self.near_token_id.clone()
        }
    }
}

// ============================================================================
// Deposit Address
// ============================================================================

#[derive(Debug, Serialize)]
pub struct DepositAddressParams {
    /// NEAR account ID to deposit to
    pub account_id: String,
    /// Chain in format "type:id" (e.g., "eth:1")
    pub chain: String,
    /// Deposit mode ("SIMPLE" for unique addresses, "MEMO" for shared address with memo)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposit_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DepositAddressResult {
    /// The deposit address on the external chain
    pub address: String,
    /// Chain identifier
    pub chain: String,
    /// Memo for MEMO-based deposits (Stellar, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

// ============================================================================
// Recent Deposits
// ============================================================================

#[derive(Debug, Serialize)]
pub struct RecentDepositsParams {
    pub account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct RecentDepositsResult {
    pub deposits: Vec<DepositInfo>,
}

/// Deposit status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DepositStatus {
    Pending,
    Confirmed,
    Completed,
    Failed,
}

impl std::fmt::Display for DepositStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "PENDING"),
            Self::Confirmed => write!(f, "CONFIRMED"),
            Self::Completed => write!(f, "COMPLETED"),
            Self::Failed => write!(f, "FAILED"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DepositInfo {
    /// Transaction hash on the source chain
    pub tx_hash: String,
    /// Amount deposited in smallest units
    pub amount: String,
    /// Deposit status
    pub status: DepositStatus,
    /// Defuse asset identifier
    pub defuse_asset_identifier: String,
    /// Source chain
    pub chain: String,
    /// NEAR transaction hash when completed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near_tx_hash: Option<String>,
    /// Timestamp when completed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

// ============================================================================
// Notify Deposit
// ============================================================================

/// Notify the bridge of a deposit transaction
#[derive(Debug, Serialize)]
pub struct NotifyDepositParams {
    /// Transaction hash on the source chain
    pub tx_hash: String,
    /// Chain where deposit was made
    pub chain: String,
}

#[derive(Debug, Deserialize)]
pub struct NotifyDepositResult {
    /// Acknowledgment status
    pub acknowledged: bool,
}

// ============================================================================
// Token Mapping Helper
// ============================================================================

/// Maps external chain tokens to NEAR representations
#[derive(Debug, Clone)]
pub struct TokenMapping {
    /// Token info from bridge
    pub info: TokenInfo,
}

impl TokenMapping {
    /// Get the USDC token for a specific chain
    pub fn usdc_for_chain(chain: &ChainId) -> Option<String> {
        match (chain.chain_type.as_str(), chain.chain_id.as_str()) {
            ("eth", "1") => Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string()),
            ("eth", "42161") => Some("0xaf88d065e77c8cc2239327c5edb3a432268e5831".to_string()), // Arbitrum
            ("eth", "8453") => Some("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string()), // Base
            _ => None,
        }
    }

    /// Get the USDT token for a specific chain
    pub fn usdt_for_chain(chain: &ChainId) -> Option<String> {
        match (chain.chain_type.as_str(), chain.chain_id.as_str()) {
            ("eth", "1") => Some("0xdac17f958d2ee523a2206206994597c13d831ec7".to_string()),
            ("eth", "42161") => Some("0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9".to_string()), // Arbitrum
            _ => None,
        }
    }

    /// Create defuse asset identifier
    pub fn defuse_asset_id(chain: &ChainId, token_address: &str) -> String {
        format!("{}:{}:{}", chain.chain_type, chain.chain_id, token_address)
    }

    /// Create defuse asset identifier for native token
    pub fn defuse_native_asset_id(chain: &ChainId) -> String {
        format!("{}:{}:native", chain.chain_type, chain.chain_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_id_parsing() {
        let chain = ChainId::parse("eth:1").unwrap();
        assert_eq!(chain.chain_type, "eth");
        assert_eq!(chain.chain_id, "1");
        assert_eq!(chain.to_string(), "eth:1");
    }

    #[test]
    fn test_chain_id_presets() {
        let eth = ChainId::ethereum_mainnet();
        assert_eq!(eth.to_string(), "eth:1");

        let arb = ChainId::arbitrum();
        assert_eq!(arb.to_string(), "eth:42161");

        let base = ChainId::base();
        assert_eq!(base.to_string(), "eth:8453");
    }

    #[test]
    fn test_chain_id_serialization() {
        let chain = ChainId::ethereum_mainnet();
        let json = serde_json::to_string(&chain).unwrap();
        assert_eq!(json, "\"eth:1\"");
    }

    #[test]
    fn test_chain_id_deserialization() {
        let chain: ChainId = serde_json::from_str("\"eth:42161\"").unwrap();
        assert_eq!(chain.chain_type, "eth");
        assert_eq!(chain.chain_id, "42161");
    }

    #[test]
    fn test_json_rpc_request_with_array_params() {
        let params = SupportedTokensParams {
            chains: vec!["eth:1".to_string()],
        };
        let request = JsonRpcRequest::new("supported_tokens", params);

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"params\":[{\"chains\":[\"eth:1\"]}]"));
    }

    #[test]
    fn test_token_info_deserialization_real_api() {
        let json = r#"{
            "defuse_asset_identifier": "eth:1:native",
            "near_token_id": "eth.omft.near",
            "decimals": 18,
            "asset_name": "ETH",
            "min_deposit_amount": "1",
            "withdrawal_fee": "35000000000000"
        }"#;

        let info: TokenInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.defuse_asset_identifier, "eth:1:native");
        assert_eq!(info.near_token_id, "eth.omft.near");
        assert_eq!(info.decimals, 18);
        assert_eq!(info.asset_name, "ETH");
        assert_eq!(info.min_deposit_amount, Some("1".to_string()));
        assert_eq!(info.withdrawal_fee, Some("35000000000000".to_string()));
        assert!(info.is_native());
        assert_eq!(info.chain(), Some("eth:1".to_string()));
        assert_eq!(info.token_address(), Some("native".to_string()));
    }

    #[test]
    fn test_token_info_usdc() {
        let json = r#"{
            "defuse_asset_identifier": "eth:1:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "near_token_id": "eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near",
            "decimals": 6,
            "asset_name": "USDC",
            "withdrawal_fee": "300000"
        }"#;

        let info: TokenInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.asset_name, "USDC");
        assert_eq!(info.decimals, 6);
        assert!(!info.is_native());
        assert_eq!(
            info.token_address(),
            Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string())
        );
    }

    #[test]
    fn test_deposit_address_params() {
        let params = DepositAddressParams {
            account_id: "tmplr-liq.near".to_string(),
            chain: "eth:1".to_string(),
        };

        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"account_id\":\"tmplr-liq.near\""));
        assert!(json.contains("\"chain\":\"eth:1\""));
    }

    #[test]
    fn test_deposit_address_result_real_api() {
        let json = r#"{
            "address": "0xbA5C6ABBAe64AD465d104949CC150011C1777eFB",
            "chain": "eth:1"
        }"#;

        let result: DepositAddressResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.address, "0xbA5C6ABBAe64AD465d104949CC150011C1777eFB");
        assert_eq!(result.chain, "eth:1");
    }

    #[test]
    fn test_deposit_status_serialization() {
        assert_eq!(
            serde_json::to_string(&DepositStatus::Pending).unwrap(),
            "\"PENDING\""
        );
        assert_eq!(
            serde_json::to_string(&DepositStatus::Completed).unwrap(),
            "\"COMPLETED\""
        );
    }

    #[test]
    fn test_deposit_info() {
        let json = r#"{
            "tx_hash": "0x123abc",
            "amount": "1000000",
            "status": "COMPLETED",
            "defuse_asset_identifier": "eth:1:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "chain": "eth:1",
            "near_tx_hash": "ABC123",
            "completed_at": "2025-11-14T12:00:00Z"
        }"#;

        let info: DepositInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.tx_hash, "0x123abc");
        assert_eq!(info.status, DepositStatus::Completed);
        assert_eq!(info.near_tx_hash, Some("ABC123".to_string()));
    }

    #[test]
    fn test_notify_deposit_params() {
        let params = NotifyDepositParams {
            tx_hash: "0xabc123".to_string(),
            chain: "eth:1".to_string(),
        };

        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"tx_hash\":\"0xabc123\""));
        assert!(json.contains("\"chain\":\"eth:1\""));
    }

    #[test]
    fn test_token_mapping_usdc() {
        let eth = ChainId::ethereum_mainnet();
        let usdc = TokenMapping::usdc_for_chain(&eth).unwrap();
        assert_eq!(usdc, "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");

        let arb = ChainId::arbitrum();
        let arb_usdc = TokenMapping::usdc_for_chain(&arb).unwrap();
        assert_eq!(arb_usdc, "0xaf88d065e77c8cc2239327c5edb3a432268e5831");
    }

    #[test]
    fn test_defuse_asset_id() {
        let eth = ChainId::ethereum_mainnet();
        let usdc_addr = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";

        let defuse_id = TokenMapping::defuse_asset_id(&eth, usdc_addr);
        assert_eq!(
            defuse_id,
            "eth:1:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
        );

        let native_id = TokenMapping::defuse_native_asset_id(&eth);
        assert_eq!(native_id, "eth:1:native");
    }
}
