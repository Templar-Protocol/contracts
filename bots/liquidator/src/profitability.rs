//! Profitability calculation module.
//!
//! Handles cost/profit calculations for liquidations including:
//! - Gas cost conversions
//! - Collateral value conversions
//! - Profitability metrics

use near_sdk::json_types::U128;
use templar_common::{market::MarketConfiguration, oracle::pyth::OracleResponse};
use tracing::debug;

use crate::{LiquidatorError, LiquidatorResult};

/// Profitability calculator for liquidations.
///
/// Responsible for:
/// - Converting gas costs to borrow asset units
/// - Converting collateral to borrow asset value
/// - Calculating profit metrics
pub struct ProfitabilityCalculator;

impl ProfitabilityCalculator {
    /// Default gas cost estimate in USD
    /// ~$0.05 USD for a liquidation transaction (conservative estimate for 0.01 NEAR at ~$5)
    pub const DEFAULT_GAS_COST_USD: f64 = 0.05;

    /// Converts USD gas cost estimate to borrow asset units using oracle prices.
    ///
    /// Formula: `gas_cost_borrow_asset = gas_cost_usd / borrow_asset_usd_price * 10^borrow_decimals`
    ///
    /// # Arguments
    ///
    /// * `gas_cost_usd` - Gas cost in USD (e.g., 0.05 for $0.05)
    /// * `oracle_response` - Oracle price data containing borrow asset/USD price
    /// * `configuration` - Market configuration containing borrow asset price ID and decimals
    ///
    /// # Returns
    ///
    /// Gas cost denominated in borrow asset base units
    ///
    /// # Errors
    ///
    /// Returns an error if the borrow asset price is not found in the oracle response
    pub fn convert_gas_cost_to_borrow_asset(
        gas_cost_usd: f64,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
    ) -> LiquidatorResult<U128> {
        // Get borrow asset price from oracle configuration
        let borrow_price_id = configuration
            .price_oracle_configuration
            .borrow_asset_price_id;
        let borrow_decimals = configuration
            .price_oracle_configuration
            .borrow_asset_decimals;

        let borrow_price = oracle_response
            .get(&borrow_price_id)
            .and_then(|opt| opt.as_ref())
            .ok_or_else(|| {
                LiquidatorError::StrategyError("Borrow asset price not found in oracle".to_string())
            })?;

        // Convert price to USD value
        // Price format: price * 10^expo
        // Note: i64 to f64 conversion may lose precision, but acceptable for price calculations
        #[allow(clippy::cast_precision_loss)]
        let borrow_usd = (borrow_price.price.0 as f64) * 10f64.powi(borrow_price.expo);

        // Convert gas cost from USD to borrow asset
        // gas_cost_borrow = (gas_cost_usd / borrow_usd) * 10^borrow_decimals
        let gas_cost_borrow = (gas_cost_usd / borrow_usd) * 10f64.powi(borrow_decimals);

        // Note: f64 to u128 conversion may truncate, but result should fit within u128 range
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(U128(gas_cost_borrow as u128))
    }

