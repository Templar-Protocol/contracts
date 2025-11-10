//! Inventory rebalancing service for post-liquidation portfolio management.
//!
//! Automatically rebalances the bot's asset inventory after liquidations by
//! swapping received collateral based on configured strategy.
//!
//! Supports multiple strategies:
//! - **Hold**: Keep all collateral as received
//! - **`SwapToPrimary`**: Convert all collateral to a single primary asset
//! - **`SwapToBorrow`**: Convert collateral back to original borrow assets

use std::{sync::Arc, time::Instant};

use near_primitives::views::FinalExecutionStatus;
use near_sdk::json_types::U128;
use templar_common::asset::{
    AssetClass, BorrowAsset, CollateralAsset, FungibleAsset, FungibleAssetAmount,
};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn, Instrument};

use crate::{
    inventory::InventoryManager,
    swap::{SwapProvider, SwapProviderImpl},
    CollateralStrategy,
};

/// Rebalancing operation metrics
#[derive(Debug, Clone, Default)]
pub struct RebalanceMetrics {
    /// Total swaps attempted
    pub swaps_attempted: u64,
    /// Successful swaps
    pub swaps_successful: u64,
    /// Failed swaps
    pub swaps_failed: u64,
    /// Total input amount swapped (in smallest units)
    pub total_input_amount: u128,
    /// Total swap latency in milliseconds
    pub total_latency_ms: u128,
    /// NEP-245 tokens skipped (not swappable)
    pub nep245_skipped: u64,
    /// Assets with no target market
    pub no_target_skipped: u64,
}

impl RebalanceMetrics {
    /// Average swap latency in milliseconds
    pub fn avg_latency_ms(&self) -> u128 {
        if self.swaps_successful > 0 {
            self.total_latency_ms / u128::from(self.swaps_successful)
        } else {
            0
        }
    }

    /// Success rate as percentage (0-100)
    #[allow(clippy::cast_precision_loss)]
    pub fn success_rate(&self) -> f64 {
        if self.swaps_attempted > 0 {
            (self.swaps_successful as f64 / self.swaps_attempted as f64) * 100.0
        } else {
            0.0
        }
    }

    /// Log metrics summary
    pub fn log_summary(&self) {
        if self.swaps_attempted == 0 {
            info!("No collateral swaps attempted - no liquidation history found");
            return;
        }

        info!(
            swaps_attempted = self.swaps_attempted,
            swaps_successful = self.swaps_successful,
            swaps_failed = self.swaps_failed,
            success_rate = format!("{:.2}%", self.success_rate()),
            avg_latency_ms = self.avg_latency_ms(),
            nep245_skipped = self.nep245_skipped,
            no_target_skipped = self.no_target_skipped,
            "Rebalancing metrics summary"
        );
    }
}

/// Inventory rebalancer for post-liquidation portfolio management.
pub struct InventoryRebalancer {
    /// Shared inventory manager
    inventory: Arc<RwLock<InventoryManager>>,
    /// Swap provider for collateral rebalancing
    oneclick_provider: Option<SwapProviderImpl>,
    /// Rebalancing strategy
    strategy: CollateralStrategy,
    /// Rebalancing metrics
    metrics: RebalanceMetrics,
    /// Dry run mode
    dry_run: bool,
}

impl InventoryRebalancer {
    /// Creates a new inventory rebalancer
    pub fn new(
        inventory: Arc<RwLock<InventoryManager>>,
        oneclick_provider: Option<SwapProviderImpl>,
        strategy: CollateralStrategy,
        dry_run: bool,
    ) -> Self {
        Self {
            inventory,
            oneclick_provider,
            strategy,
            metrics: RebalanceMetrics::default(),
            dry_run,
        }
    }

    /// Get current rebalancing metrics
    pub fn metrics(&self) -> &RebalanceMetrics {
        &self.metrics
    }

    /// Reset metrics at start of each round
    pub fn reset_metrics(&mut self) {
        self.metrics = RebalanceMetrics::default();
    }

    /// Rebalance inventory based on configured strategy
    pub async fn rebalance(&mut self) {
        let swap_span = tracing::debug_span!("collateral_swap_round");

        async {
            // Get pending swap amounts (only liquidated collateral, not entire balance)
            let collateral_balances = self.inventory.read().await.get_pending_swap_amounts();

            if collateral_balances.is_empty() {
                debug!("No liquidated collateral pending swap");
                return;
            }

            info!(
                collateral_count = collateral_balances.len(),
                strategy = ?self.strategy,
                "Starting inventory rebalancing for liquidated collateral"
            );

            // Execute swaps based on strategy
            let strategy = self.strategy.clone();
            match strategy {
                CollateralStrategy::Hold => {
                    info!("Collateral strategy is Hold - keeping all collateral");
                }
                CollateralStrategy::SwapToPrimary { primary_asset } => {
                    self.swap_to_primary(&collateral_balances, &primary_asset)
                        .await;
                }
                CollateralStrategy::SwapToBorrow => {
                    self.swap_to_borrow(&collateral_balances).await;
                }
            }

            // Log metrics
            self.metrics.log_summary();

            // Refresh inventories after swaps
            if self.metrics.swaps_successful > 0 {
                info!("Refreshing inventories after successful swaps");
                let _ = self.inventory.write().await.refresh().await;
                let _ = self.inventory.write().await.refresh_collateral().await;
            }
        }
        .instrument(swap_span)
        .await;
    }

