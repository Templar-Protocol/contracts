//! Ref Finance swap provider for NEP-141 tokens.
//!
//! Integrates with Ref Finance AMM contract for token swaps with automatic routing
//! through wNEAR for pairs without direct pools.

use std::sync::Arc;

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::Action,
    transaction::{Transaction, TransactionV0},
    views::FinalExecutionStatus,
};
use near_sdk::{
    json_types::U128,
    serde::{Deserialize, Serialize},
    AccountId, Gas,
};
use templar_common::asset::{AssetClass, FungibleAsset, FungibleAssetAmount};

use crate::rpc::{get_access_key_data, send_tx, view, AppError, AppResult};

use super::SwapProvider;

/// Storage balance bounds from NEP-145
#[derive(Debug, Deserialize)]
struct StorageBalanceBounds {
    /// Minimum storage deposit required
    min: U128,
    /// Maximum storage deposit allowed (optional)
    #[allow(dead_code)]
    max: Option<U128>,
}

/// Ref/Rhea Finance swap provider for NEP-141 tokens.
#[derive(Debug, Clone)]
pub struct RefSwap {
    /// Ref Finance contract account ID
    pub contract: AccountId,
    /// JSON-RPC client
    pub client: JsonRpcClient,
    /// Transaction signer
    pub signer: Arc<Signer>,
    /// wNEAR contract for routing
    pub wnear_contract: AccountId,
    /// Maximum slippage in basis points
    pub max_slippage_bps: u32,
    /// Ref Finance indexer URL
    pub indexer_url: String,
}

impl RefSwap {
    /// Creates a new Ref Finance swap provider
    pub fn new(contract: AccountId, client: JsonRpcClient, signer: Arc<Signer>) -> Self {
        #[allow(clippy::expect_used)]
        Self {
            contract,
            client,
            signer,
            wnear_contract: "wrap.near".parse().expect("wrap.near is a valid AccountId"),
            max_slippage_bps: Self::DEFAULT_MAX_SLIPPAGE_BPS,
            indexer_url: "https://indexer.ref.finance".to_string(),
        }
    }

    /// Default slippage tolerance (0.5%)
    pub const DEFAULT_MAX_SLIPPAGE_BPS: u32 = 50;

    /// Default transaction timeout
    const DEFAULT_TIMEOUT: u64 = 30;

    /// Validates that both assets are NEP-141 tokens
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

    /// Finds the best pool for swapping between two tokens by querying the contract.
    /// Returns `pool_id` if found, otherwise None.
    async fn find_best_pool(
        &self,
        token_in: &AccountId,
        token_out: &AccountId,
    ) -> AppResult<Option<u64>> {
        #[derive(Deserialize)]
        struct PoolInfo {
            token_account_ids: Vec<AccountId>,
            shares_total_supply: String,
        }

        use near_jsonrpc_client::methods::query::RpcQueryRequest;
        use near_primitives::types::{BlockReference, Finality};
        use near_primitives::views::QueryRequest;

        // Search common pool ranges for direct pairs
        let search_ranges = vec![
            (0, 500),
            (500, 1500),
            (1500, 2500),
            (2500, 3500),
            (3500, 4500),
            (4500, 5500),
            (5500, 6700),
        ];

        for (start, end) in search_ranges {
            let batch_size = 100;
            let mut from_index = start;

            while from_index < end {
                let limit = std::cmp::min(batch_size, end - from_index);

                let args = near_sdk::serde_json::json!({
                    "from_index": from_index,
                    "limit": limit
                });

                let request = RpcQueryRequest {
                    block_reference: BlockReference::Finality(Finality::Final),
                    request: QueryRequest::CallFunction {
                        account_id: self.contract.clone(),
                        method_name: "get_pools".to_string(),
                        args: args.to_string().into_bytes().into(),
                    },
                };

                let response = self.client.call(request).await.map_err(|e| {
                    AppError::ValidationError(format!("Failed to query pools: {e}"))
                })?;

                let result = match response.kind {
                    near_jsonrpc_primitives::types::query::QueryResponseKind::CallResult(
                        result,
                    ) => result.result,
                    _ => {
                        return Err(AppError::ValidationError(
                            "Unexpected response type".to_string(),
                        ))
                    }
                };

                let pools: Vec<PoolInfo> =
                    near_sdk::serde_json::from_slice(&result).map_err(|e| {
                        AppError::SerializationError(format!("Failed to parse pools: {e}"))
                    })?;

                if pools.is_empty() {
                    break;
                }

                // Search for matching pool in this batch
                for (idx, pool) in pools.iter().enumerate() {
                    let pool_id = from_index + idx as u64;
                    if pool.token_account_ids.len() == 2
                        && ((pool.token_account_ids[0] == *token_in
                            && pool.token_account_ids[1] == *token_out)
                            || (pool.token_account_ids[0] == *token_out
                                && pool.token_account_ids[1] == *token_in))
                        && pool.shares_total_supply != "0"
                    {
                        tracing::info!(pool_id, "Found direct pool");
                        return Ok(Some(pool_id));
                    }
                }

                from_index += limit;
            }
        }

        tracing::info!(
            token_in = %token_in,
            token_out = %token_out,
            "No direct pool found after scanning common ranges"
        );
        Ok(None)
    }

