// SPDX-License-Identifier: MIT
//! Liquidation transaction executor module.
//!
//! Handles the creation and submission of liquidation transactions,
//! including inventory management and collateral strategy execution.

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    hash::CryptoHash,
    transaction::{Transaction, TransactionV0},
};
use near_sdk::{json_types::U128, serde_json, AccountId};
use std::sync::Arc;
use templar_common::{
    asset::{AssetClass, BorrowAsset, CollateralAsset, FungibleAsset},
    market::{DepositMsg, LiquidateMsg},
};
use tracing::{debug, error, info, warn};

use crate::{
    inventory,
    rpc::{check_transaction_success, get_access_key_data, send_tx},
    swap::SwapProviderImpl,
    CollateralStrategy, LiquidationOutcome, LiquidatorError, LiquidatorResult,
};

/// Liquidation transaction executor.
///
/// Responsible for:
/// - Creating liquidation transactions
/// - Managing inventory reservations
/// - Executing transactions
/// - Handling collateral based on strategy (including post-liquidation swaps)
pub struct LiquidationExecutor {
    client: JsonRpcClient,
    signer: Arc<Signer>,
    inventory: inventory::SharedInventory,
    market: AccountId,
    collateral_strategy: CollateralStrategy,
    timeout: u64,
    dry_run: bool,
    swap_provider: Option<SwapProviderImpl>,
}

