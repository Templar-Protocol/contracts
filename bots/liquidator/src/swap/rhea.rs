// SPDX-License-Identifier: MIT
//! Rhea Finance swap provider implementation.
//!
//! Rhea Finance is a concentrated liquidity DEX on NEAR Protocol, similar to
//! Uniswap V3. This module provides integration for executing swaps through
//! Rhea's DCL (Discrete Concentrated Liquidity) pools.
//!
//! # Pool Fee Tiers
//!
//! Rhea supports multiple fee tiers (in basis points):
//! - 100 (0.01%) - for very stable pairs
//! - 500 (0.05%) - for stable pairs
//! - 2000 (0.2%) - default, for most pairs
//! - 10000 (1%) - for exotic/volatile pairs

use std::sync::Arc;

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::Action,
    transaction::{Transaction, TransactionV0},
    views::FinalExecutionStatus,
};
use near_sdk::{json_types::U128, near, serde_json, AccountId};
use templar_common::asset::{AssetClass, FungibleAsset};
use tracing::debug;

use crate::rpc::{get_access_key_data, send_tx, view, AppError, AppResult};

use super::SwapProvider;

/// Rhea Finance swap provider.
///
/// This provider integrates with Rhea's concentrated liquidity DEX,
/// supporting NEP-141 fungible tokens.
///
/// # Limitations
///
/// - Currently only supports NEP-141 tokens (not NEP-245)
/// - Uses a fixed default fee tier of 0.2%
/// - Does not support multi-hop swaps
#[derive(Debug, Clone)]
pub struct RheaSwap {
    /// Rhea DEX contract account ID
    pub contract: AccountId,
    /// JSON-RPC client for NEAR blockchain interaction
    pub client: JsonRpcClient,
    /// Transaction signer
    pub signer: Arc<Signer>,
    /// Fee tier in basis points (default: 2000 = 0.2%)
    pub fee_tier: u32,
    /// Maximum acceptable slippage in basis points (default: 50 = 0.5%)
    pub max_slippage_bps: u32,
}

impl RheaSwap {
    /// Creates a new Rhea swap provider with default settings.
    ///
    /// # Arguments
    ///
    /// * `contract` - The Rhea DEX contract account ID
    /// * `client` - JSON-RPC client for blockchain communication
    /// * `signer` - Transaction signer
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use templar_bots::swap::rhea::RheaSwap;
    /// # use near_jsonrpc_client::JsonRpcClient;
    /// # use std::sync::Arc;
    /// let swap = RheaSwap::new(
    ///     "dclv2.ref-dev.testnet".parse().unwrap(),
    ///     JsonRpcClient::connect("https://rpc.testnet.near.org"),
    ///     signer,
    /// );
    /// ```
    pub fn new(contract: AccountId, client: JsonRpcClient, signer: Arc<Signer>) -> Self {
        Self {
            contract,
            client,
            signer,
            fee_tier: Self::DEFAULT_FEE_TIER,
            max_slippage_bps: Self::DEFAULT_MAX_SLIPPAGE_BPS,
        }
    }

    /// Creates a new Rhea swap provider with custom fee tier.
    ///
    /// # Arguments
    ///
    /// * `contract` - The Rhea DEX contract account ID
    /// * `client` - JSON-RPC client for blockchain communication
    /// * `signer` - Transaction signer
    /// * `fee_tier` - Fee tier in basis points (e.g., 2000 = 0.2%)
    pub fn with_fee_tier(
        contract: AccountId,
        client: JsonRpcClient,
        signer: Arc<Signer>,
        fee_tier: u32,
    ) -> Self {
        Self {
            contract,
            client,
            signer,
            fee_tier,
            max_slippage_bps: Self::DEFAULT_MAX_SLIPPAGE_BPS,
        }
    }

    /// Default fee tier for Rhea DCL pools (0.2% = 2000 basis points)
    pub const DEFAULT_FEE_TIER: u32 = 2000;

    /// Default maximum slippage tolerance (0.5% = 50 basis points)
    pub const DEFAULT_MAX_SLIPPAGE_BPS: u32 = 50;

    /// Default transaction timeout in seconds
    const DEFAULT_TIMEOUT: u64 = 30;

    /// Creates a pool identifier for Rhea's routing.
    ///
    /// Pool IDs follow the format: `input_token|output_token|fee_tier`
    fn create_pool_id<F: AssetClass, T: AssetClass>(
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        fee_tier: u32,
    ) -> String {
        format!(
            "{}|{}|{fee_tier}",
            from_asset.contract_id(),
            to_asset.contract_id()
        )
    }

    /// Validates that both assets are NEP-141 tokens.
    fn validate_nep141_assets<F: AssetClass, T: AssetClass>(
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
    ) -> AppResult<()> {
        if from_asset.clone().into_nep141().is_none() || to_asset.clone().into_nep141().is_none() {
            return Err(AppError::ValidationError(
                "RheaSwap currently only supports NEP-141 tokens".to_string(),
            ));
        }
        Ok(())
    }
}

/// Request for getting a swap quote.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
struct QuoteRequest {
    /// Pool identifiers to route through
    pool_ids: Vec<String>,
    /// Input token contract ID
    input_token: AccountId,
    /// Output token contract ID
    output_token: AccountId,
    /// Desired output amount
    output_amount: U128,
    /// Optional request tag for tracking
    tag: Option<String>,
}

impl QuoteRequest {
    /// Creates a new quote request.
    fn new<F: AssetClass, T: AssetClass>(
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
        fee_tier: u32,
    ) -> Self {
        let pool_id = RheaSwap::create_pool_id(from_asset, to_asset, fee_tier);

        Self {
            pool_ids: vec![pool_id],
            input_token: from_asset.contract_id().into(),
            output_token: to_asset.contract_id().into(),
            output_amount,
            tag: None,
        }
    }
}

