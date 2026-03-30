//! Liquidation transaction executor module.
//!
//! Handles the creation and submission of liquidation transactions,
//! including inventory management and immediate collateral swapping.

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    hash::CryptoHash,
    transaction::{Transaction, TransactionV0},
};
use near_sdk::{json_types::U128, AccountId};
use std::sync::Arc;
use templar_common::{
    asset::{
        BorrowAsset, BorrowAssetAmount, CollateralAsset, CollateralAssetAmount, FungibleAsset,
        FungibleAssetAmount,
    },
    market::{DepositMsg, LiquidateMsg},
};

use crate::{
    inventory,
    rpc::{check_transaction_success, get_access_key_data, send_tx},
    swap::SwapProvider,
    CollateralStrategy, LiquidationOutcome, LiquidatorError, LiquidatorResult,
};

/// Liquidation transaction executor.
///
/// Responsible for:
/// - Creating liquidation transactions
/// - Managing inventory reservations
/// - Executing transactions
/// - Immediately swapping collateral based on strategy
pub struct LiquidationExecutor {
    client: JsonRpcClient,
    signer: Arc<Signer>,
    inventory: inventory::SharedInventory,
    market: AccountId,
    timeout: u64,
    dry_run: bool,
    collateral_strategy: CollateralStrategy,
    swap_provider: Option<crate::swap::SwapProviderImpl>,
    swap_retry_config: crate::swap::SwapRetryConfig,
    min_swap_value_usd: f64,
}

