//! Configuration management for the liquidator bot.
//!
//! This module handles CLI argument parsing and service configuration creation.

use std::sync::Arc;

use clap::Parser;
use near_sdk::AccountId;
use templar_gateway_client::Network;

use crate::{
    notifier::{Notifier, TelegramConfig},
    service::ServiceConfig,
    swap::SwapRetryConfig,
    CollateralStrategy,
};

/// Parse a string into `Option<i64>`, treating empty/whitespace as `None`.
/// Panics if the value is non-empty and not a valid integer.
fn parse_optional_i64(s: &str) -> Option<i64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        trimmed
            .parse::<i64>()
            .unwrap_or_else(|_| panic!("TELEGRAM_THREAD_ID '{trimmed}' is not a valid integer")),
    )
}

fn validate_percentage(s: &str) -> Result<u8, String> {
    let value: u8 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid number"))?;
    if value == 0 || value > 100 {
        return Err(format!(
            "Partial percentage must be between 1 and 100, got {value}"
        ));
    }
    Ok(value)
}

/// Command-line arguments for the liquidator bot.
#[derive(Debug, Clone, Parser)]
#[command(name = "templar-liquidator")]
#[command(about = "Inventory-based liquidator bot for Templar Protocol")]
#[allow(clippy::struct_excessive_bools)]
pub struct Args {
    /// Market registries to run liquidations for
    #[arg(short, long, env = "REGISTRY_ACCOUNT_IDS")]
    pub registries: Vec<AccountId>,

    /// Signer key to use for signing transactions
    #[arg(short = 'k', long, env = "SIGNER_KEY")]
    pub signer_key: near_crypto::SecretKey,

    /// Signer account ID
    #[arg(short, long, env = "SIGNER_ACCOUNT_ID")]
    pub signer_account: AccountId,

    /// Network to run liquidations on
    #[arg(short, long, env = "NEAR_NETWORK", default_value_t = Network::Testnet)]
    pub network: Network,

    /// Custom RPC URL (overrides default network RPC).
    #[arg(long, env = "NEAR_RPC_URL")]
    pub near_rpc_url: Option<String>,

    /// API key for the RPC endpoint, sent as an `Authorization` header. May also
    /// be supplied as an `apiKey` query parameter on `--near-rpc-url`.
    #[arg(long, env = "NEAR_RPC_API_KEY")]
    pub near_rpc_api_key: Option<String>,

    /// Transaction timeout in seconds
    #[arg(long, env = "TRANSACTION_TIMEOUT", default_value_t = 60)]
    pub transaction_timeout: u64,

    /// Interval between liquidation scans in seconds
    #[arg(long, env = "LIQUIDATION_SCAN_INTERVAL", default_value_t = 600)]
    pub liquidation_scan_interval: u64,

    /// Registry refresh interval in seconds
    #[arg(long, env = "REGISTRY_REFRESH_INTERVAL", default_value_t = 3600)]
    pub registry_refresh_interval: u64,

    /// Concurrency for liquidations
    #[arg(short, long, env = "CONCURRENCY", default_value_t = 10)]
    pub concurrency: usize,

    /// Percentage of available liquidatable collateral to liquidate (1-100)
    /// If not set and --fixed-liquidation-amount-usd is also not set, defaults to 100%
    /// Mutually exclusive with --fixed-liquidation-amount-usd
    #[arg(long, env = "PARTIAL_LIQUIDATION_PERCENTAGE", value_parser = validate_percentage)]
    pub partial_percentage: Option<u8>,

    /// Fixed liquidation amount in USD
    /// Example: 100.0 for $100 USD (works across all USD-based markets with any decimals)
    /// Only supports USD-based borrow assets (USDC, USDT, DAI, etc.)
    /// Mutually exclusive with --partial-percentage
    #[arg(long, env = "FIXED_LIQUIDATION_AMOUNT_USD")]
    pub fixed_liquidation_amount_usd: Option<f64>,

    /// Minimum profit margin in basis points
    #[arg(long, env = "MIN_PROFIT_BPS", default_value_t = 50)]
    pub min_profit_bps: u32,

    /// Dry run mode - scan without executing transactions
    #[arg(long, env = "DRY_RUN", default_value_t = false)]
    pub dry_run: bool,

