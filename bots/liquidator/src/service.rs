//! Liquidator service lifecycle management.
//!
//! This module handles the bot's main operational loop including:
//! - Registry refresh (discovering and validating markets)
//! - Inventory refresh (updating asset balances)
//! - Liquidation rounds (scanning and executing liquidations)

use std::{collections::HashMap, sync::Arc, time::Duration};

use near_crypto::{InMemorySigner, Signer};
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::AccountId;
use tokio::{
    select,
    sync::RwLock,
    time::{interval, sleep, Duration as TokioDuration, MissedTickBehavior},
};
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
    /// RPC URL
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
    /// `OneClick` API token for swap authentication
    pub oneclick_api_token: Option<String>,
    /// Ref Finance contract address for NEP-141 swaps
    pub ref_contract: Option<String>,
    /// Collateral asset allowlist for market filtering
    pub allowed_collateral_assets: Vec<String>,
    /// Collateral assets to ignore in market filtering
    pub ignored_collateral_assets: Vec<String>,
}

/// Liquidator service that manages the bot lifecycle
pub struct LiquidatorService {
    config: ServiceConfig,
    client: JsonRpcClient,
    signer: Signer,
    inventory: Arc<RwLock<InventoryManager>>,
    markets: HashMap<AccountId, Liquidator>,
    /// Swap provider used by rebalancer
    #[allow(dead_code)]
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

        // Create swap provider for rebalancer
        let (_, oneclick_provider) =
            Self::create_swap_providers(&config, &client, Arc::new(signer.clone()));

