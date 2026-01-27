//! Market processing logic.
//!
//! Handles the discovery, filtering, and analysis of markets from registries.

use crate::{
    analyzer::Analyzer, config::Config, error::Result, scanner::MarketScanner, types::MarketReport,
};
use near_sdk::AccountId;
use templar_common::number::Decimal;

/// Market processing statistics.
#[derive(Debug, Default)]
pub struct ProcessingStats {
    pub total_positions: usize,
    pub red_count: usize,
    pub yellow_count: usize,
    pub green_count: usize,
    pub red_value_usd: Decimal,
    pub yellow_value_usd: Decimal,
    pub ignored_markets_count: usize,
}

/// Processes all markets from the configured registries.
///
/// # Arguments
///
/// * `config` - Configuration settings
/// * `scanner` - Market scanner for fetching data
/// * `analyzer` - Position analyzer for health checks
///
/// # Returns
///
/// Returns a tuple of (market reports, processing statistics).
///
/// # Errors
/// Returns an error if market version checking fails or RPC calls fail.
pub async fn process_markets(
    config: &Config,
    scanner: &MarketScanner,
    analyzer: &Analyzer,
) -> Result<(Vec<MarketReport>, ProcessingStats)> {
    // Discover markets from all registries
    let all_markets = discover_markets(config, scanner).await;

    // Process each market
    let mut market_reports = Vec::new();
    let mut stats = ProcessingStats::default();

    for (i, market) in all_markets.iter().enumerate() {
        tracing::info!(
            market = %market,
            progress = format!("{}/{}", i + 1, all_markets.len()),
            "Processing market"
        );

        // Check if market is in ignore list first (before any RPC calls)
        if config.ignored_markets.contains(market) {
            tracing::debug!(market = %market, "Market in ignore list, skipping");
            stats.ignored_markets_count += 1;
            continue;
        }

        // Check version compatibility
        if !scanner
            .check_market_version(market, MarketScanner::MIN_SUPPORTED_VERSION)
            .await?
        {
            continue;
        }

        // Get market configuration
        let market_config = match scanner.get_market_config(market).await {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!(market = %market, error = %e, "Failed to get market config, skipping");
                continue;
            }
        };

        // Check collateral filters
        if !MarketScanner::should_include_market(market, &market_config, config) {
            stats.ignored_markets_count += 1;
            continue;
        }

        // Process this market
        if let Some(report) =
            process_single_market(market, config, scanner, analyzer, &mut stats).await?
        {
            market_reports.push(report);
        }
    }

    Ok((market_reports, stats))
}

/// Discovers all markets from the configured registries.
async fn discover_markets(config: &Config, scanner: &MarketScanner) -> Vec<AccountId> {
    tracing::info!(
        registries = ?config.registry_account_ids,
        "Fetching markets from registries"
    );

    let mut all_markets = Vec::new();

    for registry in &config.registry_account_ids {
        tracing::info!(registry = %registry, "Fetching deployments from registry");
        match scanner.get_markets_from_registry(registry).await {
            Ok(markets) => {
                tracing::info!(
                    registry = %registry,
                    market_count = markets.len(),
                    markets = ?markets,
                    "Found deployments from registry"
                );
                all_markets.extend(markets);
            }
            Err(e) => {
                tracing::error!(registry = %registry, error = %e, "Failed to fetch markets from registry");
            }
        }
    }

    tracing::info!(
        total_markets = all_markets.len(),
        markets = ?all_markets,
        "Total markets discovered"
    );

    all_markets
}

/// Processes a single market and returns its report.
async fn process_single_market(
    market: &AccountId,
    config: &Config,
    scanner: &MarketScanner,
    analyzer: &Analyzer,
    stats: &mut ProcessingStats,
) -> Result<Option<MarketReport>> {
    use crate::types::AlertZone;

    // Get market configuration (already validated by caller)
    let market_config = scanner.get_market_config(market).await?;

    // Fetch positions
    let positions = match scanner.get_all_borrows(market).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(market = %market, error = %e, "Failed to fetch positions, skipping");
            return Ok(None);
        }
    };

    if positions.is_empty() {
        tracing::info!(market = %market, "No positions in market, skipping");
        return Ok(None);
    }

    let position_count = positions.len();
    stats.total_positions += position_count;

    // Get oracle prices
    let oracle_response = match scanner.get_oracle_prices(&market_config).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(market = %market, error = %e, "Skipping market - oracle unavailable");
            return Ok(None);
        }
    };

    // Analyze positions
    let mut red_positions = Vec::new();
    let mut yellow_positions = Vec::new();

    for (borrower, position) in positions {
        match analyzer.analyze_position(
            market,
            &borrower,
            &position,
            &market_config,
            &oracle_response,
        ) {
            Ok(Some(alert)) => {
                match alert.zone {
                    AlertZone::Red => {
                        stats.red_count += 1;
                        stats.red_value_usd += alert.position_value_usd;
                        // Only include in alert list if above min size
                        if alert.position_value_usd >= Decimal::from(config.min_position_size_usd) {
                            red_positions.push(alert);
                        }
                    }
                    AlertZone::Yellow => {
                        stats.yellow_count += 1;
                        stats.yellow_value_usd += alert.position_value_usd;
                        // Only include in alert list if above min size
                        if alert.position_value_usd >= Decimal::from(config.min_position_size_usd) {
                            yellow_positions.push(alert);
                        }
                    }
                    AlertZone::Green => {
                        stats.green_count += 1;
                    }
                }
            }
            Ok(None) => {
                // Healthy position
                stats.green_count += 1;
            }
            Err(e) => {
                tracing::warn!(
                    market = %market,
                    borrower = %borrower,
                    error = %e,
                    "Failed to analyze position"
                );
            }
        }
    }

    // Log results
    let red_len = red_positions.len();
    let yellow_len = yellow_positions.len();

    if red_len > 0 || yellow_len > 0 {
        tracing::info!(
            market = %market,
            red = red_len,
            yellow = yellow_len,
            "Positions need attention"
        );
    } else {
        tracing::debug!(
            market = %market,
            positions = position_count,
            "Market processed - all positions healthy"
        );
    }

    Ok(Some(MarketReport {
        market: market.clone(),
        mcr_liquidation: market_config.borrow_mcr_liquidation,
        red_positions,
        yellow_positions,
    }))
}
