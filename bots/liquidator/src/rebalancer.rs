// SPDX-License-Identifier: MIT
//! Inventory rebalancing service for post-liquidation portfolio management.
//!
//! The `InventoryRebalancer` automatically rebalances the bot's asset inventory
//! after liquidations by swapping received collateral back to borrow assets or
//! a primary asset, based on the configured strategy.
//!
//! # Features
//!
//! - Intelligent swap routing (liquidation history + market configuration)
//! - Multiple rebalancing strategies (Hold, SwapToPrimary, SwapToBorrow)
//! - Comprehensive metrics (success rate, latency, amounts)
//! - Swap provider abstraction (1-Click API, Ref Finance)

use std::{
    sync::Arc,
    time::Instant,
};

use near_primitives::views::FinalExecutionStatus;
use near_sdk::json_types::U128;
use templar_common::asset::{AssetClass, BorrowAsset, CollateralAsset, FungibleAsset};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn, Instrument};

use crate::{
    inventory::InventoryManager,
    swap::{SwapProvider, SwapProviderImpl},
    CollateralStrategy, Liquidator,
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
    /// Total output amount received (in smallest units)
    pub total_output_amount: u128,
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
            self.total_latency_ms / self.swaps_successful as u128
        } else {
            0
        }
    }

    /// Success rate as percentage (0-100)
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
            info!("No rebalancing swaps attempted this round");
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
///
/// Automatically rebalances asset inventory after liquidations by swapping
/// received collateral based on the configured rebalancing strategy.
///
/// Uses intelligent routing:
/// - Ref Finance for NEP-141 tokens (NEAR-native like stNEAR, USDC)
/// - 1-Click API for NEP-245 tokens (cross-chain like BTC, ETH USDC)
pub struct InventoryRebalancer {
    /// Shared inventory manager
    inventory: Arc<RwLock<InventoryManager>>,
    /// Ref Finance swap provider for NEP-141 tokens
    ref_provider: Option<SwapProviderImpl>,
    /// OneClick swap provider for NEP-245 tokens
    oneclick_provider: Option<SwapProviderImpl>,
    /// Rebalancing strategy
    strategy: CollateralStrategy,
    /// Rebalancing metrics
    metrics: RebalanceMetrics,
    /// Dry run mode
    dry_run: bool,
}

