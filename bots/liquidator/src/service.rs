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
    /// Inventory refresh interval in seconds
    pub inventory_refresh_interval: u64,
    /// Concurrency for liquidations
    pub concurrency: usize,
    /// Liquidation strategy
    pub strategy: Arc<dyn LiquidationStrategy>,
    /// Collateral strategy
    pub collateral_strategy: CollateralStrategy,
    /// Dry run mode - scan without executing
    pub dry_run: bool,
    /// Swap provider for collateral swaps
    pub swap_provider: String,
    /// `OneClick` API token
    pub oneclick_api_token: Option<String>,
    /// Rhea contract address
    pub rhea_contract: Option<String>,
}

/// Liquidator service that manages the bot lifecycle
pub struct LiquidatorService {
    config: ServiceConfig,
    client: JsonRpcClient,
    signer: Signer,
    inventory: Arc<RwLock<InventoryManager>>,
    markets: HashMap<AccountId, Liquidator>,
    swap_provider: Option<crate::swap::SwapProviderImpl>,
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

        // Create swap provider based on configuration
        let swap_provider = Self::create_swap_provider(&config, &client, Arc::new(signer.clone()));

        Self {
            config,
            client,
            signer,
            inventory,
            markets: HashMap::new(),
            swap_provider,
        }
    }

    /// Creates a swap provider based on configuration.
    fn create_swap_provider(
        config: &ServiceConfig,
        client: &JsonRpcClient,
        signer: Arc<near_crypto::Signer>,
    ) -> Option<crate::swap::SwapProviderImpl> {
        use crate::swap::{OneClickSwap, RheaSwap, SwapProviderImpl};

        // Only create swap provider if not using Hold strategy
        if matches!(config.collateral_strategy, CollateralStrategy::Hold) {
            tracing::info!("Collateral strategy is Hold, no swap provider needed");
            return None;
        }

        tracing::info!(
            swap_provider = %config.swap_provider,
            "Creating swap provider for collateral strategy"
        );

        match config.swap_provider.to_lowercase().as_str() {
            "oneclick" => {
                if let Some(ref api_token) = config.oneclick_api_token {
                    let oneclick = OneClickSwap::new(
                        client.clone(),
                        signer,
                        None, // Use default slippage
                        Some(api_token.clone()),
                    );
                    tracing::info!("Using 1-Click API swap provider");
                    Some(SwapProviderImpl::oneclick(oneclick))
                } else {
                    tracing::error!(
                        "OneClick provider selected but ONECLICK_API_TOKEN not provided"
                    );
                    None
                }
            }
            "rhea" => {
                if let Some(ref contract_str) = config.rhea_contract {
                    match contract_str.parse::<AccountId>() {
                        Ok(contract) => {
                            let rhea = RheaSwap::new(contract, client.clone(), signer);
                            tracing::info!(contract = %contract_str, "Using Rhea Finance swap provider");
                            Some(SwapProviderImpl::rhea(rhea))
                        }
                        Err(e) => {
                            tracing::error!(
                                contract = %contract_str,
                                error = ?e,
                                "Invalid RHEA_CONTRACT"
                            );
                            None
                        }
                    }
                } else {
                    tracing::error!("Rhea provider selected but RHEA_CONTRACT not provided");
                    None
                }
            }
            other => {
                tracing::error!(
                    provider = other,
                    "Invalid swap provider, must be 'oneclick' or 'rhea'"
                );
                None
            }
        }
    }

    /// Run the service event loop
    pub async fn run(mut self) {
        let registry_refresh_interval = Duration::from_secs(self.config.registry_refresh_interval);
        let inventory_refresh_interval =
            Duration::from_secs(self.config.inventory_refresh_interval);

        let mut next_registry_refresh = Instant::now();
        let mut next_inventory_refresh = Instant::now();

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

            // Refresh inventory
            if Instant::now() >= next_inventory_refresh {
                self.refresh_inventory().await;
                next_inventory_refresh = Instant::now() + inventory_refresh_interval;
            }

            // Run liquidation round
            self.run_liquidation_round().await;

            tracing::info!(
                interval_seconds = self.config.liquidation_scan_interval,
                "Liquidation round completed, sleeping before next run"
            );
            sleep(Duration::from_secs(self.config.liquidation_scan_interval)).await;
        }
    }

    /// Refresh the market registry (discover and validate markets)
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
                    self.swap_provider.clone(),
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

            tracing::info!(
                supported_count = supported_markets.len(),
                supported = ?supported_markets.keys().collect::<Vec<_>>(),
                "Active markets to monitor"
            );

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
            match self.inventory.write().await.refresh().await {
                Ok(refreshed) => {
                    tracing::debug!(refreshed_count = refreshed, "Inventory refresh completed");
                }
                Err(e) => {
                    tracing::warn!(
                        error = ?e,
                        "Failed to refresh inventory"
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
