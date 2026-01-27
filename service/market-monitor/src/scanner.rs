//! Market scanner.
//!
//! Fetches market data from NEAR RPC including:
//! - Market deployments from registry contracts
//! - Market configurations and version metadata
//! - Borrow positions with pagination support
//! - Oracle price data

use crate::{config::Config, error::Result, rpc};
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{serde_json::json, AccountId};
use std::collections::HashMap;
use templar_common::{
    borrow::BorrowPosition, market::MarketConfiguration, oracle::pyth::OracleResponse,
};

pub type BorrowPositions = HashMap<AccountId, BorrowPosition>;

pub struct MarketScanner {
    client: JsonRpcClient,
}

impl MarketScanner {
    /// Minimum market version (1.0.0)
    pub const MIN_SUPPORTED_VERSION: (u32, u32, u32) = (1, 0, 0);

    pub fn new(rpc_url: &str) -> Self {
        let client = JsonRpcClient::connect(rpc_url);
        Self { client }
    }

    /// Fetch markets from registry
    ///
    /// # Errors
    /// Returns an error if the RPC call fails or response cannot be parsed.
    pub async fn get_markets_from_registry(&self, registry: &AccountId) -> Result<Vec<AccountId>> {
        let mut all_deployments = Vec::new();
        let page_size = 500;
        let mut current_offset = 0;

        loop {
            let page: Vec<AccountId> = rpc::view(
                &self.client,
                registry.clone(),
                "list_deployments",
                json!({
                    "offset": current_offset,
                    "count": page_size,
                }),
            )
            .await?;

            let fetched = page.len();
            if fetched == 0 {
                break;
            }

            all_deployments.extend(page);
            current_offset += fetched;

            if fetched < page_size {
                break;
            }
        }

        Ok(all_deployments)
    }

    /// Get market configuration
    /// # Errors
    /// Returns an error if the RPC call fails or configuration cannot be deserialized.
    pub async fn get_market_config(&self, market: &AccountId) -> Result<MarketConfiguration> {
        let config =
            rpc::view(&self.client, market.clone(), "get_configuration", json!({})).await?;

        Ok(config)
    }

    /// Check if market version is compatible
    ///
    /// # Errors
    /// Returns an error if the RPC call fails or version metadata cannot be fetched.
    pub async fn check_market_version(
        &self,
        market: &AccountId,
        min_version: (u32, u32, u32),
    ) -> Result<bool> {
        let version_str = rpc::get_contract_version(&self.client, market).await;

        let Some(version_str) = version_str else {
            // No NEP-330 metadata - assume basic compatibility like liquidator does
            tracing::debug!(
                market = %market,
                "Contract missing NEP-330 metadata, assuming basic compatibility"
            );
            return Ok(true);
        };

        let parts: Vec<&str> = version_str.split('.').collect();
        if parts.len() != 3 {
            tracing::info!(market = %market, version = %version_str, "Invalid version format, skipping");
            return Ok(false);
        }

        let major = parts[0].parse::<u32>().unwrap_or(0);
        let minor = parts[1].parse::<u32>().unwrap_or(0);
        let patch = parts[2].parse::<u32>().unwrap_or(0);

        let is_compatible = (major, minor, patch) >= min_version;

        if is_compatible {
            tracing::info!(
                market = %market,
                version = %version_str,
                "Market is compatible"
            );
        } else {
            let (min_major, min_minor, min_patch) = min_version;
            tracing::info!(
                market = %market,
                version = %version_str,
                min_version = %format!("{min_major}.{min_minor}.{min_patch}"),
                "Skipping market - unsupported contract version"
            );
        }

        Ok(is_compatible)
    }

    /// Fetch all borrow positions from a market
    /// # Errors
    /// Returns an error if any RPC call fails during pagination.
    pub async fn get_all_borrows(&self, market: &AccountId) -> Result<BorrowPositions> {
        let mut all_positions: BorrowPositions = HashMap::new();
        let page_size = 500;
        let mut current_offset = 0;

        loop {
            let page: BorrowPositions = rpc::view(
                &self.client,
                market.clone(),
                "list_borrow_positions",
                json!({
                    "offset": current_offset,
                    "count": page_size,
                }),
            )
            .await?;

            let fetched = page.len();
            if fetched == 0 {
                break;
            }

            tracing::debug!(
                market = %market,
                offset = current_offset,
                fetched = fetched,
                "Fetched borrow positions page"
            );

            all_positions.extend(page);
            current_offset += fetched;

            if fetched < page_size {
                break;
            }
        }

        tracing::info!(
            market = %market,
            total_positions = all_positions.len(),
            "Fetched all borrow positions"
        );

        Ok(all_positions)
    }

    /// Get oracle prices for a market
    /// # Errors
    /// Returns an error if the oracle RPC call fails or response cannot be deserialized.
    pub async fn get_oracle_prices(&self, config: &MarketConfiguration) -> Result<OracleResponse> {
        let oracle_config = &config.price_oracle_configuration;
        let oracle = &oracle_config.account_id;
        let price_ids = vec![
            oracle_config.borrow_asset_price_id,
            oracle_config.collateral_asset_price_id,
        ];

        // Skip LST oracles that use promises (can't be called in view mode)
        let oracle_str = oracle.as_str();
        if oracle_str.contains("lst.oracle") {
            return Err(crate::error::MonitorError::Rpc(format!(
                "LST oracle {oracle} uses promises and cannot be called in view mode"
            )));
        }

        // Try unsafe method first (faster, no age validation)
        let result: std::result::Result<OracleResponse, _> = rpc::view(
            &self.client,
            oracle.clone(),
            "list_ema_prices_unsafe",
            json!({ "price_ids": price_ids }),
        )
        .await;

        match result {
            Ok(response) => Ok(response),
            Err(e) => {
                let error_msg = format!("{e:?}");

                // If method not found, try the standard method with age validation
                if error_msg.contains("MethodNotFound") || error_msg.contains("MethodResolveError")
                {
                    let response: OracleResponse = rpc::view(
                        &self.client,
                        oracle.clone(),
                        "list_ema_prices_no_older_than",
                        json!({
                            "price_ids": price_ids,
                            "age": 300  // 5 minutes max age
                        }),
                    )
                    .await?;

                    Ok(response)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Filter markets based on configuration
    pub fn should_include_market(
        market: &AccountId,
        config: &MarketConfiguration,
        filter_config: &Config,
    ) -> bool {
        // Check if market is in ignore list
        if filter_config.ignored_markets.contains(market) {
            tracing::debug!(market = %market, "Market in ignore list");
            return false;
        }

        // Check ignore list for collateral
        if filter_config
            .ignored_collateral_assets
            .contains(&config.collateral_asset)
        {
            tracing::debug!(
                market = %market,
                collateral = ?config.collateral_asset,
                "Market collateral in ignore list"
            );
            return false;
        }

        true
    }
}
