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
use templar_common::utils::Network;
use tokio::{
    select,
    sync::RwLock,
    time::{interval, sleep, Duration as TokioDuration, MissedTickBehavior},
};
use tracing::Instrument;

use templar_common::{
    asset::{BorrowAsset, FungibleAsset},
    oracle::pyth::PriceIdentifier,
};

use crate::{
    inventory::InventoryManager,
    liquidation_strategy::LiquidationStrategy,
    rpc::{list_all_deployments, view},
    swap::SwapProvider,
    CollateralStrategy, Liquidator, LiquidatorError,
};

/// Information needed to price a collateral asset and determine its swap target.
#[derive(Debug, Clone)]
pub struct CollateralPriceInfo {
    /// Oracle account to query for price
    pub oracle_account: AccountId,
    /// Price identifier for this collateral asset in the oracle
    pub price_id: PriceIdentifier,
    /// Decimals for amount conversion (from oracle config)
    pub decimals: i32,
    /// Target borrow asset to swap into (derived from market config)
    pub target_borrow_asset: FungibleAsset<BorrowAsset>,
}

/// Configuration for the liquidator service
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
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
    pub near_rpc_url: Option<String>,
    /// API key sent via X-API-Key header for RPC authentication
    pub near_api_key: Option<String>,
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
    pub allowed_collateral_assets:
        Vec<templar_common::asset::FungibleAsset<templar_common::asset::CollateralAsset>>,
    /// Collateral assets to ignore in market filtering
    pub ignored_collateral_assets:
        Vec<templar_common::asset::FungibleAsset<templar_common::asset::CollateralAsset>>,
    /// Enable loop liquidation - repeatedly liquidate until position is healthy
    pub loop_liquidation: bool,
    /// Maximum iterations for loop liquidation (safety limit)
    pub max_loop_iterations: u32,
    /// Pyth Hermes API URL for fetching price data
    pub hermes_url: String,
    /// RedStone gateway URL for fetching fresh prices
    pub redstone_gateway_url: String,
    /// Minimum USD value to attempt a swap (JIT or batch)
    pub min_swap_value_usd: f64,
    /// Enable batch swap of accumulated collateral at round start
    pub batch_swap_on_cycle_start: bool,
    /// Retry configuration for transient swap errors
    pub swap_retry_config: crate::swap::SwapRetryConfig,
    /// Shared notifier for Telegram alerts
    pub notifier: crate::notifier::SharedNotifier,
}

/// Liquidator service that manages the bot lifecycle
pub struct LiquidatorService {
    config: ServiceConfig,
    client: JsonRpcClient,
    signer: Signer,
    inventory: Arc<RwLock<InventoryManager>>,
    markets: HashMap<AccountId, Liquidator>,
    /// Swap provider passed to liquidators for immediate post-liquidation swaps
    oneclick_provider: Option<crate::swap::SwapProviderImpl>,
    /// Oracle fetcher for batch swap USD threshold checks
    oracle_fetcher: crate::OracleFetcher,
    /// Collateral asset → price / swap target info (built from market configs)
    collateral_price_map: HashMap<String, CollateralPriceInfo>,
    /// Shared nonce tracker for all transactions from this signer
    nonce_tracker: crate::rpc::NonceTracker,
}

