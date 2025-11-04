// SPDX-License-Identifier: MIT
//! Liquidator service lifecycle management.
//!
//! This module handles the bot's main operational loop including:
//! - Registry refresh (discovering and validating markets)
//! - Inventory refresh (updating asset balances)
//! - Liquidation rounds (scanning and executing liquidations)

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use near_crypto::{InMemorySigner, Signer};
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::AccountId;
use tokio::{sync::RwLock, time::sleep};
use tracing::Instrument;

use crate::{
    inventory::InventoryManager,
    liquidation_strategy::LiquidationStrategy,
    rebalancer::InventoryRebalancer,
    rpc::{list_all_deployments, view, Network},
    CollateralStrategy, Liquidator, LiquidatorError,
};

/// Configuration for the liquidator service
#[derive(Debug)]
pub struct ServiceConfig {
    /// Market registries to monitor
    pub registries: Vec<AccountId>,
    /// Signer key for transactions
    pub signer_key: near_crypto::SecretKey,
    /// Signer account ID
    pub signer_account: AccountId,
    /// Network to operate on
    pub network: Network,
    /// Custom RPC URL (overrides default network RPC)
    pub rpc_url: Option<String>,
    /// Transaction timeout in seconds
    pub transaction_timeout: u64,
    /// Interval between liquidation scans in seconds
    pub liquidation_scan_interval: u64,
    /// Registry refresh interval in seconds
    pub registry_refresh_interval: u64,
    /// Concurrency for liquidations
    pub concurrency: usize,
    /// Liquidation strategy
    pub strategy: Arc<dyn LiquidationStrategy>,
    /// Collateral strategy
    pub collateral_strategy: CollateralStrategy,
    /// Dry run mode - scan without executing
    pub dry_run: bool,
    /// `OneClick` API token (for cross-chain NEP-245 swaps)
    pub oneclick_api_token: Option<String>,
    /// Ref Finance contract address (for NEAR-native NEP-141 swaps)
    pub ref_contract: Option<String>,
}

/// Liquidator service that manages the bot lifecycle
pub struct LiquidatorService {
    config: ServiceConfig,
    client: JsonRpcClient,
    signer: Signer,
    inventory: Arc<RwLock<InventoryManager>>,
    markets: HashMap<AccountId, Liquidator>,
    ref_provider: Option<crate::swap::SwapProviderImpl>,
    oneclick_provider: Option<crate::swap::SwapProviderImpl>,
    rebalancer: InventoryRebalancer,
}

impl LiquidatorService {
    /// Create a new liquidator service
    pub fn new(config: ServiceConfig) -> Self {
        let rpc_url = config
            .rpc_url
            .as_deref()
            .unwrap_or_else(|| config.network.rpc_url());

        tracing::info!(rpc_url = %rpc_url, "Connecting to RPC");

        let client = JsonRpcClient::connect(rpc_url);
        let signer = InMemorySigner::from_secret_key(
            config.signer_account.clone(),
            config.signer_key.clone(),
        );

        let inventory = Arc::new(RwLock::new(InventoryManager::new(
            client.clone(),
            config.signer_account.clone(),
        )));

        // Create both swap providers for intelligent routing
        let (ref_provider, oneclick_provider) = Self::create_swap_providers(&config, &client, Arc::new(signer.clone()));

        // Create inventory rebalancer with both providers
        let rebalancer = InventoryRebalancer::new(
            inventory.clone(),
            ref_provider.clone(),
            oneclick_provider.clone(),
            config.collateral_strategy.clone(),
            config.dry_run,
        );

        Self {
            config,
            client,
            signer,
            inventory,
            markets: HashMap::new(),
            ref_provider,
            oneclick_provider,
            rebalancer,
        }
    }

