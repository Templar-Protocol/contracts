//! Configuration management for the liquidator bot.
//!
//! This module handles CLI argument parsing and service configuration creation.

use std::{str::FromStr, sync::Arc};

use clap::Parser;
use near_sdk::AccountId;

use crate::{
    liquidation_strategy::{FullLiquidationStrategy, PartialLiquidationStrategy},
    rpc::Network,
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
}

impl FromStr for LiquidationStrategyArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "full" => Ok(Self::Full),
            "partial" => Ok(Self::Partial),
            _ => Err(format!(
                "Invalid liquidation strategy: '{s}'. Valid options: 'full', 'partial'"
            )),
        }
    }
}

impl std::fmt::Display for LiquidationStrategyArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "full"),
            Self::Partial => write!(f, "partial"),
        }
    }
}

/// Collateral strategy argument type for CLI parsing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollateralStrategyArg {
    /// Hold collateral as received
    Hold,
    /// Swap collateral to a primary asset (requires `primary_asset` config)
    SwapToPrimary,
    /// Swap collateral back to borrow assets
    SwapToBorrow,
}

impl FromStr for CollateralStrategyArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Normalize: convert to lowercase and replace hyphens with underscores
        let normalized = s.to_lowercase().replace('-', "_");
        match normalized.as_str() {
            "hold" => Ok(Self::Hold),
            "swap_to_primary" => Ok(Self::SwapToPrimary),
            "swap_to_borrow" => Ok(Self::SwapToBorrow),
            _ => Err(format!(
                "Invalid collateral strategy: '{s}'. Valid options: 'hold', 'swap-to-primary', 'swap-to-borrow'"
            )),
        }
    }
}

impl std::fmt::Display for CollateralStrategyArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hold => write!(f, "hold"),
            Self::SwapToPrimary => write!(f, "swap-to-primary"),
            Self::SwapToBorrow => write!(f, "swap-to-borrow"),
        }
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
    #[arg(short = 'k', long, env = "SIGNER_KEY")]
    pub signer_key: near_crypto::SecretKey,

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
    #[arg(long, env = "LIQUIDATION_STRATEGY", default_value = "partial")]
    pub liquidation_strategy: LiquidationStrategyArg,

    /// Partial liquidation percentage (1-100, only used with partial strategy)
    #[arg(long, env = "PARTIAL_PERCENTAGE", value_parser = validate_percentage, default_value = "50")]
    pub partial_percentage: u8,

    /// Minimum profit margin in basis points
    #[arg(long, env = "MIN_PROFIT_BPS", default_value_t = 50)]
    pub min_profit_bps: u32,

    /// Dry run mode - scan without executing transactions
    #[arg(long, env = "DRY_RUN", default_value_t = false)]
    pub dry_run: bool,

    /// Collateral strategy: "hold", "swap-to-primary", or "swap-to-borrow"
    #[arg(long, env = "COLLATERAL_STRATEGY", default_value = "hold")]
    pub collateral_strategy: CollateralStrategyArg,

    /// Primary asset for `SwapToPrimary` strategy
    #[arg(long, env = "PRIMARY_ASSET")]
    pub primary_asset: Option<String>,

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
}