    /// Collateral strategy: "hold" or "swap-to-borrow"
    #[arg(long, env = "COLLATERAL_STRATEGY", default_value = "hold")]
    pub collateral_strategy: String,

    /// `OneClick` API token for swap authentication
    #[arg(long, env = "ONECLICK_API_TOKEN")]
    pub oneclick_api_token: Option<String>,

    /// Ref Finance contract address
    #[arg(long, env = "REF_CONTRACT")]
    pub ref_contract: Option<String>,

    /// Collateral asset allowlist for market filtering
    #[arg(long, env = "ALLOWED_COLLATERAL_ASSETS", value_delimiter = ',')]
    pub allowed_collateral_assets: Vec<String>,

    /// Collateral assets to ignore in market filtering
    #[arg(long, env = "IGNORED_COLLATERAL_ASSETS", value_delimiter = ',')]
    pub ignored_collateral_assets: Vec<String>,

    /// Market account IDs to ignore (comma-separated)
    #[arg(long, env = "IGNORED_MARKETS", value_delimiter = ',')]
    pub ignored_markets: Vec<String>,

    /// Enable loop liquidation - repeatedly liquidate until position is healthy
    #[arg(long, env = "LOOP_LIQUIDATION", default_value_t = false)]
    pub loop_liquidation: bool,

    /// Maximum iterations for loop liquidation (safety limit)
    #[arg(long, env = "MAX_LOOP_ITERATIONS", default_value_t = 10)]
    pub max_loop_iterations: u32,

    /// Pyth Hermes API URL for fetching price data
    #[arg(
        long,
        env = "PYTH_HERMES_URL",
        default_value = "https://hermes.pyth.network"
    )]
    pub hermes_url: String,

    /// RedStone gateway URL for fetching fresh prices
    #[arg(
        long,
        env = "REDSTONE_GATEWAY_URL",
        default_value = "https://oracle-gateway-1.a.redstone.vip"
    )]
    pub redstone_gateway_url: String,

    /// Minimum USD value to attempt a swap (JIT or batch).
    /// Amounts below this threshold are skipped and left for batch swap.
    #[arg(long, env = "MIN_SWAP_VALUE_USD", default_value_t = 10.0)]
    pub min_swap_value_usd: f64,

    /// Enable batch swap of accumulated collateral at the start of each liquidation round.
    #[arg(long, env = "BATCH_SWAP_ON_CYCLE_START", default_value_t = true)]
    pub batch_swap_on_cycle_start: bool,

    /// Maximum retry attempts for transient swap errors
    #[arg(long, env = "SWAP_RETRY_ATTEMPTS", default_value_t = 3)]
    pub swap_retry_attempts: u32,

    /// Base delay in milliseconds for swap retry exponential backoff (2s, 4s, 8s …)
    #[arg(long, env = "SWAP_RETRY_BASE_DELAY_MS", default_value_t = 2000)]
    pub swap_retry_base_delay_ms: u64,

    /// Number of consecutive scan failures before sending a Telegram alert.
    /// Set to 0 to disable scan failure notifications.
    #[arg(long, env = "SCAN_FAILURE_NOTIFY_THRESHOLD", default_value_t = 2)]
    pub scan_failure_notify_threshold: u32,

    /// Cooldown in hours for repeated "Liquidation Failed" notifications with
    /// the same (market, borrower, error class). Successful liquidations
    /// reset the cooldown for that borrower immediately.
    #[arg(
        long,
        env = "FAILURE_NOTIFICATION_COOLDOWN_HOURS",
        default_value_t = crate::notifier::DEFAULT_FAILURE_NOTIFY_COOLDOWN_HOURS
    )]
    pub failure_notification_cooldown_hours: u64,

    /// Telegram bot token for notifications (leave empty to disable)
    #[arg(long, env = "TELEGRAM_BOT_TOKEN", default_value = "")]
    pub telegram_bot_token: String,

    /// Telegram chat/channel ID for notifications
    #[arg(long, env = "TELEGRAM_CHAT_ID", default_value = "")]
    pub telegram_chat_id: String,

    /// Telegram thread/topic ID for sending to specific threads in supergroups.
    /// Accepts an empty env var gracefully (treated as unset).
    #[arg(long, env = "TELEGRAM_THREAD_ID", default_value = "")]
    pub telegram_thread_id: String,
}