    /// Creates both swap providers (Ref Finance for NEP-141, OneClick for NEP-245).
    fn create_swap_providers(
        config: &ServiceConfig,
        client: &JsonRpcClient,
        signer: Arc<near_crypto::Signer>,
    ) -> (Option<crate::swap::SwapProviderImpl>, Option<crate::swap::SwapProviderImpl>) {
        use crate::swap::{OneClickSwap, RefSwap, SwapProviderImpl};

        // If Hold strategy, no swap providers needed
        if matches!(config.collateral_strategy, CollateralStrategy::Hold) {
            tracing::info!("Collateral strategy is Hold, no swap providers needed");
            return (None, None);
        }

        tracing::info!("Creating swap providers for intelligent routing");

        // Create Ref Finance provider for NEP-141 tokens
        let ref_provider = if let Some(ref contract_str) = config.ref_contract {
            match contract_str.parse::<AccountId>() {
                Ok(contract) => {
                    let ref_swap = RefSwap::new(contract.clone(), client.clone(), signer.clone());
                    tracing::info!(
                        contract = %contract,
                        "Ref Finance provider created for NEP-141 tokens (stNEAR, USDC, etc.)"
                    );
                    Some(SwapProviderImpl::ref_finance(ref_swap))
                }
                Err(e) => {
                    tracing::error!(
                        contract = %contract_str,
                        error = ?e,
                        "Invalid REF_CONTRACT address"
                    );
                    None
                }
            }
        } else {
            tracing::warn!(
                "REF_CONTRACT not configured - NEP-141 collateral (stNEAR, etc.) will be held, not swapped\n\
                 Set REF_CONTRACT=v2.ref-finance.near (mainnet) or v2.ref-labs.near (testnet)"
            );
            None
        };

        // Create OneClick provider for NEP-245 tokens
        let oneclick_provider = {
            let oneclick = OneClickSwap::new(
                client.clone(),
                signer,
                None, // Use default slippage
                config.oneclick_api_token.clone(),
            );
            if config.oneclick_api_token.is_some() {
                tracing::info!("1-Click API provider created with authentication (for NEP-245 tokens, no fee)");
            } else {
                tracing::warn!("1-Click API provider created WITHOUT authentication (for NEP-245 tokens, 0.1% fee applies)");
            }
            Some(SwapProviderImpl::oneclick(oneclick))
        };

        (ref_provider, oneclick_provider)
    }