    /// Swap all collateral to a single primary asset
    async fn swap_to_primary(
        &mut self,
        collateral_balances: &std::collections::HashMap<String, U128>,
        primary_asset: &FungibleAsset<CollateralAsset>,
    ) {
        if self.oneclick_provider.is_none() {
            warn!("Swap provider not configured");
            return;
        }

        // Get actual on-chain balances for verification
        let actual_balances = self.inventory.read().await.get_collateral_balances();

        for (collateral_asset_str, pending_balance) in collateral_balances {
            // Skip if already the primary asset
            if collateral_asset_str == &primary_asset.to_string() {
                debug!(
                    asset = %collateral_asset_str,
                    "Skipping swap - already primary asset"
                );
                continue;
            }

            // Verify actual balance matches pending amount
            let actual_balance = actual_balances
                .get(collateral_asset_str)
                .map_or(0, |b| b.0);

            if actual_balance == 0 {
                warn!(
                    collateral = %collateral_asset_str,
                    pending = %pending_balance.0,
                    "No actual balance found - tokens may have been transferred externally. Skipping swap."
                );
                continue;
            }

            // Use the minimum of pending and actual to avoid "insufficient balance" errors
            let swap_amount = std::cmp::min(pending_balance.0, actual_balance);
            if swap_amount < pending_balance.0 {
                warn!(
                    collateral = %collateral_asset_str,
                    pending = %pending_balance.0,
                    actual = %actual_balance,
                    swap = %swap_amount,
                    "Actual balance is less than pending - using actual balance for swap"
                );
            }

            info!(
                collateral = %collateral_asset_str,
                swap_amount = %swap_amount,
                "Preparing to swap collateral balance to primary asset"
            );

            // Parse asset
            match collateral_asset_str.parse::<FungibleAsset<CollateralAsset>>() {
                Ok(collateral_asset) => {
                    self.execute_swap(
                        &collateral_asset,
                        primary_asset,
                        FungibleAssetAmount::from(U128(swap_amount)),
                    )
                    .await;
                }
                Err(e) => {
                    error!(
                        asset = %collateral_asset_str,
                        error = ?e,
                        "Failed to parse asset"
                    );
                }
            }
        }
    }

    /// Swap collateral back to borrow assets based on liquidation history
    async fn swap_to_borrow(
        &mut self,
        collateral_balances: &std::collections::HashMap<String, U128>,
    ) {
        if self.oneclick_provider.is_none() {
            warn!("Swap provider not configured");
            return;
        }

        // Build swap plan (while holding read lock)
        let swap_plan: Vec<(String, String, U128)> = {
            let mut inventory_write = self.inventory.write().await;

            // Get actual on-chain balances for verification
            let actual_balances = inventory_write.get_collateral_balances();

            let mut plan = Vec::new();
            for (collateral_asset_str, pending_balance) in collateral_balances {
                info!(
                    collateral = %collateral_asset_str,
                    pending_balance = %pending_balance.0,
                    "Checking liquidation history for swap target"
                );

                // Parse collateral asset
                let Ok(collateral_asset) =
                    collateral_asset_str.parse::<templar_common::asset::FungibleAsset<
                        templar_common::asset::CollateralAsset,
                    >>()
                else {
                    warn!(
                        collateral = %collateral_asset_str,
                        "Failed to parse collateral asset, skipping"
                    );
                    continue;
                };

                // Verify actual balance matches pending amount
                let actual_balance = actual_balances
                    .get(collateral_asset_str)
                    .map_or(0, |b| b.0);

                if actual_balance == 0 {
                    warn!(
                        collateral = %collateral_asset_str,
                        pending = %pending_balance.0,
                        "No actual balance found - tokens may have been transferred externally or by previous swap. Clearing pending swap record."
                    );
                    inventory_write.clear_liquidation_history(&collateral_asset);
                    continue;
                }

                // Use the minimum of pending and actual to avoid "insufficient balance" errors
                let swap_amount = std::cmp::min(pending_balance.0, actual_balance);
                if swap_amount < pending_balance.0 {
                    warn!(
                        collateral = %collateral_asset_str,
                        pending = %pending_balance.0,
                        actual = %actual_balance,
                        swap = %swap_amount,
                        "Actual balance is less than pending - updating pending amount to match actual balance"
                    );
                    // Update pending_swaps to reflect actual balance
                    inventory_write.update_pending_swap_amount(&collateral_asset, U128(actual_balance));
                }

                // Only swap if we have liquidation history
                let target_asset_str = if let Some(target) =
                    inventory_write.get_liquidation_history(&collateral_asset)
                {
                    let target_str = target.to_string();
                    info!(
                        collateral = %collateral_asset_str,
                        target = %target_str,
                        swap_amount = %swap_amount,
                        "Found liquidation history"
                    );
                    target_str
                } else {
                    debug!(
                        collateral = %collateral_asset_str,
                        "No liquidation history, skipping"
                    );
                    continue;
                };

                // Skip if already the target asset
                if collateral_asset_str == &target_asset_str {
                    debug!(
                        asset = %collateral_asset_str,
                        "Already target asset, skipping"
                    );
                    continue;
                }

                plan.push((collateral_asset_str.clone(), target_asset_str, U128(swap_amount)));
            }

            plan
        }; // Write lock released

        // Execute swaps with parsed assets
        for (from_str, to_str, amount) in swap_plan {
            info!(
                from = %from_str,
                to = %to_str,
                amount = %amount.0,
                "Attempting to swap collateral"
            );

            // Parse assets
            match (
                from_str.parse::<FungibleAsset<CollateralAsset>>(),
                to_str.parse::<FungibleAsset<BorrowAsset>>(),
            ) {
                (Ok(from_asset), Ok(to_asset)) => {
                    self.execute_swap(&from_asset, &to_asset, FungibleAssetAmount::from(amount))
                        .await;
                }
                _ => {
                    error!(
                        from = %from_str,
                        to = %to_str,
                        "Failed to parse assets for swap"
                    );
                }
            }
        }
    }