impl LiquidationExecutor {
    /// Creates a new liquidation executor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: JsonRpcClient,
        signer: Arc<Signer>,
        inventory: inventory::SharedInventory,
        market: AccountId,
        timeout: u64,
        dry_run: bool,
        collateral_strategy: CollateralStrategy,
        swap_provider: Option<crate::swap::SwapProviderImpl>,
        swap_retry_config: crate::swap::SwapRetryConfig,
        min_swap_value_usd: f64,
    ) -> Self {
        Self {
            client,
            signer,
            inventory,
            market,
            timeout,
            dry_run,
            collateral_strategy,
            swap_provider,
            swap_retry_config,
            min_swap_value_usd,
        }
    }

    /// Get reference to the shared inventory
    pub fn inventory(&self) -> &inventory::SharedInventory {
        &self.inventory
    }

    /// Check if executor is in dry run mode
    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Creates a transfer transaction for liquidation.
    fn create_transfer_tx(
        &self,
        borrow_asset: &FungibleAsset<BorrowAsset>,
        borrow_account: &AccountId,
        liquidation_amount: U128,
        collateral_amount: Option<U128>,
        nonce: u64,
        block_hash: CryptoHash,
    ) -> LiquidatorResult<Transaction> {
        let msg = near_sdk::serde_json::to_string(&DepositMsg::Liquidate(LiquidateMsg {
            account_id: borrow_account.clone(),
            amount: collateral_amount.map(Into::into),
        }))?;

        let function_call =
            borrow_asset.transfer_call_action(&self.market, liquidation_amount.into(), &msg);

        Ok(Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: borrow_asset.contract_id().into(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![function_call.into()],
        }))
    }

    /// Executes a liquidation transaction.
    ///
    /// # Flow
    /// 1. Reserve inventory
    /// 2. Create and submit transaction
    /// 3. Handle collateral based on strategy
    /// 4. Release inventory on failure
    #[tracing::instrument(skip(self, borrow_asset, collateral_asset), level = "info")]
    pub async fn execute_liquidation(
        &self,
        borrow_account: &AccountId,
        borrow_asset: &FungibleAsset<BorrowAsset>,
        collateral_asset: &FungibleAsset<CollateralAsset>,
        liquidation_amount: BorrowAssetAmount,
        collateral_amount: CollateralAssetAmount,
        expected_collateral_value: BorrowAssetAmount,
    ) -> LiquidatorResult<LiquidationOutcome> {
        // Dry run mode - log what would happen, skip execution
        if self.dry_run {
            // Log JIT swap intent if applicable
            if matches!(self.collateral_strategy, CollateralStrategy::SwapToBorrow)
                && self.swap_provider.is_some()
                && collateral_asset.to_string() != borrow_asset.to_string()
            {
                #[allow(clippy::cast_precision_loss)]
                let usd_estimate = u128::from(expected_collateral_value) as f64 / 1_000_000.0;

                if usd_estimate >= self.min_swap_value_usd {
                    tracing::info!(
                        borrower = %borrow_account,
                        from = %collateral_asset,
                        to = %borrow_asset,
                        collateral_amount = %u128::from(collateral_amount),
                        usd_value = format!("${usd_estimate:.2}"),
                        "[DRY RUN] Would JIT swap collateral after liquidation"
                    );
                } else {
                    tracing::info!(
                        borrower = %borrow_account,
                        from = %collateral_asset,
                        collateral_amount = %u128::from(collateral_amount),
                        usd_value = format!("${usd_estimate:.2}"),
                        threshold = format!("${:.2}", self.min_swap_value_usd),
                        "[DRY RUN] Would skip JIT swap (below threshold), batch later"
                    );
                }
            }
            return Ok(LiquidationOutcome::Liquidated);
        }

        // Reserve inventory for this liquidation
        self.inventory
            .write()
            .await
            .reserve(borrow_asset, liquidation_amount)?;

        tracing::info!(
            borrower = %borrow_account,
            liquidation_amount = %u128::from(liquidation_amount),
            borrow_asset = %borrow_asset,
            "Reserved inventory for liquidation"
        );

        // Execute liquidation transaction
        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer)
            .await
            .map_err(LiquidatorError::AccessKeyDataError)?;

        let tx = self.create_transfer_tx(
            borrow_asset,
            borrow_account,
            U128::from(liquidation_amount),
            Some(U128::from(collateral_amount)), // Request specific collateral amount calculated by strategy
            nonce,
            block_hash,
        )?;

        tracing::info!(
            borrower = %borrow_account,
            liquidation_amount = %u128::from(liquidation_amount),
            expected_collateral_value = %u128::from(expected_collateral_value),
            collateral_amount = %u128::from(collateral_amount),
            "Submitting liquidation transaction"
        );

        let tx_start = std::time::Instant::now();
        let tx_result = send_tx(&self.client, &self.signer, self.timeout, tx).await;

        match tx_result {
            Ok(outcome) => {
                let tx_duration = tx_start.elapsed();

                // Check if transaction AND all receipts succeeded
                match check_transaction_success(&outcome) {
                    Ok(()) => {
                        tracing::info!(
                            borrower = %borrow_account,
                            liquidation_amount = %u128::from(liquidation_amount),
                            expected_collateral_value = %u128::from(expected_collateral_value),
                            collateral_amount = %u128::from(collateral_amount),
                            tx_duration_ms = tx_duration.as_millis(),
                            "Liquidation executed successfully (all receipts succeeded)"
                        );

                        // Handle collateral based on strategy
                        let swap_succeeded = match &self.collateral_strategy {
                            CollateralStrategy::Hold => false, // No swap performed
                            CollateralStrategy::SwapToBorrow => {
                                // Estimate USD value for threshold check.
                                // The expected_collateral_value is denominated in borrow asset
                                // (often USDC), so it serves as a rough USD proxy.
                                #[allow(clippy::cast_precision_loss)]
                                let usd_estimate = Some(
                                    u128::from(expected_collateral_value) as f64 / 1_000_000.0,
                                );
                                // Immediately swap collateral back to borrow asset
                                self.swap_collateral_to_borrow(
                                    collateral_asset,
                                    borrow_asset,
                                    collateral_amount,
                                    usd_estimate,
                                )
                                .await
                                .unwrap_or(false)
                            }
                        };

                        // If swap succeeded, refresh inventory to get updated balance
                        if swap_succeeded {
                            if let Err(e) = self
                                .inventory
                                .write()
                                .await
                                .refresh_asset(borrow_asset)
                                .await
                            {
                                tracing::warn!(
                                    borrow_asset = %borrow_asset,
                                    error = ?e,
                                    "Failed to refresh inventory after swap, continuing with stale balance"
                                );
                            }
                        }

                        Ok(LiquidationOutcome::Liquidated)
                    }
                    Err(error_msg) => {
                        // Receipt failed - release reserved inventory
                        self.inventory
                            .write()
                            .await
                            .release(borrow_asset, liquidation_amount);

                        tracing::error!(
                            borrower = %borrow_account,
                            liquidation_amount = %u128::from(liquidation_amount),
                            error = %error_msg,
                            tx_hash = %outcome.transaction_outcome.id,
                            "Liquidation transaction had failed receipt, inventory released"
                        );
                        Err(LiquidatorError::TransactionFailed(error_msg))
                    }
                }
            }
            Err(e) => {
                // Release reserved inventory on RPC failure
                self.inventory
                    .write()
                    .await
                    .release(borrow_asset, liquidation_amount);

                tracing::error!(
                    borrower = %borrow_account,
                    liquidation_amount = %u128::from(liquidation_amount),
                    error = ?e,
                    "Liquidation RPC call failed, inventory released"
                );
                Err(LiquidatorError::LiquidationTransactionError(e))
            }
        }
    }

    /// Swap collateral immediately after liquidation.
    ///
    /// Returns `Ok(true)` if swap succeeded, `Ok(false)` if skipped or failed (non-fatal).
    #[allow(clippy::too_many_lines)]
    async fn swap_collateral_to_borrow(
        &self,
        collateral_asset: &FungibleAsset<CollateralAsset>,
        borrow_asset: &FungibleAsset<BorrowAsset>,
        collateral_amount: CollateralAssetAmount,
        expected_collateral_value_usd: Option<f64>,
    ) -> LiquidatorResult<bool> {
        let Some(ref swap_provider) = self.swap_provider else {
            tracing::debug!("No swap provider configured, holding collateral");
            return Ok(false);
        };

        // Skip swap if collateral is already the target borrow asset
        if collateral_asset.to_string() == borrow_asset.to_string() {
            tracing::debug!("Collateral is already borrow asset, skipping JIT swap");
            return Ok(false);
        }

        // Skip swap if the provider doesn't support this asset pair
        if !swap_provider.supports_assets(collateral_asset, borrow_asset) {
            tracing::info!(
                from = %collateral_asset,
                to = %borrow_asset,
                "Swap provider does not support asset pair, holding collateral"
            );
            return Ok(false);
        }

        // Skip swap if value is below threshold — will be picked up by batch swap
        if let Some(usd_value) = expected_collateral_value_usd {
            if usd_value < self.min_swap_value_usd {
                tracing::info!(
                    asset = %collateral_asset,
                    amount_raw = %u128::from(collateral_amount),
                    usd_value = format!("${usd_value:.2}"),
                    threshold = format!("${:.2}", self.min_swap_value_usd),
                    "Skipping JIT swap - below threshold, will batch later"
                );
                return Ok(false);
            }
        }

        let from_asset_id = collateral_asset.to_string();
        let to_asset_id = borrow_asset.to_string();

        tracing::info!(
            from = %from_asset_id,
            to = %to_asset_id,
            amount_raw = %u128::from(collateral_amount),
            "JIT swap: collateral→borrow"
        );

        let swap_amount = FungibleAssetAmount::from(U128::from(collateral_amount));
        let swap_name = format!("jit:{from_asset_id}");

        let provider = swap_provider.clone();
        let coll = collateral_asset.clone();
        let borrow = borrow_asset.clone();

        let result =
            crate::swap::retry::swap_with_retry(&self.swap_retry_config, &swap_name, || {
                let provider = provider.clone();
                let coll = coll.clone();
                let borrow = borrow.clone();
                async move {
                    use crate::swap::SwapProvider;
                    provider
                        .swap(&coll, &borrow, swap_amount)
                        .await
                        .map(|_| ())
                        .map_err(|e| {
                            let msg = e.to_string();
                            let kind = if msg.contains("Amount is too low") {
                                crate::swap::SwapErrorKind::AmountTooLow { message: msg }
                            } else if msg.contains("Failed to get quote") {
                                crate::swap::SwapErrorKind::QuoteFailed { message: msg }
                            } else {
                                crate::swap::SwapErrorKind::Unknown { message: msg }
                            };
                            crate::swap::SwapError::new(kind, "JIT swap")
                        })
                }
            })
            .await;

        match result {
            Ok(()) => {
                tracing::info!(
                    from = %from_asset_id,
                    to = %to_asset_id,
                    amount_raw = %u128::from(collateral_amount),
                    "JIT swap completed - inventory replenished"
                );
                Ok(true)
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("Amount too low") {
                    tracing::info!(
                        swap = %swap_name,
                        error = %e,
                        "JIT swap skipped - amount below provider minimum, will batch"
                    );
                } else if msg.contains("Quote failed") {
                    tracing::debug!(
                        swap = %swap_name,
                        "No swap route available for asset, holding collateral"
                    );
                } else {
                    tracing::info!(
                        swap = %swap_name,
                        error = %e,
                        "JIT swap failed, holding collateral"
                    );
                }
                Ok(false)
            }
        }
    }
}