    /// Finds a two-hop route through wNEAR by querying the contract.
    /// Returns (`pool1_id`, `pool2_id`) if found.
    #[allow(clippy::too_many_lines)]
    async fn find_two_hop_route(
        &self,
        token_in: &AccountId,
        token_out: &AccountId,
    ) -> AppResult<Option<(u64, u64)>> {
        #[derive(Deserialize)]
        struct PoolInfo {
            token_account_ids: Vec<AccountId>,
            shares_total_supply: String,
        }

        use near_jsonrpc_client::methods::query::RpcQueryRequest;
        use near_primitives::types::{BlockReference, Finality};
        use near_primitives::views::QueryRequest;

        let mut pool1_opt: Option<u64> = None;
        let mut pool2_opt: Option<u64> = None;

        // Search common pool ranges for liquid wNEAR pairs
        let search_ranges = vec![
            (0, 500),
            (500, 1500),
            (1500, 2500),
            (2500, 3500),
            (3500, 4500),
            (4500, 5500),
            (5500, 6700),
        ];

        for (start, end) in search_ranges {
            let batch_size = 100;
            let mut from_index = start;

            while from_index < end {
                let limit = std::cmp::min(batch_size, end - from_index);

                let args = near_sdk::serde_json::json!({
                    "from_index": from_index,
                    "limit": limit
                });

                let request = RpcQueryRequest {
                    block_reference: BlockReference::Finality(Finality::Final),
                    request: QueryRequest::CallFunction {
                        account_id: self.contract.clone(),
                        method_name: "get_pools".to_string(),
                        args: args.to_string().into_bytes().into(),
                    },
                };

                let response = self.client.call(request).await.map_err(|e| {
                    AppError::ValidationError(format!("Failed to query pools: {e}"))
                })?;

                let result = match response.kind {
                    near_jsonrpc_primitives::types::query::QueryResponseKind::CallResult(
                        result,
                    ) => result.result,
                    _ => {
                        return Err(AppError::ValidationError(
                            "Unexpected response type".to_string(),
                        ))
                    }
                };

                let pools: Vec<PoolInfo> =
                    near_sdk::serde_json::from_slice(&result).map_err(|e| {
                        AppError::SerializationError(format!("Failed to parse pools: {e}"))
                    })?;

                if pools.is_empty() {
                    break;
                }

                // Search for wNEAR routes in this batch
                for (idx, pool) in pools.iter().enumerate() {
                    let pool_id = from_index + idx as u64;

                    if pool.token_account_ids.len() == 2 && pool.shares_total_supply != "0" {
                        // Check for pool1: token_in -> wNEAR
                        if pool1_opt.is_none()
                            && ((pool.token_account_ids[0] == *token_in
                                && pool.token_account_ids[1] == self.wnear_contract)
                                || (pool.token_account_ids[0] == self.wnear_contract
                                    && pool.token_account_ids[1] == *token_in))
                        {
                            pool1_opt = Some(pool_id);
                            tracing::info!(pool_id, "Found pool1: {} -> wNEAR", token_in);
                        }

                        // Check for pool2: wNEAR -> token_out
                        if pool2_opt.is_none()
                            && ((pool.token_account_ids[0] == self.wnear_contract
                                && pool.token_account_ids[1] == *token_out)
                                || (pool.token_account_ids[0] == *token_out
                                    && pool.token_account_ids[1] == self.wnear_contract))
                        {
                            pool2_opt = Some(pool_id);
                            tracing::info!(pool_id, "Found pool2: wNEAR -> {}", token_out);
                        }

                        // If we found both pools, we're done
                        if let (Some(p1), Some(p2)) = (pool1_opt, pool2_opt) {
                            tracing::info!(
                                pool1 = p1,
                                pool2 = p2,
                                "Found two-hop route through wNEAR"
                            );
                            return Ok(Some((p1, p2)));
                        }
                    }
                }

                from_index += limit;
            }
        }

        tracing::info!(
            token_in = %token_in,
            token_out = %token_out,
            wnear = %self.wnear_contract,
            pool1_found = pool1_opt.is_some(),
            pool2_found = pool2_opt.is_some(),
            "No two-hop route found"
        );
        Ok(None)
    }
}

