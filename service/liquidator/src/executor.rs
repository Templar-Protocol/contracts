//! Liquidation transaction executor module.
//!
//! Handles the creation and submission of liquidation transactions,
//! including inventory management and immediate collateral swapping.

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    hash::CryptoHash,
    transaction::{Transaction, TransactionV0},
    views::FinalExecutionStatus,
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
        // Dry run mode - skip execution
        if self.dry_run {
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
                                // Immediately swap collateral back to borrow asset
                                self.swap_collateral_to_borrow(
                                    collateral_asset,
                                    borrow_asset,
                                    collateral_amount,
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

    /// Swap collateral immediately after liquidation
    /// Returns Ok(true) if swap succeeded, Ok(false) if skipped, Err if failed
    async fn swap_collateral_to_borrow(
        &self,
        collateral_asset: &FungibleAsset<CollateralAsset>,
        borrow_asset: &FungibleAsset<BorrowAsset>,
        collateral_amount: CollateralAssetAmount,
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

        // Get asset IDs for logging
        let from_asset_id = collateral_asset.to_string();
        let to_asset_id = borrow_asset.to_string();

        tracing::info!(
            from = %from_asset_id,
            to = %to_asset_id,
            amount_raw = %u128::from(collateral_amount),
            "JIT swap: collateral→borrow"
        );

        let swap_amount = FungibleAssetAmount::from(U128::from(collateral_amount));

        match swap_provider
            .swap(collateral_asset, borrow_asset, swap_amount)
            .await
        {
            Ok(status) => {
                if let FinalExecutionStatus::SuccessValue(_) = status {
                    tracing::info!(
                        from = %from_asset_id,
                        to = %to_asset_id,
                        amount_raw = %u128::from(collateral_amount),
                        "JIT swap completed - inventory replenished"
                    );
                    Ok(true)
                } else {
                    tracing::warn!(
                        from = %from_asset_id,
                        to = %to_asset_id,
                        status = ?status,
                        "JIT swap failed (non-fatal) - holding collateral"
                    );
                    Ok(false)
                }
            }
            Err(e) => {
                tracing::warn!(
                    from = %from_asset_id,
                    to = %to_asset_id,
                    reason = %e,
                    "JIT swap failed (non-fatal) - holding collateral"
                );
                // Don't propagate error - swap failure is non-fatal, liquidation already succeeded
                Ok(false)
            }
        }
    }
}