    /// Converts collateral asset amount to borrow asset units using oracle prices.
    ///
    /// Formula: `borrow_value = (collateral_amount * collateral_usd_price) / borrow_usd_price`
    ///
    /// # Arguments
    ///
    /// * `collateral_amount` - Amount in collateral asset base units
    /// * `oracle_response` - Oracle price data containing both asset prices
    /// * `configuration` - Market configuration containing price IDs and decimals
    ///
    /// # Returns
    ///
    /// Collateral value denominated in borrow asset base units
    ///
    /// # Errors
    ///
    /// Returns an error if collateral or borrow asset prices are not found in the oracle response
    pub fn convert_collateral_to_borrow_asset(
        collateral_amount: U128,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
    ) -> LiquidatorResult<U128> {
        let oracle_config = &configuration.price_oracle_configuration;

        // Get collateral price
        let collateral_price = oracle_response
            .get(&oracle_config.collateral_asset_price_id)
            .and_then(|opt| opt.as_ref())
            .ok_or_else(|| {
                LiquidatorError::StrategyError(
                    "Collateral asset price not found in oracle".to_string(),
                )
            })?;

        // Get borrow price
        let borrow_price = oracle_response
            .get(&oracle_config.borrow_asset_price_id)
            .and_then(|opt| opt.as_ref())
            .ok_or_else(|| {
                LiquidatorError::StrategyError("Borrow asset price not found in oracle".to_string())
            })?;

        // Convert prices to f64 for calculation
        // Price format: price * 10^expo
        // Note: i64 to f64 may lose precision, acceptable for price calculations
        #[allow(clippy::cast_precision_loss)]
        let collateral_usd = (collateral_price.price.0 as f64) * 10f64.powi(collateral_price.expo);
        #[allow(clippy::cast_precision_loss)]
        let borrow_usd = (borrow_price.price.0 as f64) * 10f64.powi(borrow_price.expo);

        // Convert collateral to borrow asset units
        // Step 1: Convert collateral to USD value
        #[allow(clippy::cast_precision_loss)]
        let collateral_amount_f64 = collateral_amount.0 as f64;
        let collateral_decimals = oracle_config.collateral_asset_decimals;
        let collateral_value_usd =
            (collateral_amount_f64 / 10f64.powi(collateral_decimals)) * collateral_usd;

        // Step 2: Convert USD value to borrow asset units
        let borrow_decimals = oracle_config.borrow_asset_decimals;
        let borrow_value = (collateral_value_usd / borrow_usd) * 10f64.powi(borrow_decimals);

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(U128(borrow_value as u128))
    }

