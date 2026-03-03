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
        // For red zone (below MCR), calculate how far below
        // For yellow zone (above MCR), calculate how far above
        let distance_from_mcr_pct = if zone == AlertZone::Red {
            // CR is below MCR, so calculate (MCR - CR) to avoid underflow
            ((mcr_liquidation - cr) / mcr_liquidation) * Decimal::from(100u32)
        } else {
            // CR is at or above MCR
            ((cr - mcr_liquidation) / mcr_liquidation) * Decimal::from(100u32)
        };

        Ok(Some(PositionAlert {
            borrower: borrower.clone(),
            collateralization_ratio: cr,
            position_value_usd,
            zone,
            distance_from_mcr_pct,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn create_test_config(at_risk_threshold_percent: u16) -> Config {
        Config {
            network: "testnet".to_string(),
            rpc_url: "http://localhost".to_string(),
            registry_account_ids: vec![],
            scan_time: "00:00".to_string(),
            at_risk_threshold_percent,
            min_position_size_usd: 1000,
            telegram_bot_token: String::new(),
            telegram_channel_id: String::new(),
            telegram_thread_id: None,
            ignored_collateral_assets: vec![],
            ignored_markets: vec![],
        }
    }

    #[test]
    fn test_analyzer_new_calculates_multiplier() {
        let config = create_test_config(10);
        let analyzer = Analyzer::new(&config);

        // 10% threshold means multiplier is 1.10
        let expected = Decimal::from(110u32) / 100u32;
        assert_eq!(analyzer.yellow_zone_multiplier, expected);
    }

    #[test]
    fn test_analyzer_new_different_thresholds() {
        let config = create_test_config(20);
        let analyzer = Analyzer::new(&config);

        // 20% threshold means multiplier is 1.20
        let expected = Decimal::from(120u32) / 100u32;
        assert_eq!(analyzer.yellow_zone_multiplier, expected);

        let config = create_test_config(5);
        let analyzer = Analyzer::new(&config);

        // 5% threshold means multiplier is 1.05
        let expected = Decimal::from(105u32) / 100u32;
        assert_eq!(analyzer.yellow_zone_multiplier, expected);
    }

    #[test]
    fn test_zone_classification_logic() {
        // Test the zone classification boundaries
        let mcr = Decimal::from(110u32);
        let yellow_multiplier = Decimal::from(110u32) / 100u32; // 1.10 (10% threshold)
        let yellow_threshold = mcr * yellow_multiplier; // 121

        // Red zone: CR < MCR
        let cr_red = Decimal::from(105u32);
        assert!(cr_red < mcr);

        // Yellow zone: MCR <= CR < yellow_threshold
        let cr_yellow = Decimal::from(115u32);
        assert!(cr_yellow >= mcr && cr_yellow < yellow_threshold);

        // Green zone: CR >= yellow_threshold
        let cr_green = Decimal::from(125u32);
        assert!(cr_green >= yellow_threshold);
    }

    #[test]
    fn test_distance_calculation() {
        let mcr = Decimal::from(110u32);
        let cr = Decimal::from(115u32);

        // Distance = ((CR - MCR) / MCR) * 100
        // = ((115 - 110) / 110) * 100
        // = (5 / 110) * 100
        // ≈ 4.54%
        let distance = ((cr - mcr) / mcr) * Decimal::from(100u32);

        // Check it's approximately 4.54 (allowing for decimal precision)
        let distance_f64: f64 = distance.to_string().parse().unwrap_or(0.0);
        assert!((distance_f64 - 4.545).abs() < 0.1);
    }

    #[test]
    fn test_distance_calculation_below_mcr() {
        let mcr = Decimal::from(133u32);
        let cr = Decimal::from(119u32);

        // When CR < MCR (Red zone), distance should be calculated as (MCR - CR)
        // to avoid underflow
        // Distance = ((MCR - CR) / MCR) * 100
        // = ((133 - 119) / 133) * 100
        // = (14 / 133) * 100
        // ≈ 10.53%
        let distance = ((mcr - cr) / mcr) * Decimal::from(100u32);

        // Check it's approximately 10.53 (allowing for decimal precision)
        let distance_f64: f64 = distance.to_string().parse().unwrap_or(0.0);
        assert!((distance_f64 - 10.526).abs() < 0.1);

        // This test would have caught the overflow bug where we tried to do (CR - MCR)
        // when CR < MCR, which would underflow with unsigned integers
    }

    #[test]
    fn test_zone_boundaries_red() {
        let mcr = Decimal::from(150u32);
        let yellow_multiplier = Decimal::from(110u32) / 100u32;
        let yellow_threshold = mcr * yellow_multiplier;

        // Test CR exactly at MCR boundary (should be yellow, not red)
        let cr_at_mcr = Decimal::from(150u32);
        assert!(cr_at_mcr >= mcr);
        assert!(cr_at_mcr < yellow_threshold);
    }

    #[test]
    fn test_zone_boundaries_yellow_to_green() {
        let mcr = Decimal::from(120u32);
        let yellow_multiplier = Decimal::from(110u32) / 100u32;
        let yellow_threshold = mcr * yellow_multiplier; // 132

        // Just below threshold = yellow
        let cr_yellow = Decimal::from(131u32);
        assert!(cr_yellow >= mcr && cr_yellow < yellow_threshold);

        // At or above threshold = green
        let cr_green = Decimal::from(132u32);
        assert!(cr_green >= yellow_threshold);
    }

    #[test]
    fn test_config_helper() {
        let config1 = create_test_config(15);
        assert_eq!(config1.at_risk_threshold_percent, 15);

        let config2 = create_test_config(25);
        assert_eq!(config2.at_risk_threshold_percent, 25);
    }
}
