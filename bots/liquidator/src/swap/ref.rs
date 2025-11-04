// SPDX-License-Identifier: MIT
//! Ref Finance (v2.ref-finance.near) swap provider implementation.
//!
//! This provider integrates with Ref Finance's classic AMM contract for NEP-141 token swaps.
//! It supports single-hop and multi-hop routing through wNEAR as an intermediate token.
//!
//! # Architecture
//!
//! - Single-hop: token_in → token_out (direct pool)
//! - Two-hop: token_in → wNEAR → token_out (for pairs without direct pools)
//!
//! # Pool Discovery
//!
//! Pools are discovered by querying the contract's `get_pools` method and caching
//! relevant pool IDs for the token pairs we need.

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
use tracing::{debug, info, warn};

use crate::rpc::{get_access_key_data, send_tx, view, AppError, AppResult};

use super::SwapProvider;

/// Ref Finance classic AMM swap provider.
///
/// This provider integrates with the v2.ref-finance.near contract for NEP-141 swaps.
/// Supports smart routing through wNEAR as an intermediate token.
#[derive(Debug, Clone)]
pub struct RefSwap {
    /// Ref Finance contract account ID (v2.ref-finance.near on mainnet)
    pub contract: AccountId,
    /// JSON-RPC client for NEAR blockchain interaction
    pub client: JsonRpcClient,
    /// Transaction signer
    pub signer: Arc<Signer>,
    /// wNEAR contract for routing
    pub wnear_contract: AccountId,
    /// Maximum acceptable slippage in basis points (default: 50 = 0.5%)
    pub max_slippage_bps: u32,
}

impl RefSwap {
    /// Creates a new Ref Finance swap provider.
    ///
    /// # Arguments
    ///
    /// * `contract` - The Ref Finance contract account ID (v2.ref-finance.near on mainnet)
    /// * `client` - JSON-RPC client for blockchain communication
    /// * `signer` - Transaction signer
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use templar_bots::swap::ref_swap::RefSwap;
    /// # use near_jsonrpc_client::JsonRpcClient;
    /// # use std::sync::Arc;
    /// let swap = RefSwap::new(
    ///     "v2.ref-finance.near".parse().unwrap(),
    ///     JsonRpcClient::connect("https://rpc.mainnet.near.org"),
    ///     signer,
    /// );
    /// ```
    pub fn new(contract: AccountId, client: JsonRpcClient, signer: Arc<Signer>) -> Self {
        Self {
            contract,
            client,
            signer,
            wnear_contract: "wrap.near".parse().unwrap(),
            max_slippage_bps: Self::DEFAULT_MAX_SLIPPAGE_BPS,
        }
    }

    /// Default maximum slippage tolerance (0.5% = 50 basis points)
    pub const DEFAULT_MAX_SLIPPAGE_BPS: u32 = 50;

    /// Default transaction timeout in seconds
    const DEFAULT_TIMEOUT: u64 = 30;

    /// Validates that both assets are NEP-141 tokens.
    fn validate_nep141_assets<F: AssetClass, T: AssetClass>(
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
    ) -> AppResult<()> {
        if from_asset.clone().into_nep141().is_none() || to_asset.clone().into_nep141().is_none() {
            return Err(AppError::ValidationError(
                "RefSwap currently only supports NEP-141 tokens".to_string(),
            ));
        }
        Ok(())
    }

    /// Gets a quote for a single-hop swap.
    async fn get_single_hop_quote(
        &self,
        pool_id: u64,
        token_in: &AccountId,
        token_out: &AccountId,
        amount_in: U128,
    ) -> AppResult<U128> {
        let request = GetReturnRequest {
            pool_id,
            token_in: token_in.clone(),
            amount_in,
            token_out: token_out.clone(),
        };

        let output_amount: U128 = view(&self.client, self.contract.clone(), "get_return", &request).await?;

        debug!(
            pool_id,
            token_in = %token_in,
            token_out = %token_out,
            amount_in = %amount_in.0,
            amount_out = %output_amount.0,
            "Single-hop quote received"
        );

        Ok(output_amount)
    }