impl Args {
    /// Parse command-line arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Create a liquidation strategy from the arguments
    pub fn create_strategy(&self) -> Arc<dyn crate::liquidation_strategy::LiquidationStrategy> {
        match (self.partial_percentage, self.fixed_liquidation_amount_usd) {
            (Some(_), Some(_)) => {
                panic!(
                    "Cannot specify both --partial-percentage and --fixed-liquidation-amount-usd. Choose one strategy."
                );
            }
            (None, Some(fixed_amount_usd)) => {
                tracing::info!(
                    fixed_amount_usd = fixed_amount_usd,
                    "Using FixedAmountLiquidationStrategy (USD-based, works across all USD markets)"
                );
                Arc::new(
                    crate::liquidation_strategy::FixedAmountLiquidationStrategy::new(
                        fixed_amount_usd,
                        self.min_profit_bps,
                    ),
                )
            }
            (percentage, None) => {
                let pct = percentage.unwrap_or(100);
                tracing::info!(
                    percentage = pct,
                    "Using PercentageLiquidationStrategy ({}% of available liquidatable collateral, 100% = full liquidation)",
                    pct
                );
                Arc::new(
                    crate::liquidation_strategy::PercentageLiquidationStrategy::new(
                        pct,
                        self.min_profit_bps,
                    ),
                )
            }
        }
    }

    /// Parse collateral strategy from config
    fn parse_collateral_strategy(&self) -> CollateralStrategy {
        // Normalize: convert to lowercase and replace hyphens with underscores
        let normalized = self.collateral_strategy.to_lowercase().replace('-', "_");

        match normalized.as_str() {
            "swap_to_borrow" => {
                tracing::info!("Using SwapToBorrow strategy");
                CollateralStrategy::SwapToBorrow
            }
            "hold" => {
                tracing::info!("Using Hold strategy (keep collateral as received)");
                CollateralStrategy::Hold
            }
            _ => panic!(
                "Invalid collateral strategy: '{}'. Valid options: 'hold', 'swap-to-borrow'",
                self.collateral_strategy
            ),
        }
    }