impl LiquidatorService {
    /// Create a new liquidator service
    pub fn new(config: ServiceConfig) -> Self {
        let near_rpc_url = config
            .near_rpc_url
            .as_deref()
            .unwrap_or_else(|| config.network.rpc_url());

        tracing::info!(near_rpc_url = %near_rpc_url, "Connecting to RPC");

        let mut client = JsonRpcClient::connect(near_rpc_url);
        if let Some(ref api_key) = config.near_api_key {
            let header = near_jsonrpc_client::auth::ApiKey::new(api_key).unwrap_or_else(|e| {
                panic!("Invalid NEAR_API_KEY value: {e}");
            });
            client = client.header(header);
            tracing::info!("RPC API key configured via X-API-Key header");
        }
        let signer = InMemorySigner::from_secret_key(
            config.signer_account.clone(),
            config.signer_key.clone(),
        );

        let inventory = Arc::new(RwLock::new(InventoryManager::new(
            client.clone(),
            config.signer_account.clone(),
        )));

        // Shared nonce tracker for all transactions from this signer
        let nonce_tracker = crate::rpc::NonceTracker::default();

        // Create swap provider for executor
        let (_, oneclick_provider) =
            Self::create_swap_providers(&config, &client, Arc::new(signer.clone()), &nonce_tracker);

        // Create oracle fetcher for batch swap price checks (no signer needed for reads)
        let oracle_fetcher = crate::OracleFetcher::new(
            client.clone(),
            Some(config.hermes_url.clone()),
            Some(config.redstone_gateway_url.clone()),
            None,
            None,
            nonce_tracker.clone(),
        );

        // Log swap configuration
        tracing::info!(
            min_swap_usd = %config.min_swap_value_usd,
            batch_swap_enabled = %config.batch_swap_on_cycle_start,
            retry_attempts = %config.swap_retry_config.max_attempts,
            retry_base_delay_ms = %config.swap_retry_config.base_delay_ms,
            "Swap configuration loaded"
        );

        Self {
            config,
            client,
            signer,
            inventory,
            markets: HashMap::new(),
            oneclick_provider,
            oracle_fetcher,
            collateral_price_map: HashMap::new(),
            nonce_tracker,
        }
    }