        // Initialize rebalancer with swap provider
        let rebalancer = InventoryRebalancer::new(
            inventory.clone(),
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
            oneclick_provider,
            rebalancer,
        }
    }

    /// Creates swap providers for collateral rebalancing
    fn create_swap_providers(
        config: &ServiceConfig,
        client: &JsonRpcClient,
        signer: Arc<near_crypto::Signer>,
    ) -> (
        Option<crate::swap::SwapProviderImpl>,
        Option<crate::swap::SwapProviderImpl>,
    ) {
        use crate::swap::{OneClickSwap, RefSwap, SwapProviderImpl};

        // No swap providers needed for Hold strategy
        if matches!(config.collateral_strategy, CollateralStrategy::Hold) {
            tracing::info!("Collateral strategy is Hold, no swap providers needed");
            return (None, None);
        }

        tracing::info!("Creating swap providers for collateral rebalancing");

        // Initialize Ref Finance provider for NEP-141 tokens
        let ref_provider = if let Some(ref contract_str) = config.ref_contract {
            match contract_str.parse::<AccountId>() {
                Ok(contract) => {
                    let ref_swap = RefSwap::new(contract.clone(), client.clone(), signer.clone());
                    tracing::info!(
                        contract = %contract,
                        "Ref Finance provider initialized"
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
                "REF_CONTRACT not configured - set to v2.ref-finance.near (mainnet) or ref-finance-101.testnet"
            );
            None
        };

        // Initialize OneClick provider for NEP-245 and NEP-141 tokens
        let oneclick_provider = {
            let oneclick = OneClickSwap::new(
                client.clone(),
                signer,
                None,
                config.oneclick_api_token.clone(),
            );
            if config.oneclick_api_token.is_some() {
                tracing::info!("1-Click API provider initialized with authentication");
            } else {
                tracing::warn!(
                    "1-Click API provider initialized without authentication (0.1% fee applies)"
                );
            }
            Some(SwapProviderImpl::oneclick(oneclick))
        };

        (ref_provider, oneclick_provider)
    }

    /// Run the service event loop
    pub async fn run(mut self) {
        // Create intervals for periodic tasks
        let mut registry_interval = interval(TokioDuration::from_secs(
            self.config.registry_refresh_interval,
        ));
        registry_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut liquidation_interval = interval(TokioDuration::from_secs(
            self.config.liquidation_scan_interval,
        ));
        liquidation_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        // Run initial registry refresh immediately
        match self.refresh_registry().await {
            Ok(()) => {
                tracing::info!("Initial registry refresh completed successfully");
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "Initial registry refresh failed, will retry later"
                );
            }
        }

        // Reset the registry interval to start timing from now
        registry_interval.reset();

        loop {
            select! {
                _ = registry_interval.tick() => {
                    match self.refresh_registry().await {
                        Ok(()) => {
                            tracing::info!("Registry refresh completed successfully");
                        }
                        Err(e) => {
                            if is_rate_limit_error(&e) {
                                tracing::error!(
                                    error = %e,
                                    "Rate limit hit during registry refresh, will retry in 60 seconds"
                                );
                                // Reset interval to retry in 60 seconds
                                registry_interval.reset_after(TokioDuration::from_secs(60));
                            } else {
                                tracing::error!(
                                    error = %e,
                                    "Registry refresh failed, will retry in 5 minutes"
                                );
                                // Reset interval to retry in 5 minutes
                                registry_interval.reset_after(TokioDuration::from_secs(300));
                            }

                            if self.markets.is_empty() {
                                tracing::warn!("No markets available yet, skipping liquidation round");
                                continue;
                            }
                        }
                    }
                }
                _ = liquidation_interval.tick() => {
                    // Refresh borrow asset inventory before liquidations
                    self.refresh_inventory().await;

                    // Run liquidation round
                    self.run_liquidation_round().await;

                    // Refresh collateral inventory after liquidations
                    match self.inventory.write().await.refresh_collateral().await {
                        Ok(balances) => {
                            let count = balances.len();
                            if count > 0 {
                                tracing::info!(
                                    collateral_asset_count = count,
                                    "Collateral inventory refreshed"
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

                    // Rebalance inventory based on collateral strategy
                    self.rebalancer.rebalance().await;

                    tracing::info!(
                        interval_seconds = self.config.liquidation_scan_interval,
                        "Liquidation round completed"
                    );
                }
            }
        }
    }

    /// Check if a market should be processed based on asset filtering rules.
    ///
    /// Returns (`should_process`, `reason_if_filtered`).
    fn should_process_market(
        &self,
        config: &templar_common::market::MarketConfiguration,
    ) -> (bool, Option<String>) {
        let collateral_str = config.collateral_asset.to_string();

        // Helper to extract underlying token from NEP-245 wrappers
        let asset_matches = |asset: &str, pattern: &str| -> bool {
            if asset == pattern {
                return true;
            }

            // NEP-245 format: nep245:contract:token_id
            // Extract underlying token (e.g., nep141:btc.omft.near from nep245:intents.near:nep141:btc.omft.near)
            if asset.starts_with("nep245:") {
                if let Some(token_id_start) = asset.find(':').and_then(|first| {
                    asset[first + 1..]
                        .find(':')
                        .map(|second| first + 1 + second + 1)
                }) {
                    return &asset[token_id_start..] == pattern;
                }
            }

            false
        };

        // Check ignore list
        if !self.config.ignored_collateral_assets.is_empty() {
            for ignored_asset in &self.config.ignored_collateral_assets {
                if asset_matches(&collateral_str, ignored_asset) {
                    return (
                        false,
                        Some(format!(
                            "collateral '{collateral_str}' matches ignore pattern '{ignored_asset}'"
                        )),
                    );
                }
            }
        }

        // Check allow list
        if !self.config.allowed_collateral_assets.is_empty() {
            let is_allowed = self
                .config
                .allowed_collateral_assets
                .iter()
                .any(|allowed_asset| asset_matches(&collateral_str, allowed_asset));

            if !is_allowed {
                return (
                    false,
                    Some(format!("collateral '{collateral_str}' not in allowlist")),
                );
            }
        }

        (true, None)
    }

    /// Refresh the market registry
    #[allow(clippy::too_many_lines)]
    async fn refresh_registry(&mut self) -> Result<(), LiquidatorError> {
        let refresh_span = tracing::debug_span!("registry_refresh");

        async {
            tracing::info!(
                registries = ?self.config.registries,
                "Refreshing registry deployments"
            );

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
                // Check contract version using NEP-330
                let version_result = crate::rpc::get_contract_version(&self.client, market).await;

                if let Some(version) = version_result {
                    // Parse semver and verify compatibility
                    let parts: Vec<&str> = version.split('.').collect();
                    let is_supported = if let [maj, min, _patch] = parts.as_slice() {
                        let major = maj.parse::<u32>().unwrap_or(0);
                        let minor = min.parse::<u32>().unwrap_or(0);
                        (major, minor) >= (1, 0)
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
                            min_required = "1.0.0",
                            "Skipping market - unsupported version"
                        );
                        continue;
                    }
                } else {
                    tracing::info!(
                        market = %market,
                        "Contract missing NEP-330 metadata, skipping"
                    );
                    continue;
                }

                // Fetch market configuration
                match view::<templar_common::market::MarketConfiguration>(
                    &self.client,
                    market.clone(),
                    "get_configuration",
                    near_sdk::serde_json::json!({}),
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

                        // Apply market filtering rules
                        let (should_process, filter_reason) = self.should_process_market(&config);

                        if should_process {
                            market_configs.push((market.clone(), config));
                        } else {
                            tracing::info!(
                                market = %market,
                                collateral_asset = %config.collateral_asset,
                                reason = filter_reason.unwrap_or_default(),
                                "Market filtered out"
                            );
                        }
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
                    None,
                );

                // Test market compatibility
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
                    tracing::debug!(
                        refreshed_count = refreshed,
                        "Borrow inventory refresh completed"
                    );
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