/// Swap action for Ref Finance swaps
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SwapAction {
    pool_id: u64,
    token_in: AccountId,
    token_out: AccountId,
    #[serde(skip_serializing_if = "Option::is_none")]
    amount_in: Option<String>,
    min_amount_out: String,
}

/// Swap request message
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SwapMsg {
    force: u8,
    actions: Vec<SwapAction>,
}

#[async_trait::async_trait]
impl SwapProvider for RefSwap {
    #[tracing::instrument(skip(self), level = "debug", fields(
        provider = %self.provider_name(),
        from = %from_asset.to_string(),
        to = %to_asset.to_string(),
        output_amount = %_output_amount
    ))]
    async fn quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        _output_amount: FungibleAssetAmount<T>,
    ) -> AppResult<FungibleAssetAmount<F>> {
        Self::validate_nep141_assets(from_asset, to_asset)?;

        Err(AppError::ValidationError(
            "Quote not supported - use direct swap instead".to_string(),
        ))
    }

    #[tracing::instrument(skip(self), level = "info", fields(
        provider = %self.provider_name(),
        from = %from_asset.to_string(),
        to = %to_asset.to_string(),
        amount = %amount
    ))]
    async fn swap<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        amount: FungibleAssetAmount<F>,
    ) -> AppResult<FinalExecutionStatus> {
        Self::validate_nep141_assets(from_asset, to_asset)?;

        let token_in = from_asset.contract_id();
        let token_out = to_asset.contract_id();

        let token_in_owned: AccountId = token_in.into();
        let token_out_owned: AccountId = token_out.into();

        tracing::info!(
            from_contract = %token_in_owned,
            to_contract = %token_out_owned,
            amount = %amount,
            "Attempting Ref Finance swap"
        );

        // Try to find direct pool first
        let pool_id_opt = self
            .find_best_pool(&token_in_owned, &token_out_owned)
            .await?;

        let (swap_msg, intermediate_token) = if let Some(pool_id) = pool_id_opt {
            // Direct swap
            let amount_u128 = u128::from(amount);
            let min_amount_out =
                U128::from(amount_u128 * (10000 - u128::from(self.max_slippage_bps)) / 10000);

            tracing::debug!(
                pool_id,
                min_amount_out = %min_amount_out.0,
                slippage_bps = self.max_slippage_bps,
                "Using direct pool"
            );

            let msg = SwapMsg {
                force: 0,
                actions: vec![SwapAction {
                    pool_id,
                    token_in: token_in_owned.clone(),
                    token_out: token_out_owned.clone(),
                    amount_in: None,
                    min_amount_out: min_amount_out.0.to_string(),
                }],
            };

            (msg, None)
        } else {
            // Try two-hop routing through wNEAR
            let (pool1, pool2) = self
                .find_two_hop_route(&token_in_owned, &token_out_owned)
                .await?
                .ok_or_else(|| {
                    AppError::ValidationError(format!(
                        "No swap path found for {token_in_owned} -> {token_out_owned}"
                    ))
                })?;

            let amount_u128 = u128::from(amount);
            let min_amount_out =
                U128::from(amount_u128 * (10000 - u128::from(self.max_slippage_bps)) / 10000);

            tracing::debug!(
                pool1,
                pool2,
                min_amount_out = %min_amount_out.0,
                slippage_bps = self.max_slippage_bps,
                "Using two-hop route through wNEAR"
            );

            let msg = SwapMsg {
                force: 0,
                actions: vec![
                    SwapAction {
                        pool_id: pool1,
                        token_in: token_in_owned.clone(),
                        token_out: self.wnear_contract.clone(),
                        amount_in: None,
                        min_amount_out: "1".to_string(),
                    },
                    SwapAction {
                        pool_id: pool2,
                        token_in: self.wnear_contract.clone(),
                        token_out: token_out_owned.clone(),
                        amount_in: None,
                        min_amount_out: min_amount_out.0.to_string(),
                    },
                ],
            };

            (msg, Some(self.wnear_contract.clone()))
        };

        let msg_string = near_sdk::serde_json::to_string(&swap_msg).map_err(|e| {
            AppError::SerializationError(format!("Failed to serialize swap message: {e}"))
        })?;

        // Register storage for output token and intermediate token if needed
        let our_account = self.signer.get_account_id();
        self.ensure_storage_registration(to_asset, &our_account)
            .await?;

        if let Some(intermediate) = intermediate_token {
            let wnear_asset: FungibleAsset<templar_common::asset::CollateralAsset> =
                FungibleAsset::nep141(intermediate);
            self.ensure_storage_registration(&wnear_asset, &our_account)
                .await?;
        }

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer, None).await?;

        // Execute swap via ft_transfer_call
        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: from_asset.contract_id().into(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(
                from_asset.transfer_call_action(&self.contract, amount, &msg_string),
            ))],
        });

        let outcome = send_tx(&self.client, &self.signer, Self::DEFAULT_TIMEOUT, tx).await?;

        tracing::info!("Ref Finance swap executed successfully");

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
        from_asset.clone().into_nep141().is_some() && to_asset.clone().into_nep141().is_some()
    }

    async fn ensure_storage_registration<F: AssetClass>(
        &self,
        token_contract: &FungibleAsset<F>,
        account_id: &AccountId,
    ) -> AppResult<()> {
        const MAX_REASONABLE_DEPOSIT: u128 = 100_000_000_000_000_000_000_000; // 0.1 NEAR

        // Query storage_balance_bounds to get minimum deposit required
        let bounds: StorageBalanceBounds = view(
            &self.client,
            token_contract.contract_id().into(),
            "storage_balance_bounds",
            near_sdk::serde_json::json!({}),
        )
        .await
        .map_err(|e| {
            tracing::debug!(?e, token = %token_contract.contract_id(), "Failed to query storage_balance_bounds");
            AppError::Rpc(e)
        })?;

        let min_deposit = bounds.min.0;

        // Validate minimum deposit is reasonable (less than 0.1 NEAR)
        if min_deposit > MAX_REASONABLE_DEPOSIT {
            return Err(AppError::ValidationError(format!(
                "Storage deposit minimum ({min_deposit} yoctoNEAR) exceeds reasonable limit ({MAX_REASONABLE_DEPOSIT} yoctoNEAR / 0.1 NEAR)"
            )));
        }

        #[allow(clippy::cast_precision_loss)]
        let min_deposit_near = min_deposit as f64 / 1e24;

        tracing::debug!(
            token = %token_contract.contract_id(),
            min_deposit_near = %min_deposit_near,
            "Using storage deposit minimum from contract"
        );

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer, None).await?;

        let storage_deposit_action = near_primitives::action::FunctionCallAction {
            method_name: "storage_deposit".to_string(),
            args: near_sdk::serde_json::to_vec(&near_sdk::serde_json::json!({
                "account_id": account_id,
                "registration_only": true,
            }))
            .map_err(|e| {
                AppError::SerializationError(format!(
                    "Failed to serialize storage_deposit args: {e}"
                ))
            })?,
            gas: Gas::from_tgas(10).as_gas(),
            deposit: min_deposit,
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
                tracing::debug!(
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
                    tracing::debug!(
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