impl LiquidationExecutor {
    /// Creates a new liquidation executor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: JsonRpcClient,
        signer: Arc<Signer>,
        inventory: inventory::SharedInventory,
        market: AccountId,
        collateral_strategy: CollateralStrategy,
        timeout: u64,
        dry_run: bool,
        swap_provider: Option<SwapProviderImpl>,
    ) -> Self {
        Self {
            client,
            signer,
            inventory,
            market,
            collateral_strategy,
            timeout,
            dry_run,
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
        let msg = serde_json::to_string(&DepositMsg::Liquidate(LiquidateMsg {
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
        liquidation_amount: U128,
        collateral_amount: U128,
        expected_collateral_value: U128,
    ) -> LiquidatorResult<LiquidationOutcome> {
        // Dry run mode - log and skip execution
        if self.dry_run {
            info!(
                borrower = %borrow_account,
                liquidation_amount = %liquidation_amount.0,
                collateral_amount = %collateral_amount.0,
                borrow_asset = %borrow_asset,
                "DRY RUN: Liquidatable position found, skipping execution (dry run mode enabled)"
            );
            return Ok(LiquidationOutcome::Liquidated);
        }

        // Reserve inventory for this liquidation
        self.inventory
            .write()
            .await
            .reserve(borrow_asset, liquidation_amount)?;

        info!(
            borrower = %borrow_account,
            liquidation_amount = %liquidation_amount.0,
            borrow_asset = %borrow_asset,
            "Reserved inventory for liquidation"
        );

        // Note: We assume the bot is already registered with the collateral token contract.
        // Registration should be done during initialization.
        debug!(
            borrower = %borrow_account,
            collateral_asset = %collateral_asset,
            bot_account = %self.signer.get_account_id(),
            "Bot will receive collateral (registration assumed complete)"
        );

        // Execute liquidation transaction
        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer)
            .await
            .map_err(LiquidatorError::AccessKeyDataError)?;

        let tx = self.create_transfer_tx(
            borrow_asset,
            borrow_account,
            liquidation_amount,
            Some(collateral_amount), // Request specific collateral amount calculated by strategy
            nonce,
            block_hash,
        )?;

        info!(
            borrower = %borrow_account,
            liquidation_amount = %liquidation_amount.0,
            expected_collateral_value = %expected_collateral_value.0,
            collateral_amount = %collateral_amount.0,
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
                        info!(
                            borrower = %borrow_account,
                            liquidation_amount = %liquidation_amount.0,
                            expected_collateral_value = %expected_collateral_value.0,
                            collateral_amount = %collateral_amount.0,
                            tx_duration_ms = tx_duration.as_millis(),
                            "Liquidation executed successfully (all receipts succeeded)"
                        );

                        // Record liquidation history for swap-to-borrow strategy
                        self.inventory
                            .write()
                            .await
                            .record_liquidation(borrow_asset, collateral_asset);

                        // Handle collateral based on strategy (may swap)
                        self.handle_collateral(
                            borrow_account,
                            borrow_asset,
                            collateral_asset,
                            collateral_amount,
                        )
                        .await;

                        Ok(LiquidationOutcome::Liquidated)
                    }
                    Err(error_msg) => {
                        // Receipt failed - release reserved inventory
                        self.inventory
                            .write()
                            .await
                            .release(borrow_asset, liquidation_amount);

                        error!(
                            borrower = %borrow_account,
                            liquidation_amount = %liquidation_amount.0,
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

                error!(
                    borrower = %borrow_account,
                    liquidation_amount = %liquidation_amount.0,
                    error = ?e,
                    "Liquidation RPC call failed, inventory released"
                );
                Err(LiquidatorError::LiquidationTransactionError(e))
            }
        }
    }

    /// Handles collateral based on the configured strategy.
    ///
    /// For swap strategies, performs post-liquidation swap of collateral.
    async fn handle_collateral(
        &self,
        borrow_account: &AccountId,
        borrow_asset: &FungibleAsset<BorrowAsset>,
        collateral_asset: &FungibleAsset<CollateralAsset>,
        collateral_amount: U128,
    ) {
        match &self.collateral_strategy {
            CollateralStrategy::Hold => {
                info!(
                    borrower = %borrow_account,
                    collateral_asset = %collateral_asset,
                    expected_amount = %collateral_amount.0,
                    "Collateral will be held (strategy: Hold)"
                );
                // Inventory will be refreshed on next scan
            }
            CollateralStrategy::SwapToPrimary { primary_asset } => {
                info!(
                    borrower = %borrow_account,
                    collateral_asset = %collateral_asset,
                    primary_asset = %primary_asset,
                    amount = %collateral_amount.0,
                    "Swapping collateral to primary asset (strategy: SwapToPrimary)"
                );

                if let Some(ref swap_provider) = self.swap_provider {
                    match self
                        .execute_swap(
                            collateral_asset,
                            primary_asset,
                            collateral_amount,
                            swap_provider,
                        )
                        .await
                    {
                        Ok(()) => {
                            info!(
                                collateral_asset = %collateral_asset,
                                primary_asset = %primary_asset,
                                "Successfully swapped collateral to primary asset"
                            );
                        }
                        Err(e) => {
                            error!(
                                collateral_asset = %collateral_asset,
                                primary_asset = %primary_asset,
                                error = ?e,
                                "Failed to swap collateral to primary asset, will hold collateral"
                            );
                        }
                    }
                } else {
                    warn!(
                        "SwapToPrimary strategy configured but no swap provider available, holding collateral"
                    );
                }
            }
            CollateralStrategy::SwapToBorrow => {
                info!(
                    borrower = %borrow_account,
                    collateral_asset = %collateral_asset,
                    target_asset = %borrow_asset,
                    amount = %collateral_amount.0,
                    "Swapping collateral back to borrow asset (strategy: SwapToBorrow)"
                );

                if let Some(ref swap_provider) = self.swap_provider {
                    match self
                        .execute_swap(
                            collateral_asset,
                            borrow_asset,
                            collateral_amount,
                            swap_provider,
                        )
                        .await
                    {
                        Ok(()) => {
                            info!(
                                collateral_asset = %collateral_asset,
                                target_asset = %borrow_asset,
                                "Successfully swapped collateral to borrow asset"
                            );
                        }
                        Err(e) => {
                            error!(
                                collateral_asset = %collateral_asset,
                                target_asset = %borrow_asset,
                                error = ?e,
                                "Failed to swap collateral to borrow asset, will hold collateral"
                            );
                    }
                }
            } else {
                warn!(
                    "SwapToBorrow strategy configured but no swap provider available, holding collateral"
                );
            }
        }
    }
}    /// Executes a swap using the configured swap provider.
    async fn execute_swap<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        amount: U128,
        swap_provider: &SwapProviderImpl,
    ) -> LiquidatorResult<()> {
        use crate::swap::SwapProvider;

        // Get quote
        info!(
            from_asset = %from_asset,
            to_asset = %to_asset,
            amount = %amount.0,
            provider = %swap_provider.provider_name(),
            "Getting swap quote"
        );

        let output_amount = swap_provider
            .quote(from_asset, to_asset, amount)
            .await
            .map_err(|e| LiquidatorError::StrategyError(format!("Swap quote failed: {e:?}")))?;

        info!(
            from_asset = %from_asset,
            to_asset = %to_asset,
            input_amount = %amount.0,
            output_amount = %output_amount.0,
            provider = %swap_provider.provider_name(),
            "Executing swap"
        );

        // Execute swap
        let _status = swap_provider
            .swap(from_asset, to_asset, amount)
            .await
            .map_err(|e| LiquidatorError::StrategyError(format!("Swap execution failed: {e:?}")))?;

        info!(
            from_asset = %from_asset,
            to_asset = %to_asset,
            "Swap completed successfully"
        );

        Ok(())
    }
}