/// Response from quote request.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
struct QuoteResponse {
    /// Required input amount
    amount: U128,
    /// Optional response tag
    tag: Option<String>,
}

/// Swap execution request message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
enum SwapRequestMsg {
    /// Swap to obtain a specific output amount
    SwapByOutput {
        /// Pool routing path
        pool_ids: Vec<String>,
        /// Desired output token
        output_token: AccountId,
        /// Desired output amount
        output_amount: U128,
    },
}

impl SwapRequestMsg {
    /// Creates a new swap request message.
    fn new<F: AssetClass, T: AssetClass>(
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
        fee_tier: u32,
    ) -> Self {
        let pool_id = RheaSwap::create_pool_id(from_asset, to_asset, fee_tier);

        Self::SwapByOutput {
            pool_ids: vec![pool_id],
            output_token: to_asset.contract_id().into(),
            output_amount,
        }
    }
}

#[async_trait::async_trait]
impl SwapProvider for RheaSwap {
    #[tracing::instrument(skip(self), level = "debug", fields(
        provider = %self.provider_name(),
        from = %from_asset.to_string(),
        to = %to_asset.to_string(),
        output_amount = %output_amount.0
    ))]
    async fn quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
    ) -> AppResult<U128> {
        Self::validate_nep141_assets(from_asset, to_asset)?;

        let response: QuoteResponse = view(
            &self.client,
            self.contract.clone(),
            "quote_by_output",
            &QuoteRequest::new(from_asset, to_asset, output_amount, self.fee_tier),
        )
        .await?;

        debug!(
            input_amount = %response.amount.0,
            "Rhea quote received"
        );

        Ok(response.amount)
    }

    #[tracing::instrument(skip(self), level = "info", fields(
        provider = %self.provider_name(),
        from = %from_asset.to_string(),
        to = %to_asset.to_string(),
        amount = %amount.0
    ))]
    async fn swap<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        amount: U128,
    ) -> AppResult<FinalExecutionStatus> {
        Self::validate_nep141_assets(from_asset, to_asset)?;

        let msg = SwapRequestMsg::new(from_asset, to_asset, amount, self.fee_tier);
        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let msg_string = serde_json::to_string(&msg).map_err(|e| {
            AppError::SerializationError(format!("Failed to serialize swap message: {e}"))
        })?;

        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: from_asset.contract_id().into(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(
                from_asset.transfer_call_action(&self.contract, amount.into(), &msg_string),
            ))],
        });

        let status = send_tx(&self.client, &self.signer, Self::DEFAULT_TIMEOUT, tx)
            .await
            .map_err(AppError::from)?;

        debug!("Rhea swap executed successfully");

        Ok(status)
    }

    fn provider_name(&self) -> &'static str {
        "RheaSwap"
    }

    fn supports_assets<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
    ) -> bool {
        // Rhea currently only supports NEP-141 tokens
        from_asset.clone().into_nep141().is_some() && to_asset.clone().into_nep141().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_crypto::{InMemorySigner, SecretKey};
    use templar_common::asset::BorrowAsset;

    #[test]
    #[allow(clippy::similar_names)]
    fn test_pool_id_creation() {
        let usdc: FungibleAsset<BorrowAsset> = "nep141:usdc.near".parse().unwrap();
        let usdt: FungibleAsset<BorrowAsset> = "nep141:usdt.near".parse().unwrap();

        let pool_id = RheaSwap::create_pool_id(&usdc, &usdt, 2000);
        assert_eq!(pool_id, "usdc.near|usdt.near|2000");
    }

    #[test]
    fn test_nep141_validation() {
        let nep141: FungibleAsset<BorrowAsset> = "nep141:token.near".parse().unwrap();
        let nep245: FungibleAsset<BorrowAsset> = "nep245:multi.near:token1".parse().unwrap();

        // Both NEP-141 should pass
        assert!(RheaSwap::validate_nep141_assets(&nep141, &nep141).is_ok());

        // NEP-245 should fail
        assert!(RheaSwap::validate_nep141_assets(&nep141, &nep245).is_err());
        assert!(RheaSwap::validate_nep141_assets(&nep245, &nep141).is_err());
    }

    #[test]
    fn test_rhea_swap_creation() {
        let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
        let signer_key = SecretKey::from_seed(near_crypto::KeyType::ED25519, "test");
        let signer = Arc::new(InMemorySigner::from_secret_key(
            "liquidator.testnet".parse().unwrap(),
            signer_key,
        ));

        let rhea = RheaSwap::new("dclv2.ref-dev.testnet".parse().unwrap(), client, signer);

        assert_eq!(rhea.provider_name(), "RheaSwap");
        assert_eq!(rhea.fee_tier, RheaSwap::DEFAULT_FEE_TIER);
    }

    #[test]
    fn test_supports_assets() {
        let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
        let signer_key = SecretKey::from_seed(near_crypto::KeyType::ED25519, "test");
        let signer = Arc::new(InMemorySigner::from_secret_key(
            "liquidator.testnet".parse().unwrap(),
            signer_key,
        ));

        let rhea = RheaSwap::new("dclv2.ref-dev.testnet".parse().unwrap(), client, signer);

        let nep141: FungibleAsset<BorrowAsset> = "nep141:token.near".parse().unwrap();
        let nep245: FungibleAsset<BorrowAsset> = "nep245:multi.near:token1".parse().unwrap();

        // Should support NEP-141 to NEP-141
        assert!(rhea.supports_assets(&nep141, &nep141));

        // Should not support NEP-245
        assert!(!rhea.supports_assets(&nep141, &nep245));
        assert!(!rhea.supports_assets(&nep245, &nep141));
    }
}
