// SPDX-License-Identifier: MIT
//! Configuration management for the liquidator bot.
//!
//! This module handles CLI argument parsing and service configuration creation.

use std::sync::Arc;

use clap::Parser;
use near_sdk::AccountId;

use crate::{
    liquidation_strategy::{FullLiquidationStrategy, PartialLiquidationStrategy},
    rpc::Network,
    service::ServiceConfig,
    CollateralStrategy,
};

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

    /// Inventory refresh interval in seconds
    #[arg(long, env = "INVENTORY_REFRESH_INTERVAL", default_value_t = 300)]
    pub inventory_refresh_interval: u64,

    /// Concurrency for liquidations
    #[arg(short, long, env = "CONCURRENCY", default_value_t = 10)]
    pub concurrency: usize,

    /// Liquidation strategy: "partial" or "full"
    #[arg(long, env = "LIQUIDATION_STRATEGY", default_value = "partial")]
    pub liquidation_strategy: String,

    /// Partial liquidation percentage (1-100, only used with partial strategy)
    #[arg(long, env = "PARTIAL_PERCENTAGE", default_value_t = 50)]
    pub partial_percentage: u8,

    /// Minimum profit margin in basis points
    #[arg(long, env = "MIN_PROFIT_BPS", default_value_t = 50)]
    pub min_profit_bps: u32,

    /// Maximum gas cost percentage
    #[arg(long, env = "MAX_GAS_PERCENTAGE", default_value_t = 10)]
    pub max_gas_percentage: u8,

    /// Dry run mode - scan markets and log liquidation opportunities without executing transactions
    #[arg(long, env = "DRY_RUN", default_value_t = false)]
    pub dry_run: bool,
}

impl Args {
    /// Parse command-line arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Create a liquidation strategy from the arguments
    pub fn create_strategy(&self) -> Arc<dyn crate::liquidation_strategy::LiquidationStrategy> {
        match self.liquidation_strategy.to_lowercase().as_str() {
            "full" => {
                tracing::info!("Using FullLiquidationStrategy (100% liquidation)");
                Arc::new(FullLiquidationStrategy::new(
                    self.min_profit_bps,
                    self.max_gas_percentage,
                ))
            }
            "partial" => {
                tracing::info!(
                    percentage = self.partial_percentage,
                    "Using PartialLiquidationStrategy"
                );
                Arc::new(PartialLiquidationStrategy::new(
                    self.partial_percentage,
                    self.min_profit_bps,
                    self.max_gas_percentage,
                ))
            }
            other => {
                tracing::error!(
                    strategy = other,
                    "Invalid liquidation strategy, defaulting to 'partial'"
                );
                Arc::new(PartialLiquidationStrategy::new(
                    self.partial_percentage,
                    self.min_profit_bps,
                    self.max_gas_percentage,
                ))
            }
        }
    }

    /// Build a `ServiceConfig` from the arguments
    pub fn build_config(&self) -> ServiceConfig {
        let strategy = self.create_strategy();
        let collateral_strategy = CollateralStrategy::Hold;

        ServiceConfig {
            registries: self.registries.clone(),
            signer_key: self.signer_key.clone(),
            signer_account: self.signer_account.clone(),
            network: self.network,
            rpc_url: self.rpc_url.clone(),
            transaction_timeout: self.transaction_timeout,
            liquidation_scan_interval: self.liquidation_scan_interval,
            registry_refresh_interval: self.registry_refresh_interval,
            inventory_refresh_interval: self.inventory_refresh_interval,
            concurrency: self.concurrency,
            strategy,
            collateral_strategy,
            dry_run: self.dry_run,
        }
    }

    /// Log startup information
    pub fn log_startup(&self) {
        tracing::info!(
            network = %self.network,
            dry_run = self.dry_run,
            "Starting liquidator bot (inventory-based)"
        );

        if self.dry_run {
            tracing::info!(
                "DRY RUN MODE: Will scan and log opportunities without executing liquidations"
            );
        }
    }
}