    /// Build service configuration from arguments
    #[allow(clippy::too_many_lines)]
    pub fn build_config(&self) -> ServiceConfig {
        let strategy = self.create_strategy();
        let collateral_strategy = self.parse_collateral_strategy();

        // Parse collateral asset filters
        let allowed_collateral_assets: Vec<_> = self
            .allowed_collateral_assets
            .iter()
            .filter_map(|s| {
                s.parse::<templar_common::asset::FungibleAsset<templar_common::asset::CollateralAsset>>()
                    .map_err(|e| {
                        tracing::warn!(
                            asset = %s,
                            error = ?e,
                            "Failed to parse allowed collateral asset, skipping"
                        );
                        e
                    })
                    .ok()
            })
            .collect();

        let ignored_collateral_assets: Vec<_> = self
            .ignored_collateral_assets
            .iter()
            .filter_map(|s| {
                s.parse::<templar_common::asset::FungibleAsset<templar_common::asset::CollateralAsset>>()
                    .map_err(|e| {
                        tracing::warn!(
                            asset = %s,
                            error = ?e,
                            "Failed to parse ignored collateral asset, skipping"
                        );
                        e
                    })
                    .ok()
            })
            .collect();

        // Log market filtering
        if allowed_collateral_assets.is_empty() {
            tracing::info!("Market filtering: processing all assets");
        } else {
            tracing::info!(
                allowed_assets = ?allowed_collateral_assets,
                "Market filtering enabled with allowlist"
            );
        }

        if !ignored_collateral_assets.is_empty() {
            tracing::info!(
                ignored_assets = ?ignored_collateral_assets,
                "Market filtering: ignoring specified assets"
            );
        }

        let ignored_markets: Vec<AccountId> = self
            .ignored_markets
            .iter()
            .filter_map(|s| {
                s.trim()
                    .parse::<AccountId>()
                    .map_err(|e| {
                        tracing::warn!(
                            market = %s,
                            error = ?e,
                            "Failed to parse ignored market account ID, skipping"
                        );
                        e
                    })
                    .ok()
            })
            .collect();

        if !ignored_markets.is_empty() {
            tracing::info!(
                ignored_markets = ?ignored_markets,
                "Market filtering: ignoring specified markets"
            );
        }

        // Build notifier — require both bot token and chat ID
        let bot_token = self.telegram_bot_token.trim();
        let chat_id = self.telegram_chat_id.trim();
        let telegram_config = match (bot_token.is_empty(), chat_id.is_empty()) {
            (true, true) => {
                tracing::info!("Telegram notifications disabled");
                None
            }
            (false, false) => {
                tracing::info!("Telegram notifications enabled");
                Some(TelegramConfig {
                    bot_token: bot_token.to_owned().into(),
                    chat_id: chat_id.to_owned(),
                    thread_id: parse_optional_i64(&self.telegram_thread_id),
                })
            }
            _ => {
                panic!("TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID must both be set or both be empty");
            }
        };
        let failure_cooldown =
            std::time::Duration::from_secs(self.failure_notification_cooldown_hours * 3600);
        let notifier = Arc::new(Notifier::with_cooldown(telegram_config, failure_cooldown));

        ServiceConfig {
            registries: self.registries.clone(),
            signer_key: self.signer_key.clone(),
            signer_account: self.signer_account.clone(),
            network: self.network,
            near_rpc_url: self.near_rpc_url.clone(),
            near_rpc_api_key: self.near_rpc_api_key.clone(),
            transaction_timeout: self.transaction_timeout,
            liquidation_scan_interval: self.liquidation_scan_interval,
            registry_refresh_interval: self.registry_refresh_interval,
            // `0` would make `buffer_unordered` hang forever; a refresh/scan with
            // no concurrency makes no sense, so floor it at 1.
            concurrency: self.concurrency.max(1),
            strategy,
            collateral_strategy,
            dry_run: self.dry_run,
            oneclick_api_token: self.oneclick_api_token.clone(),
            ref_contract: self.ref_contract.clone(),
            allowed_collateral_assets,
            ignored_collateral_assets,
            ignored_markets,
            loop_liquidation: self.loop_liquidation,
            max_loop_iterations: self.max_loop_iterations,
            hermes_url: self.hermes_url.clone(),
            redstone_gateway_url: self.redstone_gateway_url.clone(),
            min_swap_value_usd: self.min_swap_value_usd,
            batch_swap_on_cycle_start: self.batch_swap_on_cycle_start,
            swap_retry_config: SwapRetryConfig {
                max_attempts: self.swap_retry_attempts,
                base_delay_ms: self.swap_retry_base_delay_ms,
            },
            notifier,
            scan_failure_notify_threshold: self.scan_failure_notify_threshold,
        }
    }

    /// Log startup information
    pub fn log_startup(&self) {
        tracing::info!(
            network = %self.network,
            dry_run = self.dry_run,
            "Starting liquidator bot"
        );

        if self.dry_run {
            tracing::info!("DRY RUN MODE: Scanning only, no transactions will be executed");
        }
    }
}

#[cfg(test)]
mod tests {
    use templar_gateway_client::Network;

    use super::*;

    fn create_test_args() -> Args {
        Args {
            registries: vec!["registry.testnet".parse().unwrap()],
            signer_key: "ed25519:5JQFYvABVhxnvvvULXqZUSP8QtEiRBMUi5dHfkqZmJ2FLVJqMn3mEhZpF8p8qvC6SvdZLd5VDSvkeVJdyBDZfGi1"
                .parse()
                .unwrap(),
            signer_account: "liquidator.testnet".parse().unwrap(),
            network: Network::Testnet,
            near_rpc_url: None,
            near_rpc_api_key: None,
            transaction_timeout: 60,
            liquidation_scan_interval: 600,
            registry_refresh_interval: 3600,
            concurrency: 10,
            partial_percentage: Some(50),
            fixed_liquidation_amount_usd: None,
            min_profit_bps: 100,
            dry_run: false,
            collateral_strategy: "hold".to_string(),
            oneclick_api_token: None,
            ref_contract: None,
            allowed_collateral_assets: vec![],
            ignored_collateral_assets: vec![],
            ignored_markets: vec![],
            loop_liquidation: false,
            max_loop_iterations: 10,
            hermes_url: "https://hermes.pyth.network".to_string(),
            redstone_gateway_url: "https://oracle-gateway-1.a.redstone.vip".to_string(),
            min_swap_value_usd: 10.0,
            batch_swap_on_cycle_start: true,
            swap_retry_attempts: 3,
            swap_retry_base_delay_ms: 2000,
            scan_failure_notify_threshold: 2,
            failure_notification_cooldown_hours: crate::notifier::DEFAULT_FAILURE_NOTIFY_COOLDOWN_HOURS,
            telegram_bot_token: String::new(),
            telegram_chat_id: String::new(),
            telegram_thread_id: String::new(),
        }
    }