    /// Creates swap providers for collateral rebalancing
    fn create_swap_providers(
        config: &ServiceConfig,
        client: &JsonRpcClient,
        signer: Arc<near_crypto::Signer>,
        nonce_tracker: &crate::rpc::NonceTracker,
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
                    let ref_swap = RefSwap::new(
                        contract.clone(),
                        client.clone(),
                        signer.clone(),
                        nonce_tracker.clone(),
                    );
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
                nonce_tracker.clone(),
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

        // Load 1-Click supported tokens cache
        if let Some(ref provider) = self.oneclick_provider {
            provider.load_supported_tokens().await;
        }

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
                            }
                        }
                    }
                }
                _ = liquidation_interval.tick() => {
                    // Refresh collateral inventory (detect accumulated dust from prior runs)
                    if let Err(e) = self.inventory.write().await.refresh_collateral().await {
                        tracing::warn!(error = ?e, "Failed to refresh collateral inventory before batch swap");
                    }

                    // Batch swap accumulated collateral before liquidation round
                    if self.config.batch_swap_on_cycle_start {
                        self.batch_swap_collateral().await;
                    }

                    // Refresh borrow asset inventory before liquidations
                    self.refresh_inventory().await;

                    // Run liquidation round
                    self.run_liquidation_round().await;

                    // Refresh collateral inventory after liquidations (for tracking)
                    match self.inventory.write().await.refresh_collateral().await {
                        Ok(balances) => {
                            let count = balances.len();
                            if count > 0 {
                                tracing::debug!(
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
        let collateral_asset = &config.collateral_asset;

        // Check ignore list
        if !self.config.ignored_collateral_assets.is_empty() {
            for ignored_asset in &self.config.ignored_collateral_assets {
                if collateral_asset == ignored_asset {
                    return (
                        false,
                        Some(format!("collateral '{collateral_asset}' is in ignore list")),
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
                .any(|allowed_asset| collateral_asset == allowed_asset);

            if !is_allowed {
                return (
                    false,
                    Some(format!("collateral '{collateral_asset}' not in allowlist")),
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

            // Filter deployments using registry metadata, then fetch market configs
            let mut market_configs = Vec::new();
            for market in &all_markets {
                // Step 1: Fetch market configuration — this is the definitive check
                // for whether a deployment is a market contract. Non-market contracts
                // (proxy-oracles, redstone-adapters) won't have this method.
                let config = match view::<templar_common::market::MarketConfiguration>(
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
                        config
                    }
                    Err(e) => {
                        let err_msg = format!("{e:?}");
                        if err_msg.contains("MethodNotFound")
                            || err_msg.contains("MethodResolveError")
                        {
                            tracing::info!(
                                deployment = %market,
                                "Skipping non-market deployment (no get_configuration method)"
                            );
                        } else {
                            tracing::warn!(
                                market = %market,
                                error = ?e,
                                "Failed to fetch market configuration, skipping"
                            );
                        }
                        continue;
                    }
                };

                // Step 2: Check contract version using NEP-330
                let version_result = crate::rpc::get_contract_version(&self.client, market).await;

                if let Some(version) = version_result {
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

                // Step 3: Detect proxy oracle via account name prefix convention
                let oracle_account = &config.price_oracle_configuration.account_id;
                self.oracle_fetcher
                    .detect_and_register_proxy_oracle(oracle_account)
                    .await;

                // Step 4: Apply market filtering rules (collateral asset allow/ignore lists)
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

                let mut liquidator = Liquidator::new(
                    &self.client,
                    signer,
                    &self.inventory,
                    market.clone(),
                    config,
                    self.config.strategy.clone(),
                    self.config.collateral_strategy.clone(),
                    self.config.transaction_timeout,
                    self.config.dry_run,
                    self.oneclick_provider.clone(),
                    self.config.loop_liquidation,
                    self.config.max_loop_iterations,
                    Some(self.config.hermes_url.clone()),
                    Some(self.config.redstone_gateway_url.clone()),
                    self.config.swap_retry_config.clone(),
                    self.config.min_swap_value_usd,
                    Some(self.oracle_fetcher.proxy_oracle_cache()),
                    Some((
                        self.config.signer_account.clone(),
                        self.config.signer_key.clone(),
                    )),
                    self.config.notifier.clone(),
                    self.nonce_tracker.clone(),
                );

                // Fetch market version for version-specific liquidation logic
                liquidator.fetch_market_version().await;

                // Test market compatibility
                match liquidator.scanner().check_market_compatibility().await {
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

            // Rebuild collateral price map from new market set
            self.collateral_price_map = Self::build_collateral_price_map(&self.markets);
            tracing::info!(
                collateral_assets = self.collateral_price_map.len(),
                "Built collateral price map"
            );

            // Refresh 1-Click supported tokens cache
            if let Some(ref provider) = self.oneclick_provider {
                provider.load_supported_tokens().await;
            }

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

    /// Build collateral price map from current market configurations.
    ///
    /// For each unique collateral asset, stores the oracle info and the preferred
    /// borrow asset to swap into (preferring USDC variants as they're most liquid).
    fn build_collateral_price_map(
        markets: &HashMap<AccountId, Liquidator>,
    ) -> HashMap<String, CollateralPriceInfo> {
        let mut map = HashMap::new();
        for liquidator in markets.values() {
            let config = liquidator.market_configuration();
            let collateral_key = config.collateral_asset.to_string();

            let existing_is_usdc = map
                .get(&collateral_key)
                .is_some_and(|info: &CollateralPriceInfo| is_usdc_asset(&info.target_borrow_asset));

            // Prefer USDC as target borrow asset when the same collateral appears in
            // multiple markets (most liquid for rebalancing).
            if !existing_is_usdc {
                map.insert(
                    collateral_key,
                    CollateralPriceInfo {
                        oracle_account: config.price_oracle_configuration.account_id.clone(),
                        price_id: config.price_oracle_configuration.collateral_asset_price_id,
                        decimals: config.price_oracle_configuration.collateral_asset_decimals,
                        target_borrow_asset: config.borrow_asset.clone(),
                    },
                );
            }
        }
        map
    }

    /// Fetch USD prices for a set of collateral assets, batching by oracle.
    async fn fetch_collateral_usd_prices(&self, asset_keys: &[String]) -> HashMap<String, f64> {
        // Group by oracle for efficient batching
        let mut oracle_to_assets: HashMap<AccountId, Vec<(String, PriceIdentifier)>> =
            HashMap::new();

        for key in asset_keys {
            if let Some(info) = self.collateral_price_map.get(key) {
                oracle_to_assets
                    .entry(info.oracle_account.clone())
                    .or_default()
                    .push((key.clone(), info.price_id));
            }
        }

        let mut prices = HashMap::new();

        for (oracle, assets) in oracle_to_assets {
            let price_ids: Vec<_> = assets.iter().map(|(_, id)| *id).collect();
            let response = self
                .oracle_fetcher
                .get_oracle_prices(oracle.clone(), &price_ids, 120)
                .await;

            let response = match response {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        oracle = %oracle,
                        error = ?e,
                        "Failed to fetch collateral prices from oracle"
                    );
                    continue;
                }
            };

            for (asset_key, price_id) in assets {
                if let Some(Some(price)) = response.get(&price_id) {
                    #[allow(clippy::cast_precision_loss)]
                    let usd_price = (price.price.0 as f64) * 10f64.powi(price.expo);
                    prices.insert(asset_key, usd_price);
                }
            }
        }

        prices
    }

    /// Calculate USD value of a raw collateral amount.
    fn collateral_usd_value(&self, asset_key: &str, raw_amount: u128, usd_price: f64) -> f64 {
        let decimals = self
            .collateral_price_map
            .get(asset_key)
            .map_or(8, |i| i.decimals);

        #[allow(clippy::cast_precision_loss)]
        let amount = (raw_amount as f64) / 10f64.powi(decimals);
        amount * usd_price
    }

    /// Attempt to batch-swap accumulated collateral holdings at cycle start.
    ///
    /// For each collateral asset with non-zero balance:
    /// 1. Fetch its USD price from the oracle
    /// 2. Skip if below `min_swap_value_usd`
    /// 3. Swap to the target borrow asset with retry
    ///
    /// In dry-run mode, logs what would be swapped without executing.
    #[allow(clippy::too_many_lines)]
    async fn batch_swap_collateral(&self) {
        let holdings = self.inventory.read().await.collateral_holdings();
        if holdings.is_empty() {
            tracing::debug!("No collateral holdings to batch swap");
            return;
        }

        let asset_keys: Vec<String> = holdings.iter().map(|(a, _)| a.to_string()).collect();
        let prices = self.fetch_collateral_usd_prices(&asset_keys).await;

        let dry_run = self.config.dry_run;
        let retry_config = &self.config.swap_retry_config;
        let mut swapped = 0u32;
        let mut skipped = 0u32;

        for (collateral_asset, balance) in &holdings {
            let asset_key = collateral_asset.to_string();

            // Lookup USD price
            let Some(&usd_price) = prices.get(&asset_key) else {
                tracing::debug!(asset = %asset_key, "No price available, skipping batch swap");
                skipped += 1;
                continue;
            };

            let usd_value = self.collateral_usd_value(&asset_key, balance.0, usd_price);

            if usd_value < self.config.min_swap_value_usd {
                tracing::debug!(
                    asset = %asset_key,
                    balance = balance.0,
                    usd_value = format!("${usd_value:.2}"),
                    threshold = format!("${:.2}", self.config.min_swap_value_usd),
                    "Collateral below USD threshold, skipping batch swap"
                );
                skipped += 1;
                continue;
            }

            let Some(info) = self.collateral_price_map.get(&asset_key) else {
                skipped += 1;
                continue;
            };

            // Skip if swap provider doesn't support this asset pair
            if let Some(ref provider) = self.oneclick_provider {
                use crate::swap::SwapProvider;
                if !provider.supports_assets(collateral_asset, &info.target_borrow_asset) {
                    tracing::info!(
                        from = %asset_key,
                        to = %info.target_borrow_asset,
                        "Swap provider does not support asset pair, skipping batch swap"
                    );
                    skipped += 1;
                    continue;
                }
            }

            // Dry run: log what would happen, skip actual swap
            if dry_run {
                tracing::info!(
                    from = %asset_key,
                    to = %info.target_borrow_asset,
                    amount = balance.0,
                    usd_value = format!("${usd_value:.2}"),
                    "[DRY RUN] Would batch swap collateral"
                );
                swapped += 1;
                continue;
            }

            let Some(ref swap_provider) = self.oneclick_provider else {
                tracing::debug!("No swap provider, skipping batch swap");
                return;
            };

            tracing::info!(
                from = %asset_key,
                to = %info.target_borrow_asset,
                amount = balance.0,
                usd_value = format!("${usd_value:.2}"),
                "Attempting batch swap of accumulated collateral"
            );

            let swap_amount = templar_common::asset::FungibleAssetAmount::from(
                near_sdk::json_types::U128(balance.0),
            );

            let swap_name = format!("batch:{asset_key}");
            let provider = swap_provider.clone();
            let coll = collateral_asset.clone();
            let borrow = info.target_borrow_asset.clone();

            let result = crate::swap::retry::swap_with_retry(retry_config, &swap_name, || {
                let provider = provider.clone();
                let coll = coll.clone();
                let borrow = borrow.clone();
                async move {
                    provider
                        .swap(&coll, &borrow, swap_amount)
                        .await
                        .map(|_| ())
                        .map_err(|e| {
                            // Classify the AppError into SwapError
                            let msg = e.to_string();
                            let kind = if msg.contains("Amount is too low") {
                                crate::swap::SwapErrorKind::AmountTooLow { message: msg }
                            } else if msg.contains("Failed to get quote") {
                                crate::swap::SwapErrorKind::QuoteFailed { message: msg }
                            } else {
                                crate::swap::SwapErrorKind::Unknown { message: msg }
                            };
                            crate::swap::SwapError::new(kind, "Batch swap")
                        })
                }
            })
            .await;

            match result {
                Ok(()) => {
                    tracing::info!(
                        from = %asset_key,
                        usd_value = format!("${usd_value:.2}"),
                        "Batch swap succeeded"
                    );
                    swapped += 1;
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("Amount too low") {
                        tracing::debug!(
                            from = %asset_key,
                            error = %e,
                            "Batch swap skipped - amount too low for provider"
                        );
                    } else if msg.contains("Quote failed") {
                        tracing::debug!(
                            from = %asset_key,
                            "No swap route available for asset, skipping batch swap"
                        );
                    } else {
                        tracing::info!(
                            from = %asset_key,
                            error = %e,
                            "Batch swap failed, will retry next cycle"
                        );
                    }
                    skipped += 1;
                }
            }
        }

        if swapped > 0 || skipped > 0 {
            if dry_run {
                tracing::info!(
                    swapped,
                    skipped,
                    "[DRY RUN] Batch swap round summary (no swaps executed)"
                );
            } else {
                tracing::info!(swapped, skipped, "Batch swap round completed");
            }
        }
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

/// Check if an asset is a USDC variant (preferred swap target for rebalancing).
/// Non-critical: only used as a heuristic when the same collateral appears in
/// multiple markets to prefer swapping into USDC for better liquidity. If no
/// variant is recognized, batch swap still works but may pick a less liquid target.
fn is_usdc_asset(asset: &FungibleAsset<BorrowAsset>) -> bool {
    let s = asset.to_string().to_lowercase();
    s.contains("usdc")
        || s.contains("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48") // ETH USDC
        || s.contains("17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1") // native NEAR USDC
        || s.contains("5ce3bf3a31af18be40ba30f721101b4341690186") // Solana USDC
        || s.contains("1100_111bzqbb65gxapavoxqmmcgyo5os3txhqs1uh1cgahkquetujq1tju")
    // Stellar USDC
}

/// Check if an error is a rate limit error
fn is_rate_limit_error(error: &LiquidatorError) -> bool {
    let error_msg = error.to_string();
    error_msg.contains("TooManyRequests")
        || error_msg.contains("429")
        || error_msg.contains("rate limit")
}
