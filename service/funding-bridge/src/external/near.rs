//! NEAR blockchain handler for NEP-141 token transfers
//!
//! Handles deposits from external NEAR wallets to the bridge.

use super::{ExternalChainError, ExternalChainHandler, TransferResult};
use async_trait::async_trait;
use near_crypto::SecretKey;
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    transaction::{Action, FunctionCallAction, SignedTransaction, Transaction, TransactionV0},
    types::AccountId,
};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{error, info};

/// NEAR asset information
#[derive(Debug, Clone)]
pub struct NearAsset {
    /// Asset symbol (e.g., "USDC", "USDT")
    pub symbol: String,
    /// Token contract address
    pub contract: String,
    /// Decimals for the token
    pub decimals: u8,
}

/// NEAR chain configuration
#[derive(Debug, Clone)]
pub struct NearConfig {
    /// Chain identifier (e.g., "near:mainnet", "near:testnet")
    pub chain_id: String,
    /// Human-readable name
    pub name: String,
    /// RPC URL
    pub rpc_url: String,
    /// Supported assets (symbol -> asset info)
    pub assets: HashMap<String, NearAsset>,
}

impl NearConfig {
    /// Create mainnet configuration
    pub fn mainnet() -> Self {
        let mut assets = HashMap::new();

        // Common mainnet NEP-141 tokens
        assets.insert(
            "USDC".to_string(),
            NearAsset {
                symbol: "USDC".to_string(),
                contract: "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1"
                    .to_string(),
                decimals: 6,
            },
        );

        assets.insert(
            "USDT".to_string(),
            NearAsset {
                symbol: "USDT".to_string(),
                contract: "usdt.tether-token.near".to_string(),
                decimals: 6,
            },
        );

        assets.insert(
            "NEAR".to_string(),
            NearAsset {
                symbol: "NEAR".to_string(),
                contract: "wrap.near".to_string(), // Wrapped NEAR
                decimals: 24,
            },
        );

        Self {
            chain_id: "near:mainnet".to_string(),
            name: "NEAR Mainnet".to_string(),
            rpc_url: "https://rpc.mainnet.near.org".to_string(),
            assets,
        }
    }

    /// Create testnet configuration
    pub fn testnet() -> Self {
        let mut assets = HashMap::new();

        // Testnet fake tokens
        assets.insert(
            "USDC".to_string(),
            NearAsset {
                symbol: "USDC".to_string(),
                contract: "usdc.fakes.testnet".to_string(),
                decimals: 6,
            },
        );

        assets.insert(
            "USDT".to_string(),
            NearAsset {
                symbol: "USDT".to_string(),
                contract: "usdt.fakes.testnet".to_string(),
                decimals: 6,
            },
        );

        Self {
            chain_id: "near:testnet".to_string(),
            name: "NEAR Testnet".to_string(),
            rpc_url: "https://rpc.testnet.near.org".to_string(),
            assets,
        }
    }
}

/// NEAR chain handler for external wallet deposits
pub struct NearExternalHandler {
    config: NearConfig,
    signer: Arc<near_crypto::InMemorySigner>,
    rpc_client: JsonRpcClient,
}

impl NearExternalHandler {
    /// Get token contract address for an asset
    pub fn get_token_contract(&self, asset: &str) -> Option<String> {
        self.config.assets.get(asset).map(|a| a.contract.clone())
    }

    /// Create a new NEAR external handler
    pub fn new(
        config: NearConfig,
        account_id: AccountId,
        signer_key: SecretKey,
    ) -> Result<Self, String> {
        let signer = near_crypto::InMemorySigner::from_secret_key(account_id.clone(), signer_key);
        let rpc_client = JsonRpcClient::connect(&config.rpc_url);

        info!(
            chain = %config.name,
            source_account = %account_id,
            "Initialized NEAR external handler"
        );

        Ok(Self {
            config,
            signer: Arc::new(signer),
            rpc_client,
        })
    }

    /// Parse amount string to base units
    fn parse_amount(&self, amount_str: &str, decimals: u8) -> Result<u128, String> {
        let parts: Vec<&str> = amount_str.split('.').collect();
        let (whole, frac) = match parts.len() {
            1 => (parts[0], ""),
            2 => (parts[0], parts[1]),
            _ => return Err("Invalid amount format".to_string()),
        };

        let whole_num: u128 = whole
            .parse()
            .map_err(|e| format!("Invalid whole part: {}", e))?;

        let frac_padded = format!("{:0<width$}", frac, width = decimals as usize);
        let frac_trimmed = &frac_padded[..decimals as usize];
        let frac_num: u128 = frac_trimmed
            .parse()
            .map_err(|e| format!("Invalid fractional part: {}", e))?;

        let multiplier = 10u128.pow(decimals as u32);
        whole_num
            .checked_mul(multiplier)
            .ok_or_else(|| "Overflow".to_string())?
            .checked_add(frac_num)
            .ok_or_else(|| "Overflow".to_string())
    }

