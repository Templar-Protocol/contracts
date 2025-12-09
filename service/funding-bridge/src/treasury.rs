//! NEAR chain handler implementation
//!
//! Handles NEAR treasury operations including NEP-141 token transfers.

use near_crypto::SecretKey;
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    transaction::{Action, FunctionCallAction, SignedTransaction, Transaction, TransactionV0},
    types::AccountId,
};
use std::sync::Arc;
use tracing::{debug, info};

use crate::error::{ChainError, ChainResult};

/// NEAR chain handler
pub struct NearHandler {
    treasury_account: AccountId,
    signer: Arc<near_crypto::InMemorySigner>,
    rpc_client: JsonRpcClient,
    enabled: bool,
    dry_run: bool,
}

impl NearHandler {
    /// Create new NEAR handler
    pub fn new(
        treasury_account: AccountId,
        signer_key: SecretKey,
        rpc_url: String,
        dry_run: bool,
    ) -> Self {
        let signer =
            near_crypto::InMemorySigner::from_secret_key(treasury_account.clone(), signer_key);

        let rpc_client = JsonRpcClient::connect(&rpc_url);

        Self {
            treasury_account,
            signer: Arc::new(signer),
            rpc_client,
            enabled: true,
            dry_run,
        }
    }

    /// Transfer NEP-141 tokens
    async fn ft_transfer(
        &self,
        token_contract: &AccountId,
        receiver_id: &AccountId,
        amount: u128,
    ) -> ChainResult<String> {
        if self.dry_run {
            info!(
                token = %token_contract,
                receiver = %receiver_id,
                amount = %amount,
                "DRY RUN: Would transfer tokens"
            );
            return Ok(format!("dry-run-tx-{}", amount));
        }

        debug!(
            token = %token_contract,
            receiver = %receiver_id,
            amount = %amount,
            "Executing ft_transfer"
        );

        // Create ft_transfer action
        let action = Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "ft_transfer".to_string(),
            args: serde_json::json!({
                "receiver_id": receiver_id.to_string(),
                "amount": amount.to_string(),
            })
            .to_string()
            .into_bytes(),
            gas: 50_000_000_000_000, // 50 TGas
            deposit: 1,              // 1 yoctoNEAR for security
        }));

        // Get access key
        let access_key_query = methods::query::RpcQueryRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
            request: near_primitives::views::QueryRequest::ViewAccessKey {
                account_id: self.treasury_account.clone(),
                public_key: self.signer.public_key.clone(),
            },
        };

        let access_key_response = self
            .rpc_client
            .call(access_key_query)
            .await
            .map_err(|e| ChainError::RpcError(format!("Failed to get access key: {}", e)))?;

        let nonce = match access_key_response.kind {
            QueryResponseKind::AccessKey(access_key) => access_key.nonce + 1,
            _ => {
                return Err(ChainError::RpcError(
                    "Unexpected query response".to_string(),
                ))
            }
        };

        // Get block hash
        let block_query = methods::block::RpcBlockRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
        };

        let block = self
            .rpc_client
            .call(block_query)
            .await
            .map_err(|e| ChainError::RpcError(format!("Failed to get block: {}", e)))?;

        // Create and sign transaction
        let transaction = Transaction::V0(TransactionV0 {
            signer_id: self.treasury_account.clone(),
            public_key: self.signer.public_key.clone(),
            nonce,
            receiver_id: token_contract.clone(),
            block_hash: block.header.hash,
            actions: vec![action],
        });

        // Sign transaction
        let (hash, _) = transaction.get_hash_and_size();
        let signature = self.signer.sign(hash.as_ref());
        let signed_transaction = SignedTransaction::new(signature, transaction);

        // Send transaction
        let tx_request =
            methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest { signed_transaction };

        let result = self
            .rpc_client
            .call(tx_request)
            .await
            .map_err(|e| ChainError::TransactionFailed(format!("Transaction failed: {}", e)))?;

        // Check if transaction succeeded
        if let near_primitives::views::FinalExecutionStatus::Failure(failure) = result.status {
            return Err(ChainError::TransactionFailed(format!(
                "Transaction failed: {:?}",
                failure
            )));
        }

        Ok(result.transaction.hash.to_string())
    }

    /// Query NEP-141 token balance
    async fn ft_balance_of(
        &self,
        token_contract: &AccountId,
        account_id: &AccountId,
    ) -> ChainResult<u128> {
        debug!(
            token = %token_contract,
            account = %account_id,
            "Querying ft_balance_of"
        );

        let query_request = methods::query::RpcQueryRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
            request: near_primitives::views::QueryRequest::CallFunction {
                account_id: token_contract.clone(),
                method_name: "ft_balance_of".to_string(),
                args: serde_json::json!({
                    "account_id": account_id.to_string()
                })
                .to_string()
                .into_bytes()
                .into(),
            },
        };

        let response = self
            .rpc_client
            .call(query_request)
            .await
            .map_err(|e| ChainError::BalanceQueryFailed(format!("RPC error: {}", e)))?;

        match response.kind {
            QueryResponseKind::CallResult(result) => {
                let balance_str: String = serde_json::from_slice(&result.result)
                    .map_err(|e| ChainError::BalanceQueryFailed(format!("Parse error: {}", e)))?;

                balance_str
                    .parse()
                    .map_err(|e| ChainError::BalanceQueryFailed(format!("Invalid balance: {}", e)))
            }
            _ => Err(ChainError::BalanceQueryFailed(
                "Unexpected query response".to_string(),
            )),
        }
    }

    /// Get token contract ID for asset
    fn get_token_contract(&self, asset: &str) -> ChainResult<AccountId> {
        let contract_str = if asset.contains('.') || asset.starts_with("dev-") || asset.len() == 64
        {
            // Full contract ID provided (account ID with dots, dev account, or 64-char hash)
            // Use as-is - already in correct format
            asset.to_string()
        } else {
            // Asset symbol - convert to contract ID (lowercase required)
            let asset_lower = asset.to_lowercase();
            if self.rpc_client.server_addr().contains("testnet") {
                format!("{}.fakes.testnet", asset_lower)
            } else {
                format!("{}.near", asset_lower)
            }
        };

        contract_str
            .parse()
            .map_err(|_| ChainError::InvalidAddress(format!("Invalid asset: {}", asset)))
    }

    /// Execute intents on intents.near contract
    ///
    /// This is used for cross-chain withdrawals via NEAR Intents
    pub async fn execute_intents(
        &self,
        args: &crate::intents::ExecuteIntentsArgs,
    ) -> ChainResult<String> {
        if self.dry_run {
            info!(
                intents_count = args.signed.len(),
                "DRY RUN: Would execute intents on intents.near"
            );
            return Ok(format!("dry-run-intent-tx-{}", args.signed.len()));
        }

        debug!(
            intents_count = args.signed.len(),
            "Executing intents on intents.near"
        );

        let intents_contract: AccountId = crate::intents::INTENTS_CONTRACT
            .parse()
            .map_err(|_| ChainError::InvalidAddress("Invalid intents.near".to_string()))?;

        // Create execute_intents action
        let args_json = serde_json::to_vec(args).map_err(|e| {
            ChainError::TransactionFailed(format!("Failed to serialize args: {}", e))
        })?;

        let action = Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "execute_intents".to_string(),
            args: args_json,
            gas: 100_000_000_000_000, // 100 TGas for intent execution
            deposit: 0,               // No deposit required
        }));

        // Get access key
        let access_key_query = methods::query::RpcQueryRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
            request: near_primitives::views::QueryRequest::ViewAccessKey {
                account_id: self.treasury_account.clone(),
                public_key: self.signer.public_key.clone(),
            },
        };

        let access_key_response = self
            .rpc_client
            .call(access_key_query)
            .await
            .map_err(|e| ChainError::RpcError(format!("Failed to get access key: {}", e)))?;

        let nonce = match access_key_response.kind {
            QueryResponseKind::AccessKey(access_key) => access_key.nonce + 1,
            _ => {
                return Err(ChainError::RpcError(
                    "Unexpected query response".to_string(),
                ))
            }
        };

        // Get block hash
        let block_query = methods::block::RpcBlockRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
        };

        let block = self
            .rpc_client
            .call(block_query)
            .await
            .map_err(|e| ChainError::RpcError(format!("Failed to get block: {}", e)))?;

        // Create and sign transaction
        let transaction = Transaction::V0(TransactionV0 {
            signer_id: self.treasury_account.clone(),
            public_key: self.signer.public_key.clone(),
            nonce,
            receiver_id: intents_contract,
            block_hash: block.header.hash,
            actions: vec![action],
        });

        // Sign transaction
        let (hash, _) = transaction.get_hash_and_size();
        let signature = self.signer.sign(hash.as_ref());
        let signed_transaction = SignedTransaction::new(signature, transaction);

        // Send transaction
        let tx_request =
            methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest { signed_transaction };

        let result = self.rpc_client.call(tx_request).await.map_err(|e| {
            ChainError::TransactionFailed(format!("Intent execution failed: {}", e))
        })?;

        // Check if transaction succeeded
        if let near_primitives::views::FinalExecutionStatus::Failure(failure) = result.status {
            return Err(ChainError::TransactionFailed(format!(
                "Intent execution failed: {:?}",
                failure
            )));
        }

        debug!(
            tx_hash = %result.transaction.hash,
            "Intent execution completed"
        );

        Ok(result.transaction.hash.to_string())
    }

    /// Get the signer key for intent signing
    pub fn signer_key(&self) -> &SecretKey {
        &self.signer.secret_key
    }

    /// Get the treasury account ID
    pub fn treasury_account(&self) -> &AccountId {
        &self.treasury_account
    }

    /// Get available balance for asset
    ///
    /// # Arguments
    /// * `asset` - Asset identifier (e.g. "usdc", "usdt")
    ///
    /// # Returns
    /// Balance in smallest units
    pub async fn get_balance(&self, asset: &str) -> ChainResult<u128> {
        let token_contract = self.get_token_contract(asset)?;
        self.ft_balance_of(&token_contract, &self.treasury_account)
            .await
    }

    /// Send tokens to address
    ///
    /// For NEAR: Direct ft_transfer to destination
    ///
    /// # Arguments
    /// * `to_address` - Destination NEAR account ID
    /// * `asset` - Asset identifier
    /// * `amount` - Amount in smallest units
    ///
    /// # Returns
    /// Transaction hash
    pub async fn send_tokens(
        &self,
        to_address: &str,
        asset: &str,
        amount: u128,
    ) -> ChainResult<String> {
        let token_contract = self.get_token_contract(asset)?;
        let receiver_id: AccountId = to_address
            .parse()
            .map_err(|_| ChainError::InvalidAddress(to_address.to_string()))?;

        self.ft_transfer(&token_contract, &receiver_id, amount)
            .await
    }

    /// Check if handler is enabled
    pub fn is_available(&self) -> bool {
        self.enabled
    }

    /// Get chain identifier
    pub fn chain_name(&self) -> &str {
        "near"
    }

    /// Register storage on NEP-245 multi-token contract
    ///
    /// Required before an account can receive NEP-245 tokens
    ///
    /// # Arguments
    /// * `token_contract` - NEP-245 contract account ID
    /// * `account_id` - Account to register (None = self)
    /// * `storage_deposit` - Amount of NEAR to attach (e.g. 0.01 NEAR)
    ///
    /// # Returns
    /// Transaction hash
    pub async fn storage_deposit(
        &self,
        token_contract: &AccountId,
        account_id: Option<&AccountId>,
        storage_deposit: u128,
    ) -> ChainResult<String> {
        if self.dry_run {
            info!(
                contract = %token_contract,
                account = ?account_id,
                deposit = %storage_deposit,
                "DRY RUN: Would register storage"
            );
            return Ok(format!("dry-run-storage-{}", storage_deposit));
        }

        debug!(
            contract = %token_contract,
            account = ?account_id,
            deposit = %storage_deposit,
            "Registering storage on NEP-245 contract"
        );

        // Create storage_deposit action
        let args = if let Some(acc) = account_id {
            serde_json::json!({
                "account_id": acc.to_string(),
            })
        } else {
            serde_json::json!({})
        };

        let action = Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "storage_deposit".to_string(),
            args: args.to_string().into_bytes(),
            gas: 30_000_000_000_000, // 30 TGas
            deposit: storage_deposit,
        }));

        // Get access key
        let access_key_query = methods::query::RpcQueryRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
            request: near_primitives::views::QueryRequest::ViewAccessKey {
                account_id: self.treasury_account.clone(),
                public_key: self.signer.public_key.clone(),
            },
        };

        let access_key_response = self
            .rpc_client
            .call(access_key_query)
            .await
            .map_err(|e| ChainError::RpcError(format!("Failed to get access key: {}", e)))?;

        let nonce = match access_key_response.kind {
            QueryResponseKind::AccessKey(access_key) => access_key.nonce + 1,
            _ => {
                return Err(ChainError::RpcError(
                    "Unexpected query response".to_string(),
                ))
            }
        };

        // Get block hash
        let block_query = methods::block::RpcBlockRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
        };

        let block = self
            .rpc_client
            .call(block_query)
            .await
            .map_err(|e| ChainError::RpcError(format!("Failed to get block: {}", e)))?;

        // Create and sign transaction
        let transaction = Transaction::V0(TransactionV0 {
            signer_id: self.treasury_account.clone(),
            public_key: self.signer.public_key.clone(),
            nonce,
            receiver_id: token_contract.clone(),
            block_hash: block.header.hash,
            actions: vec![action],
        });

        // Sign transaction
        let (hash, _) = transaction.get_hash_and_size();
        let signature = self.signer.sign(hash.as_ref());
        let signed_transaction = SignedTransaction::new(signature, transaction);

        // Send transaction
        let tx_request =
            methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest { signed_transaction };

        let result =
            self.rpc_client.call(tx_request).await.map_err(|e| {
                ChainError::TransactionFailed(format!("Storage deposit failed: {}", e))
            })?;

        // Check if transaction succeeded
        if let near_primitives::views::FinalExecutionStatus::Failure(failure) = result.status {
            return Err(ChainError::TransactionFailed(format!(
                "Storage deposit failed: {:?}",
                failure
            )));
        }

        info!(
            tx_hash = %result.transaction.hash,
            contract = %token_contract,
            "Storage registered successfully"
        );

        Ok(result.transaction.hash.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_primitives::types::AccountId;
    use std::str::FromStr;

    fn create_test_handler() -> NearHandler {
        let treasury_account = AccountId::from_str("treasury.near").unwrap();
        let signer_key = SecretKey::from_random(near_crypto::KeyType::ED25519);

        NearHandler::new(
            treasury_account,
            signer_key,
            "https://rpc.testnet.near.org".to_string(),
            true, // dry_run = true for tests
        )
    }

    #[test]
    fn test_near_handler_creation() {
        let handler = create_test_handler();
        assert_eq!(handler.chain_name(), "near");
        assert!(handler.is_available());
    }

    #[test]
    fn test_get_token_contract_testnet() {
        let handler = create_test_handler();
        let contract = handler.get_token_contract("usdc").unwrap();
        assert_eq!(contract.to_string(), "usdc.fakes.testnet");
    }

    #[test]
    fn test_get_token_contract_uppercase() {
        let handler = create_test_handler();
        let contract = handler.get_token_contract("USDC").unwrap();
        assert_eq!(contract.to_string(), "usdc.fakes.testnet");
    }

    #[test]
    fn test_get_token_contract_mixed_case() {
        let handler = create_test_handler();
        let contract = handler.get_token_contract("UsDc").unwrap();
        assert_eq!(contract.to_string(), "usdc.fakes.testnet");
    }

    #[test]
    fn test_get_token_contract_invalid_asset() {
        let handler = create_test_handler();
        let result = handler.get_token_contract("");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_token_contract_mainnet() {
        let treasury_account = AccountId::from_str("treasury.near").unwrap();
        let signer_key = SecretKey::from_random(near_crypto::KeyType::ED25519);

        let handler = NearHandler::new(
            treasury_account,
            signer_key,
            "https://rpc.mainnet.near.org".to_string(),
            true,
        );

        let contract = handler.get_token_contract("USDC").unwrap();
        assert_eq!(contract.to_string(), "usdc.near");
    }

    #[tokio::test]
    async fn test_send_tokens_dry_run() {
        let handler = create_test_handler();

        let result = handler.send_tokens("receiver.near", "usdc", 1000000).await;

        assert!(result.is_ok());
        let tx_hash = result.unwrap();
        assert!(tx_hash.starts_with("dry-run-tx-"));
    }

    #[tokio::test]
    async fn test_send_tokens_uppercase_asset() {
        let handler = create_test_handler();

        let result = handler.send_tokens("receiver.near", "USDC", 1000000).await;

        assert!(result.is_ok());
        let tx_hash = result.unwrap();
        assert!(tx_hash.starts_with("dry-run-tx-"));
    }

    #[tokio::test]
    async fn test_send_tokens_invalid_receiver() {
        let handler = create_test_handler();

        let result = handler
            .send_tokens("invalid!!account", "usdc", 1000000)
            .await;

        assert!(result.is_err());
        match result {
            Err(ChainError::InvalidAddress(_)) => {}
            _ => panic!("Expected InvalidAddress error"),
        }
    }
}
