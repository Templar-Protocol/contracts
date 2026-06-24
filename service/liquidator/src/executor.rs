//! Liquidation transaction executor module.
//!
//! Handles the creation and submission of liquidation transactions,
//! including inventory management and immediate collateral swapping.

use near_sdk::{json_types::U128, AccountId};
use templar_common::asset::{
    BorrowAsset, BorrowAssetAmount, CollateralAsset, CollateralAssetAmount, FungibleAsset,
    FungibleAssetAmount,
};
use templar_gateway_client::SigningClient;
use templar_gateway_methods_spec::market;
use templar_gateway_types::OperationStatus;

use crate::{
    inventory, swap::SwapProvider, CollateralStrategy, LiquidationOutcome, LiquidatorError,
    LiquidatorResult,
};

/// Swap issue that occurred after a successful liquidation.
/// Returned to the caller so notifications can be sent in the right order
/// (liquidation success first, then swap issue).
#[derive(Debug)]
pub enum SwapIssue {
    /// Swap provider doesn't support this asset pair.
    Unsupported {
        from: String,
        to: String,
        amount: String,
    },
    /// Swap failed with an error.
    Failed {
        from: String,
        to: String,
        amount: String,
        error: String,
    },
}

/// Liquidation transaction executor.
///
/// Responsible for:
/// - Creating liquidation transactions
/// - Managing inventory reservations
/// - Executing transactions
/// - Immediately swapping collateral based on strategy
pub struct LiquidationExecutor {
    client: SigningClient,
    inventory: inventory::SharedInventory,
    market: AccountId,
    dry_run: bool,
    collateral_strategy: CollateralStrategy,
    swap_provider: Option<crate::swap::SwapProviderImpl>,
    swap_retry_config: crate::swap::SwapRetryConfig,
    min_swap_value_usd: f64,
    collateral_decimals: i32,
}