    /// Register an account with a NEP-141 token contract
    async fn register_account_if_needed(
        &self,
        token_contract: &AccountId,
        account_id: &AccountId,
    ) -> Result<(), ExternalChainError> {
        // Check if account is already registered by calling storage_balance_of
        let query_request = methods::query::RpcQueryRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
            request: near_primitives::views::QueryRequest::CallFunction {
                account_id: token_contract.clone(),
                method_name: "storage_balance_of".to_string(),
                args: serde_json::json!({
                    "account_id": account_id.to_string()
                })
                .to_string()
                .into_bytes()
                .into(),
            },
        };

        let response = self.rpc_client.call(query_request).await.map_err(|e| {
            ExternalChainError::RpcConnectionFailed(format!("Failed to check registration: {}", e))
        })?;

        if let QueryResponseKind::CallResult(result) = response.kind {
            if !result.result.is_empty() {
                let balance: Option<serde_json::Value> =
                    serde_json::from_slice(&result.result).ok();
                if balance.is_some() && !balance.unwrap().is_null() {
                    info!(account = %account_id, "Account already registered");
                    return Ok(());
                }
            }
        }

        info!(account = %account_id, token = %token_contract, "Registering account with token contract");

        let storage_deposit_action = Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "storage_deposit".to_string(),
            args: serde_json::json!({
                "account_id": account_id.to_string(),
                "registration_only": true,
            })
            .to_string()
            .into_bytes(),
            gas: 30_000_000_000_000,                // 30 TGas
            deposit: 1_250_000_000_000_000_000_000, // 0.00125 NEAR
        }));

        self.execute_transaction(token_contract, vec![storage_deposit_action])
            .await?;

        info!(account = %account_id, "Account registered successfully");
        Ok(())
    }

    /// Execute a transaction with given actions
    async fn execute_transaction(
        &self,
        receiver_id: &AccountId,
        actions: Vec<Action>,
    ) -> Result<String, ExternalChainError> {
        // Get access key
        let access_key_query = methods::query::RpcQueryRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
            request: near_primitives::views::QueryRequest::ViewAccessKey {
                account_id: self.signer.account_id.clone(),
                public_key: self.signer.public_key.clone(),
            },
        };

        let access_key_response = self.rpc_client.call(access_key_query).await.map_err(|e| {
            ExternalChainError::RpcConnectionFailed(format!("Failed to get access key: {}", e))
        })?;

        let nonce = match access_key_response.kind {
            QueryResponseKind::AccessKey(access_key) => access_key.nonce + 1,
            _ => {
                return Err(ExternalChainError::RpcConnectionFailed(
                    "Unexpected query response".to_string(),
                ))
            }
        };

        // Get block hash
        let block_query = methods::block::RpcBlockRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
        };

        let block = self.rpc_client.call(block_query).await.map_err(|e| {
            ExternalChainError::RpcConnectionFailed(format!("Failed to get block: {}", e))
        })?;

        let transaction = Transaction::V0(TransactionV0 {
            signer_id: self.signer.account_id.clone(),
            public_key: self.signer.public_key.clone(),
            nonce,
            receiver_id: receiver_id.clone(),
            block_hash: block.header.hash,
            actions,
        });

        let (hash, _) = transaction.get_hash_and_size();
        let signature = self.signer.sign(hash.as_ref());
        let signed_transaction = SignedTransaction::new(signature, transaction);

        let tx_request =
            methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest { signed_transaction };

        let result = self.rpc_client.call(tx_request).await.map_err(|e| {
            error!("NEAR transaction failed: {}", e);
            ExternalChainError::TransactionFailed(format!("Transaction failed: {}", e))
        })?;

        if let near_primitives::views::FinalExecutionStatus::Failure(failure) = result.status {
            return Err(ExternalChainError::TransactionFailed(format!(
                "Transaction failed: {:?}",
                failure
            )));
        }

        Ok(result.transaction.hash.to_string())
    }

    /// Execute NEP-141 ft_transfer for direct transfers
    async fn ft_transfer(
        &self,
        token_contract: &AccountId,
        receiver_id: &AccountId,
        amount: u128,
    ) -> Result<String, ExternalChainError> {
        info!(
            token = %token_contract,
            receiver = %receiver_id,
            amount = %amount,
            "Executing NEAR ft_transfer"
        );

        self.register_account_if_needed(token_contract, receiver_id)
            .await?;

        let action = Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "ft_transfer".to_string(),
            args: serde_json::json!({
                "receiver_id": receiver_id.to_string(),
                "amount": amount.to_string(),
            })
            .to_string()
            .into_bytes(),
            gas: 30_000_000_000_000, // 30 TGas
            deposit: 1,              // 1 yoctoNEAR
        }));

        let tx_hash = self
            .execute_transaction(token_contract, vec![action])
            .await?;

        info!(
            tx_hash = %tx_hash,
            "NEAR transfer completed successfully"
        );

        Ok(tx_hash)
    }

    /// Execute NEP-141 ft_transfer_call to bridge deposit address
    async fn ft_transfer_call(
        &self,
        token_contract: &AccountId,
        receiver_id: &AccountId,
        amount: u128,
        memo: Option<&str>,
    ) -> Result<String, ExternalChainError> {
        info!(
            token = %token_contract,
            receiver = %receiver_id,
            amount = %amount,
            memo = ?memo,
            "Executing NEAR ft_transfer_call"
        );

        self.register_account_if_needed(token_contract, receiver_id)
            .await?;

        let msg = memo.unwrap_or("");

        let action = Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "ft_transfer_call".to_string(),
            args: serde_json::json!({
                "receiver_id": receiver_id.to_string(),
                "amount": amount.to_string(),
                "msg": msg,
            })
            .to_string()
            .into_bytes(),
            gas: 100_000_000_000_000, // 100 TGas
            deposit: 1,               // 1 yoctoNEAR
        }));

        let tx_hash = self
            .execute_transaction(token_contract, vec![action])
            .await?;

        info!(
            tx_hash = %tx_hash,
            "NEAR transfer completed successfully"
        );

        Ok(tx_hash)
    }
}

