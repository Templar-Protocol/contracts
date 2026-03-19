//! Configuration management for the liquidator bot.
//!
//! This module handles CLI argument parsing and service configuration creation.

use std::{str::FromStr, sync::Arc};

use clap::Parser;
use near_sdk::AccountId;
use templar_common::config::env::resolve_secret_key;
use templar_common::utils::Network;

use crate::{
    liquidation_strategy::{FullLiquidationStrategy, PartialLiquidationStrategy},
    service::ServiceConfig,
    CollateralStrategy,
};

/// Liquidation strategy argument type for CLI parsing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidationStrategyArg {
    /// Full liquidation (100%)
    Full,
    /// Partial liquidation (percentage specified separately)
    Partial,
    /// Fixed amount liquidation (amount specified separately)
    FixedAmount,
}

impl FromStr for LiquidationStrategyArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "full" => Ok(Self::Full),
            "partial" => Ok(Self::Partial),
            "fixed-amount" | "fixed_amount" => Ok(Self::FixedAmount),
            _ => Err(format!(
                "Invalid liquidation strategy: '{s}'. Valid options: 'full', 'partial', 'fixed-amount'"
            )),
        }
    }
}

impl std::fmt::Display for LiquidationStrategyArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "full"),
            Self::Partial => write!(f, "partial"),
            Self::FixedAmount => write!(f, "fixed-amount"),
        }
    }
}

impl Default for LiquidationStrategyArg {
    fn default() -> Self {
        Self::Partial
    }
}

/// Validator function for `partial_percentage` range
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
pub struct Args {
    /// Market registries to run liquidations for
    #[arg(short, long, env = "REGISTRY_ACCOUNT_IDS")]
    pub registries: Vec<AccountId>,

    /// Signer key to use for signing transactions
    #[arg(short = 'k', long)]
    pub signer_key: Option<near_crypto::SecretKey>,

    /// Signer account ID
    #[arg(short, long, env = "SIGNER_ACCOUNT_ID")]
    pub signer_account: AccountId,

    /// Network to run liquidations on
    #[arg(short, long, env = "NETWORK", default_value_t = Network::Testnet)]
    pub network: Network,

    /// Custom RPC URL (overrides default network RPC)
    #[arg(long, env = "RPC_URL")]
    pub rpc_url: Option<String>,

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

    /// Liquidation strategy: "partial" or "full"
    #[arg(long, env = "LIQUIDATION_STRATEGY", default_value_t = LiquidationStrategyArg::default())]
    pub liquidation_strategy: LiquidationStrategyArg,

    /// Partial liquidation percentage (1-100, only used with partial strategy)
    #[arg(long, env = "PARTIAL_LIQUIDATION_PERCENTAGE", value_parser = validate_percentage, default_value = "50")]
    pub partial_percentage: u8,

    /// Fixed liquidation amount in USD (only used with fixed-amount strategy)
    /// Example: 100.0 for $100 USD (works across all USD-based markets with any decimals)
    /// Only supports USD-based borrow assets (USDC, USDT, DAI, etc.)
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

    /// Enable loop liquidation - repeatedly liquidate until position is healthy
    #[arg(long, env = "LOOP_LIQUIDATION", default_value_t = false)]
    pub loop_liquidation: bool,

    /// Maximum iterations for loop liquidation (safety limit)
    #[arg(long, env = "MAX_LOOP_ITERATIONS", default_value_t = 10)]
    pub max_loop_iterations: u32,

    /// Pyth Hermes API URL for price updates
    #[arg(
        long,
        env = "PYTH_HERMES_URL",
        default_value = "https://hermes.pyth.network"
    )]
    pub hermes_url: String,

    /// Enable automatic Pyth price updates before liquidations
    #[arg(long, env = "AUTO_UPDATE_PRICES", default_value_t = false)]
    pub auto_update_prices: bool,
}

impl Args {
    /// Parse command-line arguments
    pub fn parse_args() -> Result<Self, String> {
        let mut args = Self::parse();
        args.signer_key = resolve_secret_key(
            args.signer_key.take(),
            "SIGNER_KEY",
            "SIGNER_KEY_FILE",
            |err| err.to_string(),
        )?;
        Ok(args)
    }

    /// Validate required configuration after parsing and secret resolution.
    pub fn validate(&self) -> Result<(), String> {
        if self.signer_key.is_none() {
            return Err("SIGNER_KEY or SIGNER_KEY_FILE is required".to_string());
        }

        Ok(())
    }