impl LiquidationExecutor {
    /// Creates a new liquidation executor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: SigningClient,
        inventory: inventory::SharedInventory,
        market: AccountId,
        dry_run: bool,
        collateral_strategy: CollateralStrategy,
        swap_provider: Option<crate::swap::SwapProviderImpl>,
        swap_retry_config: crate::swap::SwapRetryConfig,
        min_swap_value_usd: f64,
        collateral_decimals: i32,
    ) -> Self {
        Self {
            client,
            inventory,
            market,
            dry_run,
            collateral_strategy,
            swap_provider,
            swap_retry_config,
            min_swap_value_usd,
            collateral_decimals,
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

    /// Executes a liquidation transaction.
    ///
    /// # Flow
    /// 1. Reserve inventory
    /// 2. Create and submit transaction
    /// 3. Handle collateral based on strategy
    /// 4. Release inventory on failure
    #[tracing::instrument(skip(self, borrow_asset, collateral_asset), level = "info")]
    #[allow(clippy::too_many_lines)]
    pub async fn execute_liquidation(
        &self,
        borrow_account: &AccountId,
        borrow_asset: &FungibleAsset<BorrowAsset>,
        collateral_asset: &FungibleAsset<CollateralAsset>,
        liquidation_amount: BorrowAssetAmount,
        collateral_amount: CollateralAssetAmount,
        expected_collateral_value: BorrowAssetAmount,
    ) -> LiquidatorResult<(LiquidationOutcome, Option<SwapIssue>)> {
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
            return Ok((LiquidationOutcome::Liquidated, None));
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

        // Execute liquidation transaction through the gateway. The driver signs,
        // submits, and polls to finality; a reverted on-chain transaction comes
        // back as `Ok` with a `Failed` operation status (not an `Err`), so the
        // status is checked explicitly below.
        tracing::info!(
            borrower = %borrow_account,
            liquidation_amount = %u128::from(liquidation_amount),
            expected_collateral_value = %u128::from(expected_collateral_value),
            collateral_amount = %u128::from(collateral_amount),
            "Submitting liquidation transaction"
        );

        let tx_start = std::time::Instant::now();
        let tx_result = self
            .client
            .execute(market::Liquidate {
                market_id: self.market.clone(),
                account_id: borrow_account.clone(),
                liquidation_amount,
                collateral_amount: Some(collateral_amount), // Request specific collateral amount calculated by strategy
            })
            .await;

        match tx_result {
            Ok(operation_result) => {
                let tx_duration = tx_start.elapsed();

                // Check if the operation (and thus all receipts) succeeded.
                match operation_result.operation.status {
                    OperationStatus::Succeeded => {
                        tracing::info!(
                            borrower = %borrow_account,
                            liquidation_amount = %u128::from(liquidation_amount),
                            expected_collateral_value = %u128::from(expected_collateral_value),
                            collateral_amount = %u128::from(collateral_amount),
                            tx_duration_ms = tx_duration.as_millis(),
                            "Liquidation executed successfully (all receipts succeeded)"
                        );

                        // Release the reservation — tokens have left our account
                        self.inventory
                            .write()
                            .await
                            .release(borrow_asset, liquidation_amount);

                        // Handle collateral based on strategy
                        let (swap_succeeded, swap_issue) = match &self.collateral_strategy {
                            CollateralStrategy::Hold => (false, None),
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
                                .unwrap_or((false, None))
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

                        Ok((LiquidationOutcome::Liquidated, swap_issue))
                    }
                    failed_status => {
                        // Operation did not succeed (reverted receipt, or did not
                        // reach finality) - release reserved inventory.
                        self.inventory
                            .write()
                            .await
                            .release(borrow_asset, liquidation_amount);

                        let operation_id = operation_result.operation.id.0.clone();
                        let error_msg = format!(
                            "Liquidation operation {operation_id} ended with status {failed_status:?}"
                        );

                        tracing::error!(
                            borrower = %borrow_account,
                            liquidation_amount = %u128::from(liquidation_amount),
                            operation_id = %operation_id,
                            status = ?failed_status,
                            "Liquidation transaction did not succeed, inventory released"
                        );
                        Err(LiquidatorError::TransactionFailed(error_msg))
                    }
                }
            }
            Err(e) => {
                // Release reserved inventory on submission failure
                self.inventory
                    .write()
                    .await
                    .release(borrow_asset, liquidation_amount);

                tracing::error!(
                    borrower = %borrow_account,
                    liquidation_amount = %u128::from(liquidation_amount),
                    error = %e,
                    "Liquidation gateway call failed, inventory released"
                );
                Err(LiquidatorError::LiquidationTransactionError(e.into()))
            }
        }
    }

    /// Swap collateral immediately after liquidation.
    ///
    /// Returns `Ok((succeeded, swap_issue))` where `swap_issue` is populated
    /// when the swap failed or was unsupported (for notification by the caller).
    #[allow(clippy::too_many_lines)]
    async fn swap_collateral_to_borrow(
        &self,
        collateral_asset: &FungibleAsset<CollateralAsset>,
        borrow_asset: &FungibleAsset<BorrowAsset>,
        collateral_amount: CollateralAssetAmount,
        expected_collateral_value_usd: Option<f64>,
    ) -> LiquidatorResult<(bool, Option<SwapIssue>)> {
        let Some(ref swap_provider) = self.swap_provider else {
            tracing::debug!("No swap provider configured, holding collateral");
            return Ok((false, None));
        };

        // Skip swap if collateral is already the target borrow asset
        if collateral_asset.to_string() == borrow_asset.to_string() {
            tracing::debug!("Collateral is already borrow asset, skipping JIT swap");
            return Ok((false, None));
        }

        // Skip swap if the provider doesn't support this asset pair
        if !swap_provider.supports_assets(collateral_asset, borrow_asset) {
            tracing::info!(
                from = %collateral_asset,
                to = %borrow_asset,
                "Swap provider does not support asset pair, holding collateral"
            );
            return Ok((
                false,
                Some(SwapIssue::Unsupported {
                    from: crate::format::short_asset_name(&collateral_asset.to_string()),
                    to: crate::format::short_asset_name(&borrow_asset.to_string()),
                    amount: crate::format::format_amount_short(
                        u128::from(collateral_amount),
                        self.collateral_decimals,
                        &collateral_asset.to_string(),
                    ),
                }),
            ));
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
                return Ok((false, None));
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

        let amount_short = crate::format::format_amount_short(
            u128::from(collateral_amount),
            self.collateral_decimals,
            &from_asset_id,
        );
        let from_short = crate::format::short_asset_name(&from_asset_id);
        let to_short = crate::format::short_asset_name(&to_asset_id);

        match result {
            Ok(()) => {
                tracing::info!(
                    from = %from_asset_id,
                    to = %to_asset_id,
                    amount_raw = %u128::from(collateral_amount),
                    "JIT swap completed - inventory replenished"
                );
                Ok((true, None))
            }
            Err(e) => {
                let msg = e.to_string();
                let issue = if msg.contains("Amount too low") {
                    tracing::info!(
                        swap = %swap_name,
                        error = %e,
                        "JIT swap skipped - amount below provider minimum, will batch"
                    );
                    None // Not a notification-worthy issue
                } else if msg.contains("Quote failed") {
                    tracing::debug!(
                        swap = %swap_name,
                        "No swap route available for asset, holding collateral"
                    );
                    Some(SwapIssue::Failed {
                        from: from_short,
                        to: to_short,
                        amount: amount_short,
                        error: "No swap route available".to_string(),
                    })
                } else {
                    tracing::info!(
                        swap = %swap_name,
                        error = %e,
                        "JIT swap failed, holding collateral"
                    );
                    Some(SwapIssue::Failed {
                        from: from_short,
                        to: to_short,
                        amount: amount_short,
                        error: msg,
                    })
                };
                Ok((false, issue))
            }
        }
    }
}