    /// Run the service event loop
    pub async fn run(mut self) {
        let registry_refresh_interval = Duration::from_secs(self.config.registry_refresh_interval);

        let mut next_registry_refresh = Instant::now();

        loop {
            // Refresh market registry
            if Instant::now() >= next_registry_refresh {
                match self.refresh_registry().await {
                    Ok(()) => {
                        tracing::info!("Registry refresh completed successfully");
                        next_registry_refresh = Instant::now() + registry_refresh_interval;
                    }
                    Err(e) => {
                        if is_rate_limit_error(&e) {
                            tracing::error!(
                                error = %e,
                                "Rate limit hit during registry refresh, will retry in 60 seconds"
                            );
                            next_registry_refresh = Instant::now() + Duration::from_secs(60);
                        } else {
                            tracing::error!(
                                error = %e,
                                "Registry refresh failed, will retry in 5 minutes"
                            );
                            next_registry_refresh = Instant::now() + Duration::from_secs(300);
                        }

                        if self.markets.is_empty() {
                            tracing::warn!("No markets available yet, waiting before retry");
                            sleep(Duration::from_secs(10)).await;
                            continue;
                        }
                    }
                }
            }

            // Refresh borrow asset inventory before liquidations
            self.refresh_inventory().await;

            // Run liquidation round
            self.run_liquidation_round().await;

            // Refresh collateral inventory after liquidations (may have received collateral)
            match self.inventory.write().await.refresh_collateral().await {
                Ok(balances) => {
                    let count = balances.len();
                    if count > 0 {
                        tracing::info!(
                            collateral_asset_count = count,
                            "Collateral inventory refreshed after liquidation round"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = ?e,
                        "Failed to refresh collateral inventory"
                    );
                }
            }

            // After liquidation round, rebalance inventory based on collateral strategy
            let market_refs: Vec<&Liquidator> = self.markets.values().collect();
            self.rebalancer.rebalance(&market_refs).await;

            tracing::info!(
                interval_seconds = self.config.liquidation_scan_interval,
                "Liquidation round completed, sleeping before next run"
            );
            sleep(Duration::from_secs(self.config.liquidation_scan_interval)).await;
        }
    }

    /// Refresh the market registry (discover and validate markets)
    #[allow(clippy::too_many_lines)]
    async fn refresh_registry(&mut self) -> Result<(), LiquidatorError> {
        let refresh_span = tracing::debug_span!("registry_refresh");

        async {
            tracing::info!("Refreshing registry deployments");

            let all_markets = list_all_deployments(
                self.client.clone(),
                self.config.registries.clone(),
                self.config.concurrency,
            )
            .await
            .map_err(LiquidatorError::ListDeploymentsError)?;

            tracing::info!(
                market_count = all_markets.len(),
                markets = ?all_markets,
                "Found deployments from registries"
            );

            // Fetch configurations for all markets
            let mut market_configs = Vec::new();
            for market in &all_markets {
                // First check contract version using NEP-330
                let version_result = crate::rpc::get_contract_version(&self.client, market).await;
                
                if let Some(version) = version_result {
                    // Parse semver (e.g., "1.2.3" or "0.1.0")
                    let parts: Vec<&str> = version.split('.').collect();
                    let is_supported = if let [maj, min, _patch] = parts.as_slice() {
                        let major = maj.parse::<u32>().unwrap_or(0);
                        let minor = min.parse::<u32>().unwrap_or(0);
                        // Require version >= 1.1.0 (when price_oracle_configuration was added)
                        (major, minor) >= (1, 1)
                    } else {
                        tracing::warn!(
                            market = %market,
                            version = %version,
                            "Invalid semver format, skipping"
                        );
                        false
                    };
                    
                    if !is_supported {
                        tracing::info!(
                            market = %market,
                            version = %version,
                            min_version = "1.1.0",
                            "Skipping market - unsupported contract version"
                        );
                        continue;
                    }
                } else {
                    tracing::info!(
                        market = %market,
                        "Contract does not implement NEP-330 (contract_source_metadata), skipping"
                    );
                    continue;
                }
                
                // Now fetch configuration
                match view::<templar_common::market::MarketConfiguration>(
                    &self.client,
                    market.clone(),
                    "get_configuration",
                    serde_json::json!({}),
                )
                .await
                {
                    Ok(config) => {
                        tracing::debug!(
                            market = %market,
                            borrow_asset = %config.borrow_asset,
                            collateral_asset = %config.collateral_asset,
                            "Fetched market configuration"
                        );
                        market_configs.push((market.clone(), config));
                    }
                    Err(e) => {
                        tracing::warn!(
                            market = %market,
                            error = ?e,
                            "Failed to fetch market configuration, skipping"
                        );
                    }
                }
            }

            // Discover assets from all market configurations
            {
                let mut inventory_guard = self.inventory.write().await;
                inventory_guard.discover_assets(market_configs.iter().map(|(_, config)| config));
                inventory_guard
                    .discover_collateral_assets(market_configs.iter().map(|(_, config)| config));
            }

            // Create liquidators for each market
            let mut supported_markets = HashMap::new();
            let mut unsupported_markets = Vec::new();

            for (market, config) in market_configs {
                tracing::debug!(market = %market, "Creating liquidator for market");

                // Clone Signer enum
                let signer = Arc::new(self.signer.clone());

                let liquidator = Liquidator::new(
                    &self.client,
                    signer,
                    &self.inventory,
                    market.clone(),
                    config,
                    self.config.strategy.clone(),
                    self.config.collateral_strategy.clone(),
                    self.config.transaction_timeout,
                    self.config.dry_run,
                    None, // Swapping is now handled by rebalancer post-liquidation
                );

                // Test market compatibility using scanner
                match liquidator.scanner().test_market_compatibility().await {
                    Ok(()) => {
                        supported_markets.insert(market, liquidator);
                    }
                    Err(_) => {
                        unsupported_markets.push(market);
                    }
                }
            }

            if !unsupported_markets.is_empty() {
                tracing::debug!(
                    unsupported_count = unsupported_markets.len(),
                    unsupported = ?unsupported_markets,
                    "Filtered out unsupported markets"
                );
            }

            self.markets = supported_markets;
            Ok(())
        }
        .instrument(refresh_span)
        .await
    }

    /// Refresh inventory balances
    async fn refresh_inventory(&self) {
        let inventory_span = tracing::debug_span!("inventory_refresh");

        async {
            // Refresh borrow assets
            match self.inventory.write().await.refresh().await {
                Ok(refreshed) => {
                    tracing::debug!(refreshed_count = refreshed, "Borrow inventory refresh completed");
                }
                Err(e) => {
                    tracing::warn!(
                        error = ?e,
                        "Failed to refresh borrow inventory"
                    );
                }
            }
        }
        .instrument(inventory_span)
        .await;
    }


    /// Run a single liquidation round across all markets
    async fn run_liquidation_round(&self) {
        let liquidation_span = tracing::debug_span!("liquidation_round");

        async {
            for (i, (market, liquidator)) in self.markets.iter().enumerate() {
                let market_span = tracing::debug_span!("market", market = %market);

                let result = async {
                    tracing::info!(market = %market, "Scanning market for liquidations");
                    liquidator.run_liquidations(self.config.concurrency).await
                }
                .instrument(market_span)
                .await;

                // Handle errors gracefully
                match result {
                    Ok(()) => {
                        tracing::info!(market = %market, "Market scan completed");
                    }
                    Err(e) => {
                        if is_rate_limit_error(&e) {
                            tracing::error!(
                                market = %market,
                                error = %e,
                                "Rate limit hit while scanning market, sleeping 60 seconds before continuing"
                            );
                            sleep(Duration::from_secs(60)).await;
                        } else {
                            tracing::error!(
                                market = %market,
                                error = %e,
                                "Failed to scan market, continuing to next market"
                            );
                        }
                    }
                }

                // Add delay between markets to avoid rate limiting (except after last market)
                if i < self.markets.len() - 1 {
                    let delay_seconds = 5;
                    tracing::debug!(
                        "Waiting {}s before next market to avoid rate limits",
                        delay_seconds
                    );
                    sleep(Duration::from_secs(delay_seconds)).await;
                }
            }
        }
        .instrument(liquidation_span)
        .await;
    }
}

/// Check if an error is a rate limit error
fn is_rate_limit_error(error: &LiquidatorError) -> bool {
    let error_msg = error.to_string();
    error_msg.contains("TooManyRequests")
        || error_msg.contains("429")
        || error_msg.contains("rate limit")
}