    /// Execute a swap with metrics tracking
    #[allow(clippy::too_many_lines)]
    async fn execute_swap<T>(
        &mut self,
        from_asset: &FungibleAsset<CollateralAsset>,
        to_asset: &FungibleAsset<T>,
        input_amount: FungibleAssetAmount<CollateralAsset>,
    ) where
        T: AssetClass,
    {
        self.metrics.swaps_attempted += 1;
        let swap_start = Instant::now();

        // Select swap provider
        let (swap_provider, provider_name) =
            if let Some(provider) = self.select_provider(from_asset, to_asset) {
                let name = provider.provider_name();
                (provider, name)
            } else {
                self.metrics.swaps_failed += 1;
                info!(
                    from = %from_asset,
                    to = %to_asset,
                    "No swap provider available"
                );
                return;
            };

        info!(
            from = %from_asset,
            to = %to_asset,
            input_amount = %u128::from(input_amount),
            provider = %provider_name,
            "Starting swap execution"
        );

        // Verify provider supports assets
        if !swap_provider.supports_assets(from_asset, to_asset) {
            self.metrics.swaps_failed += 1;
            warn!(
                from = %from_asset,
                to = %to_asset,
                provider = %provider_name,
                "Provider does not support these assets"
            );
            return;
        }

        // Check dry run mode
        if self.dry_run {
            info!(
                from = %from_asset,
                to = %to_asset,
                input_amount = %u128::from(input_amount),
                provider = %provider_name,
                "[DRY RUN] Skipping swap"
            );
            return;
        }

        // Execute swap with input amount (all providers use input-based swaps for rebalancing)
        info!(
            from = %from_asset,
            to = %to_asset,
            input_amount = %u128::from(input_amount),
            "Executing input-based swap"
        );

        // Execute swap
        match swap_provider.swap(from_asset, to_asset, input_amount).await {
            Ok(FinalExecutionStatus::SuccessValue(_)) => {
                let latency = swap_start.elapsed().as_millis();
                self.metrics.swaps_successful += 1;
                self.metrics.total_input_amount += u128::from(input_amount);
                self.metrics.total_latency_ms += latency;

                info!(
                    from = %from_asset,
                    to = %to_asset,
                    input = %u128::from(input_amount),
                    latency_ms = latency,
                    "Swap completed successfully"
                );

                // Clear liquidation history for this collateral
                self.inventory
                    .write()
                    .await
                    .clear_liquidation_history(from_asset);
            }
            Ok(status) => {
                self.metrics.swaps_failed += 1;
                error!(
                    from = %from_asset,
                    to = %to_asset,
                    status = ?status,
                    "Swap failed with unexpected status"
                );
            }
            Err(e) => {
                self.metrics.swaps_failed += 1;
                error!(
                    from = %from_asset,
                    to = %to_asset,
                    error = %e,
                    "Swap execution failed"
                );
            }
        }
    }

    /// Selects the swap provider for the given asset pair
    fn select_provider<F, T>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
    ) -> Option<&SwapProviderImpl>
    where
        F: AssetClass,
        T: AssetClass,
    {
        // Use 1-Click API for all swaps
        if let Some(provider) = self.oneclick_provider.as_ref() {
            if provider.supports_assets(from_asset, to_asset) {
                debug!(
                    from = %from_asset,
                    to = %to_asset,
                    "Using 1-Click API"
                );
                return Some(provider);
            }
            warn!(
                from = %from_asset,
                to = %to_asset,
                "Asset pair not supported"
            );
        } else {
            warn!("Swap provider not available");
        }

        None
    }
}