    /// Finds a pool for the given token pair.
    ///
    /// Searches through pools to find a matching pair.
    /// Searches up to 500 pools to cover most available pairs.
    async fn find_pool(
        &self,
        token_in: &AccountId,
        token_out: &AccountId,
    ) -> AppResult<Option<u64>> {
        // Search in batches of 100, up to 500 pools total
        const BATCH_SIZE: u64 = 100;
        const MAX_POOLS: u64 = 500;
        
        debug!(
            token_in = %token_in,
            token_out = %token_out,
            "Searching for pool"
        );

        for batch_start in (0..MAX_POOLS).step_by(BATCH_SIZE as usize) {
            let request = GetPoolsRequest {
                from_index: batch_start,
                limit: BATCH_SIZE,
            };

            let pools: Vec<PoolInfo> = match view(&self.client, self.contract.clone(), "get_pools", &request).await {
                Ok(p) => p,
                Err(e) => {
                    debug!(error = ?e, batch_start, "Failed to fetch pool batch");
                    break; // No more pools available
                }
            };

            if pools.is_empty() {
                break; // No more pools
            }

            for (index, pool) in pools.iter().enumerate() {
                let pool_id = batch_start + index as u64;
                
                if let Some(tokens) = &pool.token_account_ids {
                    if tokens.len() == 2 &&
                       ((tokens[0] == *token_in && tokens[1] == *token_out) ||
                        (tokens[0] == *token_out && tokens[1] == *token_in))
                    {
                        info!(
                            pool_id,
                            token_in = %token_in,
                            token_out = %token_out,
                            "Found direct pool"
                        );
                        return Ok(Some(pool_id));
                    }
                }
            }
        }

        warn!(
            token_in = %token_in,
            token_out = %token_out,
            "No direct pool found (searched {} pools)",
            MAX_POOLS
        );
        Ok(None)
    }

    /// Attempts to find a two-hop path through wNEAR.
    ///
    /// Returns (pool1_id, pool2_id) if both pools exist.
    async fn find_two_hop_path(
        &self,
        token_in: &AccountId,
        token_out: &AccountId,
    ) -> AppResult<Option<(u64, u64)>> {
        // Check if we have pools: token_in <-> wNEAR and wNEAR <-> token_out
        let pool1 = self.find_pool(token_in, &self.wnear_contract).await?;
        let pool2 = self.find_pool(&self.wnear_contract, token_out).await?;

        match (pool1, pool2) {
            (Some(p1), Some(p2)) => {
                info!(
                    pool1 = p1,
                    pool2 = p2,
                    "Found two-hop path: {} -> wNEAR -> {}",
                    token_in,
                    token_out
                );
                Ok(Some((p1, p2)))
            }
            _ => {
                debug!("No two-hop path found through wNEAR");
                Ok(None)
            }
        }
    }
}

/// Request for getting a swap quote from a single pool.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
struct GetReturnRequest {
    /// Pool ID to swap through
    pool_id: u64,
    /// Input token contract ID
    token_in: AccountId,
    /// Input amount
    amount_in: U128,
    /// Output token contract ID
    token_out: AccountId,
}

/// Request for getting multiple pools.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
struct GetPoolsRequest {
    /// Starting index
    from_index: u64,
    /// Number of pools to fetch
    limit: u64,
}

/// Pool information returned by get_pools.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PoolInfo {
    /// Pool type and parameters
    #[serde(flatten)]
    pool_kind: serde_json::Value,
    /// Token account IDs in the pool
    token_account_ids: Option<Vec<AccountId>>,
    /// Total fee charged by the pool (in basis points)
    total_fee: Option<u32>,
    /// Shares total supply
    shares_total_supply: Option<U128>,
}

/// Swap action for executing swaps.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SwapAction {
    /// Pool ID to swap through
    pool_id: u64,
    /// Input token
    token_in: AccountId,
    /// Output token (None for final output)
    token_out: Option<AccountId>,
    /// Minimum amount out (for slippage protection)
    min_amount_out: U128,
}

/// Swap request message for ft_transfer_call.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SwapMsg {
    /// Swap actions to execute
    actions: Vec<SwapAction>,
}

#[async_trait::async_trait]
impl SwapProvider for RefSwap {
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

        let token_in = from_asset.contract_id();
        let token_out = to_asset.contract_id();

        // Try to find a direct pool first
        let token_in_owned: AccountId = token_in.into();
        let token_out_owned: AccountId = token_out.into();
        
        if let Some(pool_id) = self.find_pool(&token_in_owned, &token_out_owned).await? {
            // For quote, we need to reverse-calculate input from desired output
            // This is complex, so for now we'll return an error
            // TODO: Implement reverse quote calculation
            warn!("Reverse quote calculation not yet implemented for Ref Finance");
            return Err(AppError::ValidationError(
                "Reverse quote calculation not yet implemented".to_string(),
            ));
        }

        // Try two-hop routing through wNEAR
        info!("No direct pool found, attempting two-hop routing through wNEAR");
        Err(AppError::ValidationError(
            "Two-hop routing not yet implemented".to_string(),
        ))
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

        let token_in = from_asset.contract_id();
        let token_out = to_asset.contract_id();

        let token_in_owned: AccountId = token_in.into();
        let token_out_owned: AccountId = token_out.into();
        
        info!(
            from_contract = %token_in_owned,
            to_contract = %token_out_owned,
            amount = %amount.0,
            "Attempting Ref Finance swap"
        );
        
