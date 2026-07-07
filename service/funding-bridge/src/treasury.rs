//! NEAR chain handler implementation
//!
//! Handles NEAR treasury operations including NEP-141 token transfers, driven
//! through the in-process gateway client (`templar-gateway-client`) rather than
//! hand-rolled RPC/signing/transaction plumbing.

use near_account_id::AccountId;
use near_api::SecretKey;
use near_sdk::NearToken;

use templar_common::SU128;
use templar_gateway_client::{Network, NetworkConfigBuilder, SigningClient};
use templar_gateway_methods_spec::{ft, intents::ExecuteIntents, storage};
use templar_gateway_types::{common::WriteOperationResult, OperationStatus};
use tracing::{debug, info};

use crate::error::{ChainError, ChainResult};
use crate::intents::INTENTS_CONTRACT;

/// NEAR chain handler
pub struct NearHandler {
    treasury_account: AccountId,
    /// Treasury key, retained for NEP-413 intent message signing (see
    /// [`NearHandler::treasury_key`]). All on-chain calls go through `client`.
    secret_key: SecretKey,
    client: SigningClient,
    /// Configured RPC URL, kept only to resolve asset symbols against the right
    /// network in [`NearHandler::get_token_contract`].
    rpc_url: String,
    enabled: bool,
    dry_run: bool,
}

impl NearHandler {
    /// Create new NEAR handler backed by the in-process gateway client.
    pub fn new(
        treasury_account: AccountId,
        signer_key: SecretKey,
        rpc_url: String,
        dry_run: bool,
    ) -> ChainResult<Self> {
        let network = if rpc_url.contains("testnet") {
            Network::Testnet
        } else {
            Network::Mainnet
        };
        let network_config = NetworkConfigBuilder::new(network)
            .rpc_url(Some(&rpc_url))
            .map_err(|e| ChainError::ConfigError(format!("invalid RPC URL: {e}")))?
            .build();

        let client =
            SigningClient::connect(network_config, treasury_account.clone(), signer_key.clone())
                .map_err(|e| {
                    ChainError::ConfigError(format!("failed to build gateway client: {e}"))
                })?;

        Ok(Self {
            treasury_account,
            secret_key: signer_key,
            client,
            rpc_url,
            enabled: true,
            dry_run,
        })
    }

    /// Map a gateway write result into the tx-hash string the routes expect,
    /// surfacing a non-success operation as a [`ChainError`].
    fn tx_result(result: WriteOperationResult) -> ChainResult<String> {
        match result.operation.status {
            OperationStatus::Succeeded => Ok(result
                .operation
                .latest_tx_hash()
                .map(|hash| hash.to_string())
                .unwrap_or_default()),
            other => Err(ChainError::TransactionFailed(format!(
                "operation did not succeed (status: {other:?})"
            ))),
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

        let result = self
            .client
            .execute(ft::Transfer {
                contract_id: token_contract.clone(),
                receiver_id: receiver_id.clone(),
                amount: SU128::from(amount),
                memo: None,
            })
            .await
            .map_err(|e| ChainError::TransactionFailed(format!("ft_transfer failed: {e}")))?;

        Self::tx_result(result)
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

        let result = self
            .client
            .read(ft::GetBalanceOf {
                contract_id: token_contract.clone(),
                account_id: account_id.clone(),
            })
            .await
            .map_err(|e| ChainError::BalanceQueryFailed(format!("ft_balance_of failed: {e}")))?;

        Ok(*result.balance)
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
            if self.rpc_url.contains("testnet") {
                format!("{}.fakes.testnet", asset_lower)
            } else {
                format!("{}.near", asset_lower)
            }
        };

        contract_str
            .parse()
            .map_err(|_| ChainError::InvalidAddress(format!("Invalid asset: {}", asset)))
    }

    /// Execute intents on the intents contract
    ///
    /// This is used for cross-chain withdrawals via NEAR Intents
    pub async fn execute_intents(
        &self,
        args: &crate::intents::ExecuteIntentsArgs,
    ) -> ChainResult<String> {
        if self.dry_run {
            info!(
                intents_count = args.signed.len(),
                "DRY RUN: Would execute intents on intents contract"
            );
            return Ok(format!("dry-run-intent-tx-{}", args.signed.len()));
        }

        debug!(
            intents_count = args.signed.len(),
            "Executing intents on intents contract"
        );

        let contract_id: AccountId = INTENTS_CONTRACT
            .parse()
            .map_err(|_| ChainError::InvalidAddress("Invalid intents contract".to_string()))?;

        let result = self
            .client
            .execute(ExecuteIntents {
                contract_id,
                signed: args.signed.clone(),
            })
            .await
            .map_err(|e| ChainError::TransactionFailed(format!("Intent execution failed: {e}")))?;

        Self::tx_result(result)
    }

    /// Get the treasury key for NEP-413 intent signing
    pub fn treasury_key(&self) -> SecretKey {
        self.secret_key.clone()
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

    /// Register storage on a NEP-141/245 token contract
    ///
    /// Required before an account can receive tokens on that contract.
    ///
    /// # Arguments
    /// * `token_contract` - token contract account ID
    /// * `account_id` - Account to register (None = self)
    /// * `storage_deposit` - Amount of NEAR to attach (e.g. 0.01 NEAR)
    ///
    /// # Returns
    /// Transaction hash
    pub async fn storage_deposit(
        &self,
        token_contract: &AccountId,
        account_id: Option<&AccountId>,
        storage_deposit: NearToken,
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
            "Registering storage on token contract"
        );

        let result = self
            .client
            .execute(storage::Deposit {
                contract_id: token_contract.clone(),
                beneficiary_id: account_id.cloned(),
                registration_only: false,
                deposit: storage_deposit,
            })
            .await
            .map_err(|e| ChainError::TransactionFailed(format!("Storage deposit failed: {e}")))?;

        Self::tx_result(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed, valid ED25519 test key (the gateway sandbox test key). Handlers
    /// built here are dry-run only, so the key is never used to sign anything.
    fn test_key() -> SecretKey {
        "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q"
            .parse()
            .unwrap()
    }

    fn create_test_handler() -> NearHandler {
        NearHandler::new(
            "treasury.near".parse().unwrap(),
            test_key(),
            "https://rpc.testnet.near.org".to_string(),
            true, // dry_run = true for tests
        )
        .unwrap()
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
        let handler = NearHandler::new(
            "treasury.near".parse().unwrap(),
            test_key(),
            "https://free.rpc.fastnear.com".to_string(),
            true,
        )
        .unwrap();

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