    /// Create a liquidation strategy from the arguments
    pub fn create_strategy(&self) -> Arc<dyn crate::liquidation_strategy::LiquidationStrategy> {
        match self.liquidation_strategy {
            LiquidationStrategyArg::Full => {
                tracing::info!("Using FullLiquidationStrategy (100% liquidation)");
                Arc::new(FullLiquidationStrategy::new(self.min_profit_bps))
            }
            LiquidationStrategyArg::Partial => {
                tracing::info!(
                    percentage = self.partial_percentage,
                    "Using PartialLiquidationStrategy"
                );
                Arc::new(PartialLiquidationStrategy::new(
                    self.partial_percentage,
                    self.min_profit_bps,
                ))
            }
            LiquidationStrategyArg::FixedAmount => {
                let Some(fixed_amount_usd) = self.fixed_liquidation_amount_usd else {
                    panic!(
                        "FIXED_LIQUIDATION_AMOUNT_USD must be set when using fixed-amount strategy"
                    );
                };
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

        ServiceConfig {
            registries: self.registries.clone(),
            signer_key: self
                .signer_key
                .clone()
                .expect("SIGNER_KEY or SIGNER_KEY_FILE must be set before build_config"),
            signer_account: self.signer_account.clone(),
            network: self.network,
            rpc_url: self.rpc_url.clone(),
            transaction_timeout: self.transaction_timeout,
            liquidation_scan_interval: self.liquidation_scan_interval,
            registry_refresh_interval: self.registry_refresh_interval,
            concurrency: self.concurrency,
            strategy,
            collateral_strategy,
            dry_run: self.dry_run,
            oneclick_api_token: self.oneclick_api_token.clone(),
            ref_contract: self.ref_contract.clone(),
            allowed_collateral_assets,
            ignored_collateral_assets,
            loop_liquidation: self.loop_liquidation,
            max_loop_iterations: self.max_loop_iterations,
            hermes_url: self.hermes_url.clone(),
            auto_update_prices: self.auto_update_prices,
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
    use templar_common::utils::Network;

    use super::*;

    fn create_test_args() -> Args {
        Args {
            registries: vec!["registry.testnet".parse().unwrap()],
            signer_key: Some(
                "ed25519:5JQFYvABVhxnvvvULXqZUSP8QtEiRBMUi5dHfkqZmJ2FLVJqMn3mEhZpF8p8qvC6SvdZLd5VDSvkeVJdyBDZfGi1"
                    .parse()
                    .unwrap(),
            ),
            signer_account: "liquidator.testnet".parse().unwrap(),
            network: Network::Testnet,
            rpc_url: None,
            transaction_timeout: 60,
            liquidation_scan_interval: 600,
            registry_refresh_interval: 3600,
            concurrency: 10,
            liquidation_strategy: LiquidationStrategyArg::Partial,
            partial_percentage: 50,
            fixed_liquidation_amount_usd: None,
            min_profit_bps: 100,
            dry_run: false,
            collateral_strategy: "hold".to_string(),
            oneclick_api_token: None,
            ref_contract: None,
            allowed_collateral_assets: vec![],
            ignored_collateral_assets: vec![],
            loop_liquidation: false,
            max_loop_iterations: 10,
            hermes_url: "https://hermes.pyth.network".to_string(),
            auto_update_prices: false,
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
    fn test_create_strategy_full() {
        let mut args = create_test_args();
        args.liquidation_strategy = LiquidationStrategyArg::Full;
        args.min_profit_bps = 200;

        let strategy = args.create_strategy();
        assert_eq!(strategy.strategy_name(), "Full Liquidation");
        assert_eq!(strategy.max_liquidation_percentage(), 100);
    }

    #[test]
    fn test_create_strategy_partial() {
        let mut args = create_test_args();
        args.liquidation_strategy = LiquidationStrategyArg::Partial;
        args.partial_percentage = 75;
        args.min_profit_bps = 150;

        let strategy = args.create_strategy();
        assert_eq!(strategy.strategy_name(), "Partial Liquidation");
        assert_eq!(strategy.max_liquidation_percentage(), 75);
    }

    #[test]
    fn test_build_config() {
        let mut args = create_test_args();
        args.rpc_url = Some("https://custom.rpc.url".to_string());
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
        assert_eq!(config.rpc_url, Some("https://custom.rpc.url".to_string()));
        assert_eq!(config.transaction_timeout, 90);
        assert_eq!(config.liquidation_scan_interval, 300);
        assert_eq!(config.registry_refresh_interval, 1800);
        assert_eq!(config.concurrency, 5);
        assert!(config.dry_run);
        assert_eq!(config.allowed_collateral_assets.len(), 1);
        assert_eq!(config.ignored_collateral_assets.len(), 1);
    }

    #[test]
    fn test_validate_requires_signer_key() {
        let mut args = create_test_args();
        args.signer_key = None;

        let result = args.validate();
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "SIGNER_KEY or SIGNER_KEY_FILE is required"
        );
    }

    #[test]
    fn test_network_display() {
        assert_eq!(Network::Mainnet.to_string(), "mainnet");
        assert_eq!(Network::Testnet.to_string(), "testnet");
    }

    #[test]
    fn test_liquidation_strategy_parsing() {
        // Test valid strategies
        assert_eq!(
            "partial".parse::<LiquidationStrategyArg>().unwrap(),
            LiquidationStrategyArg::Partial
        );
        assert_eq!(
            "full".parse::<LiquidationStrategyArg>().unwrap(),
            LiquidationStrategyArg::Full
        );
        assert_eq!(
            "FULL".parse::<LiquidationStrategyArg>().unwrap(),
            LiquidationStrategyArg::Full
        );

        // Test invalid strategy
        let result = "invalid".parse::<LiquidationStrategyArg>();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid liquidation strategy"));
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
}