impl Args {
    /// Parse command-line arguments
    pub fn parse_args() -> Self {
        Self::parse()
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
        }
    }

    /// Parse collateral strategy from config
    fn parse_collateral_strategy(&self) -> CollateralStrategy {
        use templar_common::asset::FungibleAsset;

        match self.collateral_strategy {
            CollateralStrategyArg::SwapToPrimary => {
                let Some(ref primary_asset_str) = self.primary_asset else {
                    panic!("COLLATERAL_STRATEGY=swap-to-primary requires PRIMARY_ASSET to be set");
                };

                let primary_asset = primary_asset_str.parse::<FungibleAsset<_>>()
                    .unwrap_or_else(|_| panic!(
                        "Failed to parse PRIMARY_ASSET: '{primary_asset_str}'. Expected format: nep141:contract_id or nep245:contract_id:token_id"
                    ));

                tracing::info!(
                    primary_asset = %primary_asset,
                    "Using SwapToPrimary strategy"
                );
                CollateralStrategy::SwapToPrimary { primary_asset }
            }
            CollateralStrategyArg::SwapToBorrow => {
                tracing::info!("Using SwapToBorrow strategy");
                CollateralStrategy::SwapToBorrow
            }
            CollateralStrategyArg::Hold => {
                tracing::info!("Using Hold strategy (keep collateral as received)");
                CollateralStrategy::Hold
            }
        }
    }

    /// Build service configuration from arguments
    pub fn build_config(&self) -> ServiceConfig {
        let strategy = self.create_strategy();
        let collateral_strategy = self.parse_collateral_strategy();

        // Log market filtering
        if self.allowed_collateral_assets.is_empty() {
            tracing::info!("Market filtering: processing all assets");
        } else {
            tracing::info!(
                allowed_assets = ?self.allowed_collateral_assets,
                "Market filtering enabled with allowlist"
            );
        }

        if !self.ignored_collateral_assets.is_empty() {
            tracing::info!(
                ignored_assets = ?self.ignored_collateral_assets,
                "Market filtering: ignoring specified assets"
            );
        }

        ServiceConfig {
            registries: self.registries.clone(),
            signer_key: self.signer_key.clone(),
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
            allowed_collateral_assets: self.allowed_collateral_assets.clone(),
            ignored_collateral_assets: self.ignored_collateral_assets.clone(),
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
    use super::*;
    use crate::rpc::Network;

    fn create_test_args() -> Args {
        Args {
            registries: vec!["registry.testnet".parse().unwrap()],
            signer_key: "ed25519:5JQFYvABVhxnvvvULXqZUSP8QtEiRBMUi5dHfkqZmJ2FLVJqMn3mEhZpF8p8qvC6SvdZLd5VDSvkeVJdyBDZfGi1"
                .parse()
                .unwrap(),
            signer_account: "liquidator.testnet".parse().unwrap(),
            network: Network::Testnet,
            rpc_url: None,
            transaction_timeout: 60,
            liquidation_scan_interval: 600,
            registry_refresh_interval: 3600,
            concurrency: 10,
            liquidation_strategy: LiquidationStrategyArg::Partial,
            partial_percentage: 50,
            min_profit_bps: 100,
            dry_run: false,
            collateral_strategy: CollateralStrategyArg::Hold,
            primary_asset: None,
            oneclick_api_token: None,
            ref_contract: None,
            allowed_collateral_assets: vec![],
            ignored_collateral_assets: vec![],
        }
    }

    #[test]
    fn test_parse_collateral_strategy_swap_to_primary() {
        let mut args = create_test_args();
        args.collateral_strategy = CollateralStrategyArg::SwapToPrimary;
        args.primary_asset = Some("nep141:usdc.testnet".to_string());

        let strategy = args.parse_collateral_strategy();
        assert!(matches!(strategy, CollateralStrategy::SwapToPrimary { .. }));
    }

    #[test]
    fn test_parse_collateral_strategy_swap_to_borrow() {
        let mut args = create_test_args();
        args.collateral_strategy = CollateralStrategyArg::SwapToBorrow;

        let strategy = args.parse_collateral_strategy();
        assert!(matches!(strategy, CollateralStrategy::SwapToBorrow));
    }

    #[test]
    fn test_parse_collateral_strategy_hold() {
        let mut args = create_test_args();
        args.collateral_strategy = CollateralStrategyArg::Hold;

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
    fn test_network_display() {
        assert_eq!(Network::Mainnet.to_string(), "mainnet");
        assert_eq!(Network::Testnet.to_string(), "testnet");
    }

    #[test]
    fn test_collateral_strategy_parsing() {
        // Test hyphenated version
        let result1 = "swap-to-borrow".parse::<CollateralStrategyArg>();
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), CollateralStrategyArg::SwapToBorrow);

        // Test underscored version
        let result2 = "swap_to_borrow".parse::<CollateralStrategyArg>();
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap(), CollateralStrategyArg::SwapToBorrow);

        // Test case insensitivity
        let result3 = "HOLD".parse::<CollateralStrategyArg>();
        assert!(result3.is_ok());
        assert_eq!(result3.unwrap(), CollateralStrategyArg::Hold);
    }

    #[test]
    fn test_invalid_collateral_strategy() {
        let result = "invalid_strategy".parse::<CollateralStrategyArg>();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid collateral strategy"));
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