#[async_trait]
impl ExternalChainHandler for NearExternalHandler {
    fn chain_id(&self) -> &str {
        &self.config.chain_id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn supports_token(&self, asset: &str) -> bool {
        self.config.assets.contains_key(asset)
    }

    async fn transfer_tokens(
        &self,
        to_address: &str,
        asset: &str,
        amount: &str,
        memo: Option<&str>,
    ) -> Result<TransferResult, ExternalChainError> {
        info!(
            chain = %self.config.name,
            to = %to_address,
            asset = %asset,
            amount = %amount,
            memo = ?memo,
            "Initiating NEAR transfer"
        );

        // Get asset configuration
        let near_asset =
            self.config
                .assets
                .get(asset)
                .ok_or_else(|| ExternalChainError::TokenNotSupported {
                    asset: asset.to_string(),
                    chain: self.config.chain_id.clone(),
                })?;

        // Parse token contract
        let token_contract: AccountId = near_asset.contract.parse().map_err(|_| {
            ExternalChainError::InvalidAddress(format!(
                "Invalid token contract: {}",
                near_asset.contract
            ))
        })?;

        // Parse destination address
        let receiver_id: AccountId = to_address.parse().map_err(|_| {
            ExternalChainError::InvalidAddress(format!("Invalid NEAR address: {}", to_address))
        })?;

        // Parse amount
        let amount_base_units = self
            .parse_amount(amount, near_asset.decimals)
            .map_err(ExternalChainError::InvalidAmount)?;

        let tx_hash = if memo.is_none() || memo == Some("") {
            self.ft_transfer(&token_contract, &receiver_id, amount_base_units)
                .await?
        } else {
            self.ft_transfer_call(&token_contract, &receiver_id, amount_base_units, memo)
                .await?
        };

        Ok(TransferResult {
            tx_hash,
            confirmed: true, // NEAR broadcast_tx_commit waits for confirmation
        })
    }
}

/// Create NEAR external handler from environment variables
///
/// Required:
/// - `NEAR_ACCOUNT`: Account ID for NEAR wallet (deposits/withdrawals)
/// - `NEAR_KEY`: Secret key for NEAR wallet (ed25519:...)
///
/// Optional:
/// - `NEAR_RPC_URL`: Custom RPC URL (overrides default)
pub fn near_handler_from_env() -> Option<Box<dyn ExternalChainHandler>> {
    let mut config = NearConfig::mainnet();

    if let Ok(rpc_url) = std::env::var("NEAR_RPC_URL") {
        config.rpc_url = rpc_url;
    }

    match (std::env::var("NEAR_ACCOUNT"), std::env::var("NEAR_KEY")) {
        (Ok(account_str), Ok(key_str)) => {
            let account_id = match AccountId::from_str(&account_str) {
                Ok(id) => id,
                Err(e) => {
                    error!("Invalid NEAR_ACCOUNT '{}': {}", account_str, e);
                    return None;
                }
            };

            let signer_key = match SecretKey::from_str(&key_str) {
                Ok(key) => key,
                Err(e) => {
                    error!("Invalid NEAR_KEY: {}", e);
                    return None;
                }
            };

            match NearExternalHandler::new(config.clone(), account_id, signer_key) {
                Ok(handler) => {
                    info!(
                        chain_id = %handler.config.chain_id,
                        rpc_url = %handler.config.rpc_url,
                        account = %handler.signer.account_id,
                        "Configured NEAR external handler"
                    );
                    Some(Box::new(handler))
                }
                Err(e) => {
                    error!("Failed to create NEAR external handler: {}", e);
                    None
                }
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_near_config_mainnet() {
        let config = NearConfig::mainnet();
        assert_eq!(config.chain_id, "near:mainnet");
        assert!(config.assets.contains_key("USDC"));
        assert!(config.assets.contains_key("USDT"));
        assert_eq!(config.rpc_url, "https://rpc.mainnet.near.org");
    }

    #[test]
    fn test_near_config_testnet() {
        let config = NearConfig::testnet();
        assert_eq!(config.chain_id, "near:testnet");
        assert!(config.assets.contains_key("USDC"));
        assert_eq!(config.rpc_url, "https://rpc.testnet.near.org");
    }

    #[test]
    fn test_near_asset_mainnet_usdc() {
        let config = NearConfig::mainnet();
        let usdc = config.assets.get("USDC").unwrap();

        assert_eq!(usdc.symbol, "USDC");
        assert_eq!(usdc.decimals, 6);
        assert!(!usdc.contract.is_empty());
    }

    #[test]
    fn test_parse_amount() {
        let config = NearConfig::mainnet();
        let account_id = AccountId::from_str("test.near").unwrap();
        let signer_key = SecretKey::from_random(near_crypto::KeyType::ED25519);

        let handler = NearExternalHandler::new(config, account_id, signer_key).unwrap();

        // Test USDC (6 decimals)
        assert_eq!(handler.parse_amount("100", 6).unwrap(), 100_000_000);
        assert_eq!(handler.parse_amount("1.5", 6).unwrap(), 1_500_000);
        assert_eq!(handler.parse_amount("0.000001", 6).unwrap(), 1);

        // Test NEAR (24 decimals)
        assert_eq!(
            handler.parse_amount("1", 24).unwrap(),
            1_000_000_000_000_000_000_000_000
        );
    }

    #[test]
    fn test_parse_amount_invalid() {
        let config = NearConfig::mainnet();
        let account_id = AccountId::from_str("test.near").unwrap();
        let signer_key = SecretKey::from_random(near_crypto::KeyType::ED25519);

        let handler = NearExternalHandler::new(config, account_id, signer_key).unwrap();

        assert!(handler.parse_amount("abc", 6).is_err());
        assert!(handler.parse_amount("", 6).is_err());
        assert!(handler.parse_amount("1.2.3", 6).is_err());
    }

    #[test]
    fn test_near_handler_supports_token() {
        let config = NearConfig::mainnet();
        let account_id = AccountId::from_str("test.near").unwrap();
        let signer_key = SecretKey::from_random(near_crypto::KeyType::ED25519);

        let handler = NearExternalHandler::new(config, account_id, signer_key).unwrap();

        assert!(handler.supports_token("USDC"));
        assert!(handler.supports_token("USDT"));
        assert!(!handler.supports_token("BTC"));
    }

    #[test]
    fn test_near_handler_chain_id() {
        let config = NearConfig::mainnet();
        let account_id = AccountId::from_str("test.near").unwrap();
        let signer_key = SecretKey::from_random(near_crypto::KeyType::ED25519);

        let handler = NearExternalHandler::new(config, account_id, signer_key).unwrap();
        assert_eq!(handler.chain_id(), "near:mainnet");
    }
}