    #[test]
    fn test_parse_collateral_strategy_swap_to_borrow() {
        let mut args = create_test_args();
        args.collateral_strategy = "swap-to-borrow".to_string();

        let strategy = args.parse_collateral_strategy();
        assert!(matches!(strategy, CollateralStrategy::SwapToBorrow));
    }

    #[test]
    fn test_parse_collateral_strategy_hold() {
        let mut args = create_test_args();
        args.collateral_strategy = "hold".to_string();

        let strategy = args.parse_collateral_strategy();
        assert!(matches!(strategy, CollateralStrategy::Hold));
    }

    #[test]
    fn test_create_strategy_percentage_100() {
        let mut args = create_test_args();
        args.partial_percentage = Some(100);
        args.min_profit_bps = 200;

        let strategy = args.create_strategy();
        assert_eq!(strategy.strategy_name(), "Percentage Liquidation");
        assert_eq!(strategy.max_liquidation_percentage(), 100);
    }

    #[test]
    fn test_create_strategy_percentage_75() {
        let mut args = create_test_args();
        args.partial_percentage = Some(75);
        args.min_profit_bps = 150;

        let strategy = args.create_strategy();
        assert_eq!(strategy.strategy_name(), "Percentage Liquidation");
        assert_eq!(strategy.max_liquidation_percentage(), 75);
    }

    #[test]
    fn test_create_strategy_default_percentage() {
        let mut args = create_test_args();
        args.partial_percentage = None;
        args.fixed_liquidation_amount_usd = None;

        let strategy = args.create_strategy();
        assert_eq!(strategy.strategy_name(), "Percentage Liquidation");
        assert_eq!(strategy.max_liquidation_percentage(), 100);
    }

    #[test]
    fn test_create_strategy_fixed_amount() {
        let mut args = create_test_args();
        args.partial_percentage = None;
        args.fixed_liquidation_amount_usd = Some(100.0);

        let strategy = args.create_strategy();
        assert_eq!(strategy.strategy_name(), "Fixed Amount Liquidation");
    }

