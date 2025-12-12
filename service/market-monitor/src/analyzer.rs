//! Position analyzer.
//!
//! Calculates collateralization ratio (CR) and classifies positions into alert zones:
//! - Red: CR < MCR (liquidatable)
//! - Yellow: MCR ≤ CR < MCR × (1 + threshold%) (at risk)
//! - Green: CR ≥ MCR × (1 + threshold%) (healthy)

use crate::{
    config::Config,
    error::Result,
    types::{AlertZone, PositionAlert},
};
use near_sdk::AccountId;

use templar_common::{
    borrow::BorrowPosition, market::MarketConfiguration, number::Decimal,
    oracle::pyth::OracleResponse,
};

pub struct Analyzer {
    yellow_zone_multiplier: Decimal,
}

impl Analyzer {
    /// Creates a new analyzer with the configured at-risk threshold.
    pub fn new(config: &Config) -> Self {
        let yellow_zone_multiplier =
            Decimal::from(100 + u32::from(config.at_risk_threshold_percent)) / 100u32;

        Self {
            yellow_zone_multiplier,
        }
    }

    /// Analyzes a borrow position and classifies it into an alert zone.
    ///
    /// # Arguments
    /// * `market` - The market contract account ID
    /// * `borrower` - The borrower's account ID
    /// * `position` - The borrow position data
    /// * `market_config` - Market configuration including MCR
    /// * `oracle_response` - Current oracle price data
    ///
    /// # Returns
    /// * `Ok(Some(alert))` - Position requires attention (Red or Yellow zone)
    /// * `Ok(None)` - Position is healthy (Green zone) or has no debt
    ///
    /// # Errors
    /// Returns an error if the oracle price pair cannot be created.
    pub fn analyze_position(
        &self,
        market: &AccountId,
        borrower: &AccountId,
        position: &BorrowPosition,
        market_config: &MarketConfiguration,
        oracle_response: &OracleResponse,
    ) -> Result<Option<PositionAlert>> {
        // Create price pair
        let price_pair = market_config
            .price_oracle_configuration
            .create_price_pair(oracle_response)
            .map_err(|e| {
                crate::error::MonitorError::Market(format!("Failed to create price pair: {e:?}"))
            })?;

        // Calculate collateralization ratio
        let cr = position.collateralization_ratio(&price_pair);

        let Some(cr) = cr else {
            // No debt, skip
            return Ok(None);
        };

        // Calculate position amounts
        let collateral_amount: u128 = position.collateral_asset_deposit.into();
        let borrow_amount: u128 = position.get_total_borrow_asset_liability().into();

        // Calculate position value in USD using borrow amount
        // The borrow_amount is in raw units (e.g., for USDC with 6 decimals: 1_000_000 = $1)
        let borrow_decimals = market_config
            .price_oracle_configuration
            .borrow_asset_decimals;
        #[allow(clippy::cast_sign_loss)]
        let decimals_divisor = 10u128.pow(borrow_decimals.max(0) as u32);
        let position_value_usd = Decimal::from(borrow_amount) / Decimal::from(decimals_divisor);

        // Determine alert zone
        let mcr_liquidation = market_config.borrow_mcr_liquidation;
        let yellow_threshold = mcr_liquidation * self.yellow_zone_multiplier;

        let zone = if cr < mcr_liquidation {
            AlertZone::Red
        } else if cr < yellow_threshold {
            AlertZone::Yellow
        } else {
            AlertZone::Green
        };

        tracing::debug!(
            market = %market,
            borrower = %borrower,
            cr = %cr,
            mcr = %mcr_liquidation,
            yellow_threshold = %yellow_threshold,
            zone = ?zone,
            collateral = collateral_amount,
            debt = borrow_amount,
            "Position analyzed"
        );

        // Skip green zones
        if zone == AlertZone::Green {
            return Ok(None);
        }

        // Calculate distance from MCR as percentage
        let distance_from_mcr_pct =
            ((cr - mcr_liquidation) / mcr_liquidation) * Decimal::from(100u32);

        Ok(Some(PositionAlert {
            borrower: borrower.clone(),
            collateralization_ratio: cr,
            position_value_usd,
            zone,
            distance_from_mcr_pct,
        }))
    }
}