    /// Calculates detailed profitability metrics for a liquidation.
    ///
    /// Returns (`net_profit`, `profit_percentage`)
    pub fn calculate_profit_metrics(
        liquidation_amount: U128,
        expected_collateral_value: U128,
        gas_cost: U128,
    ) -> (u128, u64) {
        let liquidation_cost = liquidation_amount.0;
        let gas_cost_u128 = gas_cost.0;
        let total_cost = liquidation_cost + gas_cost_u128;
        let expected_revenue = expected_collateral_value.0;

        let net_profit = expected_revenue.saturating_sub(total_cost);

        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let profit_percentage = if total_cost > 0 {
            ((net_profit as f64 / total_cost as f64) * 100.0) as u64
        } else {
            0
        };

        debug!(
            liquidation_cost = %liquidation_cost,
            gas_cost = %gas_cost_u128,
            total_cost = %total_cost,
            expected_revenue = %expected_revenue,
            net_profit = %net_profit,
            profit_percentage = %profit_percentage,
            "Calculated profitability metrics"
        );

        (net_profit, profit_percentage)
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::json_types::U128;

    use super::ProfitabilityCalculator;

    #[test]
    fn test_calculate_profit_metrics_basic() {
        let liquidation_amount = U128(1000);
        let expected_collateral = U128(1200); // 20% profit before gas
        let gas_cost = U128(50);

        let (net_profit, profit_pct) = ProfitabilityCalculator::calculate_profit_metrics(
            liquidation_amount,
            expected_collateral,
            gas_cost,
        );

        // Net profit: 1200 - (1000 + 50) = 150
        assert_eq!(net_profit, 150);
        // Profit %: (150 / 1050) * 100 = 14%
        assert_eq!(profit_pct, 14);
    }

    #[test]
    fn test_calculate_profit_metrics_zero_profit() {
        let liquidation_amount = U128(1000);
        let expected_collateral = U128(1000);
        let gas_cost = U128(0);

        let (net_profit, profit_pct) = ProfitabilityCalculator::calculate_profit_metrics(
            liquidation_amount,
            expected_collateral,
            gas_cost,
        );

        assert_eq!(net_profit, 0);
        assert_eq!(profit_pct, 0);
    }

    #[test]
    fn test_calculate_profit_metrics_loss() {
        let liquidation_amount = U128(1000);
        let expected_collateral = U128(900); // 10% loss
        let gas_cost = U128(50);

        let (net_profit, profit_pct) = ProfitabilityCalculator::calculate_profit_metrics(
            liquidation_amount,
            expected_collateral,
            gas_cost,
        );

        // Loss scenario: 900 - 1050 = -150, but saturating_sub makes it 0
        assert_eq!(net_profit, 0);
        assert_eq!(profit_pct, 0);
    }

    #[test]
    fn test_calculate_profit_metrics_high_profit() {
        let liquidation_amount = U128(1000);
        let expected_collateral = U128(2000); // 100% profit before gas
        let gas_cost = U128(100);

        let (net_profit, profit_pct) = ProfitabilityCalculator::calculate_profit_metrics(
            liquidation_amount,
            expected_collateral,
            gas_cost,
        );

        // Net profit: 2000 - 1100 = 900
        assert_eq!(net_profit, 900);
        // Profit %: (900 / 1100) * 100 = 81%
        assert_eq!(profit_pct, 81);
    }

    #[test]
    fn test_calculate_profit_metrics_zero_cost() {
        let liquidation_amount = U128(0);
        let expected_collateral = U128(1000);
        let gas_cost = U128(0);

        let (net_profit, profit_pct) = ProfitabilityCalculator::calculate_profit_metrics(
            liquidation_amount,
            expected_collateral,
            gas_cost,
        );

        assert_eq!(net_profit, 1000);
        // Division by zero protected, returns 0
        assert_eq!(profit_pct, 0);
    }

    #[test]
    fn test_calculate_profit_metrics_with_gas() {
        let liquidation_amount = U128(10_000);
        let expected_collateral = U128(11_500);
        let gas_cost = U128(500);

        let (net_profit, profit_pct) = ProfitabilityCalculator::calculate_profit_metrics(
            liquidation_amount,
            expected_collateral,
            gas_cost,
        );

        // Net profit: 11500 - (10000 + 500) = 1000
        assert_eq!(net_profit, 1000);
        // Profit %: (1000 / 10500) * 100 = 9%
        assert_eq!(profit_pct, 9);
    }

    #[test]
    fn test_calculate_profit_metrics_large_amounts() {
        let liquidation_amount = U128(1_000_000_000_000); // 1T units
        let expected_collateral = U128(1_100_000_000_000); // 10% profit
        let gas_cost = U128(1_000_000_000); // 1B units

        let (net_profit, profit_pct) = ProfitabilityCalculator::calculate_profit_metrics(
            liquidation_amount,
            expected_collateral,
            gas_cost,
        );

        // Should handle large numbers
        assert!(net_profit > 0);
        assert!(profit_pct < 100);
    }

    #[test]
    fn test_calculate_profit_metrics_minimal_profit() {
        let liquidation_amount = U128(10_000);
        let expected_collateral = U128(10_101); // ~1% profit
        let gas_cost = U128(100);

        let (net_profit, profit_pct) = ProfitabilityCalculator::calculate_profit_metrics(
            liquidation_amount,
            expected_collateral,
            gas_cost,
        );

        // Net profit: 10101 - 10100 = 1
        assert_eq!(net_profit, 1);
        // Profit %: very small, likely rounds to 0
        assert_eq!(profit_pct, 0);
    }

    #[test]
    fn test_calculate_profit_metrics_percentage_rounding() {
        let liquidation_amount = U128(1000);
        let expected_collateral = U128(1550); // 55% profit before gas
        let gas_cost = U128(0);

        let (net_profit, profit_pct) = ProfitabilityCalculator::calculate_profit_metrics(
            liquidation_amount,
            expected_collateral,
            gas_cost,
        );

        assert_eq!(net_profit, 550);
        // Profit %: (550 / 1000) * 100 = 55%
        assert_eq!(profit_pct, 55);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_default_gas_cost_constant() {
        // Verify the default gas cost constant is reasonable
        assert!(ProfitabilityCalculator::DEFAULT_GAS_COST_USD > 0.0);
        assert!(ProfitabilityCalculator::DEFAULT_GAS_COST_USD < 1.0);
    }
}