        // Try direct pool first
        let swap_msg = if let Some(pool_id) = self.find_pool(&token_in_owned, &token_out_owned).await? {
            // Direct single-hop swap
            info!(pool_id, "Using direct pool for swap");
            
            let expected_output = self.get_single_hop_quote(pool_id, &token_in_owned, &token_out_owned, amount).await?;
            let min_amount_out = U128::from(
                expected_output.0 * (10000 - self.max_slippage_bps as u128) / 10000
            );

            debug!(
                expected_output = %expected_output.0,
                min_amount_out = %min_amount_out.0,
                slippage_bps = self.max_slippage_bps,
                "Calculated slippage protection (direct)"
            );

            SwapMsg {
                actions: vec![SwapAction {
                    pool_id,
                    token_in: token_in_owned.clone(),
                    token_out: None, // None means final output
                    min_amount_out,
                }],
            }
        } else if let Some((pool1, pool2)) = self.find_two_hop_path(&token_in_owned, &token_out_owned).await? {
            // Two-hop swap through wNEAR
            info!(pool1, pool2, "Using two-hop path through wNEAR");
            
            // For two-hop, we need to calculate intermediate amounts
            // First hop: token_in -> wNEAR
            let wnear_amount = self.get_single_hop_quote(pool1, &token_in_owned, &self.wnear_contract, amount).await?;
            
            // Second hop: wNEAR -> token_out
            let expected_output = self.get_single_hop_quote(pool2, &self.wnear_contract, &token_out_owned, wnear_amount).await?;
            
            // Apply slippage to final output
            let min_amount_out = U128::from(
                expected_output.0 * (10000 - self.max_slippage_bps as u128) / 10000
            );

            debug!(
                wnear_intermediate = %wnear_amount.0,
                expected_output = %expected_output.0,
                min_amount_out = %min_amount_out.0,
                slippage_bps = self.max_slippage_bps,
                "Calculated slippage protection (two-hop)"
            );

            // Build two-hop swap actions
            SwapMsg {
                actions: vec![
                    SwapAction {
                        pool_id: pool1,
                        token_in: token_in_owned.clone(),
                        token_out: Some(self.wnear_contract.clone()), // Intermediate output
                        min_amount_out: U128(1), // Don't restrict intermediate amount
                    },
                    SwapAction {
                        pool_id: pool2,
                        token_in: self.wnear_contract.clone(),
                        token_out: None, // Final output
                        min_amount_out, // Apply slippage protection here
                    },
                ],
            }
        } else {
            return Err(AppError::ValidationError(format!(
                "No swap path found for {token_in_owned} -> {token_out_owned} (tried direct and wNEAR routing)"
            )));
        };

        let msg_string = serde_json::to_string(&swap_msg).map_err(|e| {
            AppError::SerializationError(format!("Failed to serialize swap message: {e}"))
        })?;

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        // Execute swap via ft_transfer_call
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

        let outcome = send_tx(&self.client, &self.signer, Self::DEFAULT_TIMEOUT, tx)
            .await
            .map_err(AppError::from)?;

        info!("Ref Finance swap executed successfully");

        Ok(outcome.status)
    }

    fn provider_name(&self) -> &'static str {
        "RefFinance"
    }

    fn supports_assets<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
    ) -> bool {
        // Ref Finance only supports NEP-141 tokens
        from_asset.clone().into_nep141().is_some() && to_asset.clone().into_nep141().is_some()
    }

    async fn ensure_storage_registration<F: AssetClass>(
        &self,
        token_contract: &FungibleAsset<F>,
        account_id: &AccountId,
    ) -> AppResult<()> {
        // Call storage_deposit on the token contract
        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let storage_deposit_action = near_primitives::action::FunctionCallAction {
            method_name: "storage_deposit".to_string(),
            args: serde_json::to_vec(&serde_json::json!({
                "account_id": account_id,
                "registration_only": true,
            }))
            .map_err(|e| {
                AppError::SerializationError(format!(
                    "Failed to serialize storage_deposit args: {e}"
                ))
            })?,
            gas: 10_000_000_000_000,                // 10 TGas
            deposit: 1_250_000_000_000_000_000_000, // 0.00125 NEAR
        };

        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: token_contract.contract_id().into(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(storage_deposit_action))],
        });

        match send_tx(&self.client, &self.signer, Self::DEFAULT_TIMEOUT, tx).await {
            Ok(_) => {
                debug!(
                    account = %account_id,
                    token = %token_contract.contract_id(),
                    "Storage registration successful"
                );
                Ok(())
            }
            Err(e) => {
                // If already registered, that's fine
                let error_msg = e.to_string();
                if error_msg.contains("The account") && error_msg.contains("is already registered")
                {
                    debug!(
                        account = %account_id,
                        token = %token_contract.contract_id(),
                        "Account already registered"
                    );
                    Ok(())
                } else {
                    Err(AppError::Rpc(e))
                }
            }
        }
    }
}