    #[test]
    #[should_panic(
        expected = "Cannot specify both --partial-percentage and --fixed-liquidation-amount-usd"
    )]
    fn test_create_strategy_mutual_exclusivity() {
        let mut args = create_test_args();
        args.partial_percentage = Some(50);
        args.fixed_liquidation_amount_usd = Some(100.0);

        args.create_strategy();
    }

    #[test]
    fn test_build_config() {
        let mut args = create_test_args();
        args.near_rpc_url = Some("https://custom.rpc.url".to_string());
        args.transaction_timeout = 90;
        args.liquidation_scan_interval = 300;
        args.registry_refresh_interval = 1800;
        args.concurrency = 5;
        args.dry_run = true;
        args.oneclick_api_token = Some("test_token".to_string());
        args.ref_contract = Some("ref.testnet".to_string());
        args.allowed_collateral_assets = vec!["nep141:usdc.testnet".to_string()];
        args.ignored_collateral_assets = vec!["nep141:scam.testnet".to_string()];

        let config = args.build_config();
        assert_eq!(config.registries.len(), 1);
        assert_eq!(config.network, Network::Testnet);
        assert_eq!(
            config.near_rpc_url,
            Some("https://custom.rpc.url".to_string())
        );
        assert_eq!(config.transaction_timeout, 90);
        assert_eq!(config.liquidation_scan_interval, 300);
        assert_eq!(config.registry_refresh_interval, 1800);
        assert_eq!(config.concurrency, 5);
        assert!(config.dry_run);
        assert_eq!(config.allowed_collateral_assets.len(), 1);
        assert_eq!(config.ignored_collateral_assets.len(), 1);
    }

    #[test]
    fn test_network_display() {
        assert_eq!(Network::Mainnet.to_string(), "mainnet");
        assert_eq!(Network::Testnet.to_string(), "testnet");
    }

    #[test]
    fn test_percentage_validation() {
        // Valid percentages
        assert_eq!(validate_percentage("1").unwrap(), 1);
        assert_eq!(validate_percentage("50").unwrap(), 50);
        assert_eq!(validate_percentage("100").unwrap(), 100);

        // Invalid percentages
        assert!(validate_percentage("0").is_err());
        assert!(validate_percentage("101").is_err());
        assert!(validate_percentage("abc").is_err());
        assert!(validate_percentage("-5").is_err());
    }

    #[test]
    fn test_telegram_disabled_both_empty() {
        let args = create_test_args();
        // Both empty → notifier disabled
        let config = args.build_config();
        assert!(!config.notifier.is_enabled());
    }

    #[test]
    fn test_telegram_enabled_both_set() {
        let mut args = create_test_args();
        args.telegram_bot_token = "123:ABC".to_string();
        args.telegram_chat_id = "-100123".to_string();
        let config = args.build_config();
        assert!(config.notifier.is_enabled());
    }

    #[test]
    fn test_telegram_enabled_with_thread_id() {
        let mut args = create_test_args();
        args.telegram_bot_token = "123:ABC".to_string();
        args.telegram_chat_id = "-100123".to_string();
        args.telegram_thread_id = "42".to_string();
        let config = args.build_config();
        assert!(config.notifier.is_enabled());
    }

    #[test]
    #[should_panic(expected = "TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID must both be set")]
    fn test_telegram_panics_token_without_chat_id() {
        let mut args = create_test_args();
        args.telegram_bot_token = "123:ABC".to_string();
        args.telegram_chat_id = String::new();
        args.build_config();
    }

    #[test]
    #[should_panic(expected = "TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID must both be set")]
    fn test_telegram_panics_chat_id_without_token() {
        let mut args = create_test_args();
        args.telegram_bot_token = String::new();
        args.telegram_chat_id = "-100123".to_string();
        args.build_config();
    }

    #[test]
    fn test_telegram_whitespace_only_treated_as_empty() {
        let mut args = create_test_args();
        args.telegram_bot_token = "  ".to_string();
        args.telegram_chat_id = "  ".to_string();
        let config = args.build_config();
        assert!(!config.notifier.is_enabled());
    }

    #[test]
    fn test_parse_optional_i64_empty() {
        assert_eq!(parse_optional_i64(""), None);
        assert_eq!(parse_optional_i64("  "), None);
    }

    #[test]
    fn test_parse_optional_i64_valid() {
        assert_eq!(parse_optional_i64("42"), Some(42));
        assert_eq!(parse_optional_i64(" -100 "), Some(-100));
    }

    #[test]
    #[should_panic(expected = "not a valid integer")]
    fn test_parse_optional_i64_invalid() {
        parse_optional_i64("abc");
    }

    #[test]
    fn test_telegram_empty_thread_id_env_var() {
        let mut args = create_test_args();
        args.telegram_bot_token = "123:ABC".to_string();
        args.telegram_chat_id = "-100123".to_string();
        args.telegram_thread_id = String::new();
        let config = args.build_config();
        assert!(config.notifier.is_enabled());
    }

    #[test]
    fn test_scan_failure_threshold_default() {
        let args = create_test_args();
        assert_eq!(args.scan_failure_notify_threshold, 2);
        let config = args.build_config();
        assert_eq!(config.scan_failure_notify_threshold, 2);
    }

    #[test]
    fn test_scan_failure_threshold_disabled() {
        let mut args = create_test_args();
        args.scan_failure_notify_threshold = 0;
        let config = args.build_config();
        assert_eq!(config.scan_failure_notify_threshold, 0);
    }

    #[test]
    fn test_scan_failure_threshold_custom() {
        let mut args = create_test_args();
        args.scan_failure_notify_threshold = 5;
        let config = args.build_config();
        assert_eq!(config.scan_failure_notify_threshold, 5);
    }
}