impl InventoryRebalancer {
    /// Creates a new inventory rebalancer with intelligent routing
    pub fn new(
        inventory: Arc<RwLock<InventoryManager>>,
        ref_provider: Option<SwapProviderImpl>,
        oneclick_provider: Option<SwapProviderImpl>,
        strategy: CollateralStrategy,
        dry_run: bool,
    ) -> Self {
        Self {
            inventory,
            ref_provider,
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

    /// Reset metrics (call at start of each rebalancing round)
    pub fn reset_metrics(&mut self) {
        self.metrics = RebalanceMetrics::default();
    }

    /// Rebalance inventory based on configured strategy
    ///
    /// This is the main entry point that:
    /// 1. Queries current collateral balances
    /// 2. Executes swaps based on strategy (Hold/SwapToPrimary/SwapToBorrow)
    /// 3. Tracks metrics
    /// 4. Refreshes inventory after successful swaps
    pub async fn rebalance(&mut self, markets: &[&Liquidator]) {
        let swap_span = tracing::debug_span!("collateral_swap_round");

        async {
            // Get current collateral balances
            let collateral_balances = self.inventory.read().await.get_collateral_balances();

            if collateral_balances.is_empty() {
                debug!("No collateral holdings to process");
                return;
            }

            info!(
                collateral_count = collateral_balances.len(),
                strategy = ?self.strategy,
                "Starting inventory rebalancing"
            );

            // Execute swaps based on strategy
            let strategy = self.strategy.clone();
            match strategy {
                CollateralStrategy::Hold => {
                    info!("Collateral strategy is Hold - keeping all collateral");
                }
                CollateralStrategy::SwapToPrimary { primary_asset } => {
                    self.swap_to_primary(&collateral_balances, &primary_asset).await;
                }
                CollateralStrategy::SwapToBorrow => {
                    self.swap_to_borrow(&collateral_balances, markets).await;
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
        if self.ref_provider.is_none() && self.oneclick_provider.is_none() {
            warn!("No swap providers configured - cannot swap collateral");
            return;
        }

        for (collateral_asset_str, balance) in collateral_balances {
            // Skip if already the primary asset
            if collateral_asset_str == &primary_asset.to_string() {
                debug!(
                    asset = %collateral_asset_str,
                    "Skipping swap - already primary asset"
                );
                continue;
            }

            // TEST MODE: Only swap 20% of collateral to test the flow
            let test_percentage = 20u128;
            
            let test_amount = U128(balance.0 * test_percentage / 100);

            info!(
                collateral = %collateral_asset_str,
                total_balance = %balance.0,
                test_amount = %test_amount.0,
                test_percentage = test_percentage,
                "TEST MODE: Swapping {}% of collateral",
                test_percentage
            );

            // Parse asset
            match collateral_asset_str.parse::<FungibleAsset<CollateralAsset>>() {
                Ok(collateral_asset) => {
                    self.execute_swap(&collateral_asset, primary_asset, test_amount)
                        .await;
                }
                Err(e) => {
                    error!(
                        asset = %collateral_asset_str,
                        error = ?e,
                        "Failed to parse collateral asset"
                    );
                }
            }
        }
    }

    /// Swap collateral back to borrow assets (intelligent routing)
    async fn swap_to_borrow(
        &mut self,
        collateral_balances: &std::collections::HashMap<String, U128>,
        markets: &[&Liquidator],
    ) {
        if self.ref_provider.is_none() && self.oneclick_provider.is_none() {
            warn!("No swap providers configured - cannot swap collateral");
            return;
        }

        // Build swap plan (while holding read lock)
        let swap_plan: Vec<(String, String, U128)> = {
            let inventory_read = self.inventory.read().await;

            let mut plan = Vec::new();
            for (collateral_asset_str, balance) in collateral_balances {
                // TEST MODE: Only swap 33% of collateral to test the flow
                let test_percentage = 33u128;
                
                let test_amount = U128(balance.0 * test_percentage / 100);

                info!(
                    collateral = %collateral_asset_str,
                    total_balance = %balance.0,
                    test_amount = %test_amount.0,
                    test_percentage = test_percentage,
                    "TEST MODE: Swapping {}% of collateral",
                    test_percentage
                );

                // Determine target borrow asset
                let target_asset_str =
                    if let Some(target_from_history) = inventory_read.get_liquidation_history(collateral_asset_str) {
                        info!(
                            collateral = %collateral_asset_str,
                            target = %target_from_history,
                            "Using liquidation history to determine swap target"
                        );
                        target_from_history.clone()
                    } else {
                        // No history - use market configuration
                        info!(
                            collateral = %collateral_asset_str,
                            "No liquidation history - checking market configurations"
                        );

                        // Find markets using this collateral
                        let mut matching_markets: Vec<(String, u128)> = Vec::new();
                        for liquidator in markets {
                            let market_collateral = liquidator.market_config.collateral_asset.to_string();
                            if market_collateral == *collateral_asset_str {
                                let borrow_asset_str = liquidator.market_config.borrow_asset.to_string();
                                let borrow_balance =
                                    inventory_read.get_available_balance(&liquidator.market_config.borrow_asset).0;
                                matching_markets.push((borrow_asset_str, borrow_balance));
                            }
                        }

                        if matching_markets.is_empty() {
                            warn!(
                                collateral = %collateral_asset_str,
                                "No markets found using this collateral asset"
                            );
                            self.metrics.no_target_skipped += 1;
                            continue;
                        }

                        // Select market with highest borrow asset balance
                        matching_markets.sort_by(|a, b| b.1.cmp(&a.1));
                        let target = &matching_markets[0].0;

                        if matching_markets.len() > 1 {
                            info!(
                                collateral = %collateral_asset_str,
                                markets_count = matching_markets.len(),
                                selected_target = %target,
                                "Multiple markets - selected one with highest borrow balance"
                            );
                        } else {
                            info!(
                                collateral = %collateral_asset_str,
                                target = %target,
                                "Using market configuration for swap target"
                            );
                        }

                        target.clone()
                    };

                // Skip if already the target asset
                if collateral_asset_str == &target_asset_str {
                    debug!(
                        asset = %collateral_asset_str,
                        "Skipping swap - already a borrow asset"
                    );
                    continue;
                }

                plan.push((collateral_asset_str.clone(), target_asset_str, test_amount));
            }

            plan
        }; // Read lock released

        // Execute swaps
        for (from_str, to_str, amount) in swap_plan {
            info!(
                from = %from_str,
                to = %to_str,
                amount = %amount.0,
                "Attempting to swap collateral"
            );

            // Parse assets (both NEP-141 and NEP-245 are supported via intelligent routing)
            match (
                from_str.parse::<FungibleAsset<CollateralAsset>>(),
                to_str.parse::<FungibleAsset<BorrowAsset>>(),
            ) {
                (Ok(from_asset), Ok(to_asset)) => {
                    self.execute_swap(&from_asset, &to_asset, amount).await;
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

    /// Execute a single swap with metrics tracking (generic over asset types)
    ///
    /// Intelligently routes to the correct provider:
    /// - NEP-141 → NEP-141: Uses Ref Finance (may map native tokens to bridged equivalents)
    /// - NEP-245 → NEP-245: Uses 1-Click API
    async fn execute_swap<F, T>(
        &mut self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        amount: U128,
    ) where
        F: AssetClass,
        T: AssetClass,
    {
        self.metrics.swaps_attempted += 1;
        let swap_start = Instant::now();

        // Select the appropriate swap provider based on asset types
        let (swap_provider, provider_name) = match self.select_provider(from_asset, to_asset) {
            Some(provider) => {
                let name = provider.provider_name();
                (provider, name)
            }
            None => {
                self.metrics.swaps_failed += 1;
                info!(
                    from = %from_asset,
                    to = %to_asset,
                    "No swap provider available - collateral will be held in inventory"
                );
                return;
            }
        };

        info!(
            from = %from_asset,
            to = %to_asset,
            amount = %amount.0,
            provider = %provider_name,
            "Starting swap execution"
        );

        // Double-check provider supports these assets
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

        // TEMPORARY: Skip dry-run check for swap testing
        // TODO: Re-enable after testing
        // if self.dry_run {
        //     info!("[DRY RUN] Would swap {} to {}", from_asset, to_asset);
        //     return;
        // }

        // Get quote or use full amount for input-based swaps (Ref Finance)
        let input_amount = if swap_provider.provider_name() == "RefFinance" {
            // Ref Finance swaps based on input amount, not output
            // Just use the full available amount
            info!(
                from = %from_asset,
                to = %to_asset,
                amount = %amount.0,
                "Using full available amount for input-based swap (RefFinance)"
            );
            amount
        } else {
            // For output-based swaps (like 1-Click), get quote for required input
            match swap_provider.quote(from_asset, to_asset, amount).await {
                Ok(input) => {
                    info!(
                        from = %from_asset,
                        to = %to_asset,
                        input_amount = %input.0,
                        output_amount = %amount.0,
                        "Quote received"
                    );
                    input
                }
                Err(e) => {
                    self.metrics.swaps_failed += 1;
                    error!(
                        from = %from_asset,
                        to = %to_asset,
                        error = %e,
                        "Failed to get swap quote"
                    );
                    return;
                }
            }
        };

        // Execute swap
        match swap_provider.swap(from_asset, to_asset, input_amount).await {
            Ok(FinalExecutionStatus::SuccessValue(_)) => {
                let latency = swap_start.elapsed().as_millis();
                self.metrics.swaps_successful += 1;
                self.metrics.total_input_amount += input_amount.0;
                self.metrics.total_output_amount += amount.0;
                self.metrics.total_latency_ms += latency;

                info!(
                    from = %from_asset,
                    to = %to_asset,
                    input = %input_amount.0,
                    output = %amount.0,
                    latency_ms = latency,
                    "Swap completed successfully"
                );

                // Clear liquidation history for this collateral
                self.inventory
                    .write()
                    .await
                    .clear_liquidation_history(&from_asset.to_string());
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

    /// Selects the appropriate swap provider based on asset types.
    ///
    /// Routing logic:
    /// - Both NEP-141: Use Ref Finance (NEAR-native DEX)
    /// - Both NEP-245: Use 1-Click API (cross-chain via Intents)
    /// - Mixed: Not supported
    ///
    /// Note: For Ref Finance, native NEAR tokens are automatically mapped to their
    /// bridged equivalents (e.g., Circle USDC → Bridged USDC from Ethereum)
    fn select_provider<F, T>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
    ) -> Option<&SwapProviderImpl>
    where
        F: AssetClass,
        T: AssetClass,
    {
        let from_is_nep141 = from_asset.clone().into_nep141().is_some();
        let from_is_nep245 = from_asset.clone().into_nep245().is_some();
        let to_is_nep141 = to_asset.clone().into_nep141().is_some();
        let to_is_nep245 = to_asset.clone().into_nep245().is_some();

        match (from_is_nep141, from_is_nep245, to_is_nep141, to_is_nep245) {
            // NEP-141 → NEP-141: Check if Ref Finance supports these specific tokens
            (true, false, true, false) => {
                // Ref Finance smart router can handle any NEP-141 token pair
                // It will find routes automatically or fail if no pools exist
                debug!(
                    from = %from_asset,
                    to = %to_asset,
                    "Routing NEP-141 → NEP-141 swap to Ref Finance smart router"
                );
                self.ref_provider.as_ref()
            }
            // NEP-245 → NEP-245: Use 1-Click
            (false, true, false, true) => {
                debug!(
                    from = %from_asset,
                    to = %to_asset,
                    "Routing NEP-245 → NEP-245 swap to 1-Click API"
                );
                self.oneclick_provider.as_ref()
            }
            // Mixed or unsupported
            _ => {
                warn!(
                    from = %from_asset,
                    to = %to_asset,
                    from_nep141 = from_is_nep141,
                    from_nep245 = from_is_nep245,
                    to_nep141 = to_is_nep141,
                    to_nep245 = to_is_nep245,
                    "Unsupported asset type combination for swap"
                );
                None
            }
        }
    }
}
