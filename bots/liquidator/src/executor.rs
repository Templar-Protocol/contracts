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
    asset::{BorrowAsset, CollateralAsset, FungibleAsset},
    market::{DepositMsg, LiquidateMsg},
};
use tracing::{debug, error, info};

use crate::{
    inventory,
    rpc::{check_transaction_success, get_access_key_data, send_tx},
    LiquidationOutcome, LiquidatorError, LiquidatorResult,
};

/// Liquidation transaction executor.
///
/// Responsible for:
/// - Creating liquidation transactions
/// - Managing inventory reservations
/// - Executing transactions
/// - Collateral is added to inventory (rebalancer handles swaps)
pub struct LiquidationExecutor {
    client: JsonRpcClient,
    signer: Arc<Signer>,
    inventory: inventory::SharedInventory,
    market: AccountId,
    timeout: u64,
    dry_run: bool,
}

impl LiquidationExecutor {
    /// Creates a new liquidation executor.
    pub fn new(
        client: JsonRpcClient,
        signer: Arc<Signer>,
        inventory: inventory::SharedInventory,
        market: AccountId,
        timeout: u64,
        dry_run: bool,
    ) -> Self {
        Self {
            client,
            signer,
            inventory,
            market,
            timeout,
            dry_run,
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

                        // Collateral is now in inventory - rebalancer will handle any swaps
                        debug!(
                            borrower = %borrow_account,
                            collateral_asset = %collateral_asset,
                            amount = %collateral_amount.0,
                            "Collateral added to inventory"
                        );

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
}
