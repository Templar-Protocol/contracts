// SPDX-License-Identifier: MIT
//! Liquidation strategy implementations.
//!
//! This module provides flexible, configurable strategies for determining
//! liquidation amounts and profitability. The Strategy pattern enables:
//! - Partial vs. full liquidations
//! - Custom profitability calculations
//! - Risk management policies
//! - Gas cost optimization
//!
//! # Architecture
//!
//! Strategies implement the `LiquidationStrategy` trait, which provides
//! methods for calculating optimal liquidation amounts and determining
//! whether a liquidation should proceed based on profitability criteria.

use near_sdk::json_types::U128;
use templar_common::{
    borrow::BorrowPosition,
    market::MarketConfiguration,
    oracle::pyth::OracleResponse,
};
use tracing::{debug, instrument};

use crate::LiquidatorResult;

/// Core trait for liquidation strategies.
///
/// Implementations of this trait define how liquidation amounts are calculated
/// and whether liquidations should proceed based on profitability and risk criteria.
pub trait LiquidationStrategy: Send + Sync + std::fmt::Debug {
    /// Calculates the optimal liquidation amount for a position.
    ///
    /// # Arguments
    ///
    /// * `position` - The borrow position to liquidate
    /// * `oracle_response` - Current price oracle data
    /// * `configuration` - Market configuration
    /// * `available_balance` - Available balance in the liquidation asset
    ///
    /// # Returns
    ///
    /// The optimal liquidation amount in borrow asset units, or `None` if
    /// the position should not be liquidated.
    ///
    /// # Errors
    /// Returns an error if price pair retrieval fails or position calculations fail.
    fn calculate_liquidation_amount(
        &self,
        position: &BorrowPosition,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
        available_balance: U128,
    ) -> LiquidatorResult<Option<U128>>;

    /// Determines if a liquidation should proceed based on profitability.
    ///
    /// # Arguments
    ///
    /// * `swap_input_amount` - Amount of input asset required for swap
    /// * `liquidation_amount` - Amount to be used for liquidation (borrow asset)
    /// * `expected_collateral` - Expected collateral to receive
    /// * `gas_cost_estimate` - Estimated gas cost in NEAR
    ///
    /// # Returns
    ///
    /// `true` if the liquidation should proceed, `false` otherwise.
    ///
    /// # Errors
    /// Returns an error if profitability calculations fail.
    fn should_liquidate(
        &self,
        swap_input_amount: U128,
        liquidation_amount: U128,
        expected_collateral: U128,
        gas_cost_estimate: U128,
    ) -> LiquidatorResult<bool>;

    /// Returns the strategy name for logging and debugging.
    fn strategy_name(&self) -> &'static str;

    /// Returns the maximum liquidation percentage (0-100).
    ///
    /// # Default
    ///
    /// Returns 100 (full liquidation) by default.
    fn max_liquidation_percentage(&self) -> u8 {
        100
    }
}

/// Partial liquidation strategy.
///
/// This strategy liquidates a configured percentage of the position to minimize
/// market impact and gas costs while still profiting from the liquidation.
///
/// # Benefits
///
/// - Reduced market impact
/// - Lower gas costs
/// - Faster execution
/// - Multiple liquidators can participate
///
/// # Tradeoffs
///
/// - May leave position partially underwater
/// - Requires multiple transactions for full liquidation
/// - More complex profitability calculations
#[derive(Debug, Clone, Copy)]
pub struct PartialLiquidationStrategy {
    /// Target liquidation percentage (0-100)
    pub target_percentage: u8,
    /// Minimum profit margin in basis points (e.g., 50 = 0.5%)
    pub min_profit_margin_bps: u32,
    /// Maximum gas cost as percentage of liquidation value (e.g., 10 = 10%)
    pub max_gas_cost_percentage: u8,
}

impl PartialLiquidationStrategy {
    /// Creates a new partial liquidation strategy.
    ///
    /// # Arguments
    ///
    /// * `target_percentage` - Target liquidation percentage (1-100)
    /// * `min_profit_margin_bps` - Minimum profit margin in basis points
    /// * `max_gas_cost_percentage` - Maximum gas cost as percentage of value
    ///
    /// # Panics
    ///
    /// Panics if `target_percentage` is 0 or > 100.
    ///
    /// # Example
    ///
    /// ```
    /// use templar_bots::strategy::PartialLiquidationStrategy;
    ///
    /// // Liquidate 50% of position, require 0.5% profit margin, max 5% gas cost
    /// let strategy = PartialLiquidationStrategy::new(50, 50, 5);
    /// ```
    #[must_use]
    pub fn new(
        target_percentage: u8,
        min_profit_margin_bps: u32,
        max_gas_cost_percentage: u8,
    ) -> Self {
        assert!(
            target_percentage > 0 && target_percentage <= 100,
            "Target percentage must be between 1 and 100"
        );
        assert!(
            max_gas_cost_percentage <= 100,
            "Max gas cost percentage must be <= 100"
        );

        Self {
            target_percentage,
            min_profit_margin_bps,
            max_gas_cost_percentage,
        }
    }

    /// Creates a strategy that liquidates 50% of positions (recommended default).
    #[must_use]
    pub fn default_partial() -> Self {
        Self {
            target_percentage: 50,
            min_profit_margin_bps: 50, // 0.5% profit margin
            max_gas_cost_percentage: 10, // Max 10% gas cost
        }
    }

    /// Calculates the partial liquidation amount based on target percentage.
    fn calculate_partial_amount(
        self,
        full_amount: U128,
    ) -> U128 {
        #[allow(clippy::cast_lossless)]
        let percentage = self.target_percentage as u128;
        let full: u128 = full_amount.into();
        U128((full * percentage) / 100)
    }
}

impl LiquidationStrategy for PartialLiquidationStrategy {
    #[instrument(skip(self, position, oracle_response, configuration), level = "debug")]
    fn calculate_liquidation_amount(
        &self,
        position: &BorrowPosition,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
        available_balance: U128,
    ) -> LiquidatorResult<Option<U128>> {
        // Get the minimum acceptable liquidation amount (full liquidation)
        let price_pair = configuration
            .price_oracle_configuration
            .create_price_pair(oracle_response)?;

        let min_full_amount = configuration
            .minimum_acceptable_liquidation_amount(
                position.collateral_asset_deposit,
                &price_pair,
            );

        let Some(full_amount) = min_full_amount else {
            debug!("Could not calculate minimum liquidation amount");
            return Ok(None);
        };

        // Calculate partial amount based on target percentage
        let partial_amount = self.calculate_partial_amount(full_amount.into());

        // Ensure we don't exceed available balance
        let partial_u128: u128 = partial_amount.into();
        let available_u128: u128 = available_balance.into();

        let liquidation_amount = if partial_u128 > available_u128 {
            debug!(
                requested = %partial_u128,
                available = %available_u128,
                "Insufficient balance, using available amount"
            );
            available_balance
        } else {
            partial_amount
        };

        // Ensure the partial amount is still economically viable
        // (at least 10% of the full amount, or we're wasting gas)
        let full_u128: u128 = full_amount.into();
        let minimum_viable = U128((full_u128 * 10) / 100);
        let liquidation_u128: u128 = liquidation_amount.into();
        let min_viable_u128: u128 = minimum_viable.into();

        if liquidation_u128 < min_viable_u128 {
            debug!(
                amount = %liquidation_u128,
                minimum = %min_viable_u128,
                "Partial amount too small to be viable"
            );
            return Ok(None);
        }

        debug!(
            full_amount = %full_u128,
            partial_amount = %liquidation_u128,
            percentage = %self.target_percentage,
            "Calculated partial liquidation amount"
        );

        Ok(Some(liquidation_amount))
    }

    #[instrument(skip(self), level = "debug")]
    fn should_liquidate(
        &self,
        swap_input_amount: U128,
        liquidation_amount: U128,
        expected_collateral: U128,
        gas_cost_estimate: U128,
    ) -> LiquidatorResult<bool> {
        // Check gas cost is acceptable
        let liquidation_u128: u128 = liquidation_amount.into();
        #[allow(clippy::cast_lossless)]
        let max_gas_cost = (liquidation_u128 * self.max_gas_cost_percentage as u128) / 100;

        let gas_cost_u128: u128 = gas_cost_estimate.into();

        if gas_cost_u128 > max_gas_cost {
            debug!(
                gas_cost = %gas_cost_u128,
                max_allowed = %max_gas_cost,
                "Gas cost too high"
            );
            return Ok(false);
        }

        // Calculate total cost (swap input + gas)
        let swap_u128: u128 = swap_input_amount.into();
        let total_cost = swap_u128.saturating_add(gas_cost_u128);

        // Calculate minimum acceptable revenue based on profit margin
        let profit_margin_multiplier = 10_000 + self.min_profit_margin_bps;
        let min_revenue = (total_cost * u128::from(profit_margin_multiplier)) / 10_000;

        // Check if expected collateral meets minimum revenue requirement
        let collateral_u128: u128 = expected_collateral.into();
        let is_profitable = collateral_u128 >= min_revenue;

        debug!(
            total_cost = %total_cost,
            expected_collateral = %collateral_u128,
            min_revenue = %min_revenue,
            profit_margin_bps = %self.min_profit_margin_bps,
            is_profitable = %is_profitable,
            "Profitability check"
        );

        Ok(is_profitable)
    }

    fn strategy_name(&self) -> &'static str {
        "Partial Liquidation"
    }

    fn max_liquidation_percentage(&self) -> u8 {
        self.target_percentage
    }
}

/// Full liquidation strategy.
///
/// This strategy liquidates the entire position in a single transaction,
/// maximizing immediate profit but potentially incurring higher costs.
#[derive(Debug, Clone, Copy)]
pub struct FullLiquidationStrategy {
    /// Minimum profit margin in basis points
    pub min_profit_margin_bps: u32,
    /// Maximum gas cost as percentage of liquidation value
    pub max_gas_cost_percentage: u8,
}

impl FullLiquidationStrategy {
    /// Creates a new full liquidation strategy.
    #[must_use]
    pub fn new(min_profit_margin_bps: u32, max_gas_cost_percentage: u8) -> Self {
        Self {
            min_profit_margin_bps,
            max_gas_cost_percentage,
        }
    }

    /// Creates a conservative full liquidation strategy.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            min_profit_margin_bps: 100, // 1% profit margin
            max_gas_cost_percentage: 5, // Max 5% gas cost
        }
    }

    /// Creates an aggressive full liquidation strategy.
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            min_profit_margin_bps: 20, // 0.2% profit margin
            max_gas_cost_percentage: 15, // Max 15% gas cost
        }
    }
}

impl LiquidationStrategy for FullLiquidationStrategy {
    #[instrument(skip(self, position, oracle_response, configuration), level = "debug")]
    fn calculate_liquidation_amount(
        &self,
        position: &BorrowPosition,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
        available_balance: U128,
    ) -> LiquidatorResult<Option<U128>> {
        let price_pair = configuration
            .price_oracle_configuration
            .create_price_pair(oracle_response)?;

        let full_amount = configuration
            .minimum_acceptable_liquidation_amount(
                position.collateral_asset_deposit,
                &price_pair,
            );

        let Some(amount) = full_amount else {
            return Ok(None);
        };

        // Check if we have enough balance
        let amount_u128: u128 = amount.into();
        let available_u128: u128 = available_balance.into();

        if amount_u128 > available_u128 {
            debug!(
                required = %amount_u128,
                available = %available_u128,
                "Insufficient balance for full liquidation"
            );
            return Ok(None);
        }

        debug!(
            amount = %amount_u128,
            "Calculated full liquidation amount"
        );

        Ok(Some(amount.into()))
    }

    #[instrument(skip(self), level = "debug")]
    fn should_liquidate(
        &self,
        swap_input_amount: U128,
        liquidation_amount: U128,
        expected_collateral: U128,
        gas_cost_estimate: U128,
    ) -> LiquidatorResult<bool> {
        // Same profitability logic as partial strategy
        let liquidation_u128: u128 = liquidation_amount.into();
        #[allow(clippy::cast_lossless)]
        let max_gas_cost = (liquidation_u128 * self.max_gas_cost_percentage as u128) / 100;

        let gas_cost_u128: u128 = gas_cost_estimate.into();

        if gas_cost_u128 > max_gas_cost {
            debug!(
                gas_cost = %gas_cost_u128,
                max_allowed = %max_gas_cost,
                "Gas cost too high for full liquidation"
            );
            return Ok(false);
        }

        let swap_u128: u128 = swap_input_amount.into();
        let total_cost = swap_u128.saturating_add(gas_cost_u128);
        let profit_margin_multiplier = 10_000 + self.min_profit_margin_bps;
        let min_revenue = (total_cost * u128::from(profit_margin_multiplier)) / 10_000;

        let collateral_u128: u128 = expected_collateral.into();
        let is_profitable = collateral_u128 >= min_revenue;

        debug!(
            total_cost = %total_cost,
            expected_collateral = %collateral_u128,
            min_revenue = %min_revenue,
            is_profitable = %is_profitable,
            "Full liquidation profitability check"
        );

        Ok(is_profitable)
    }

    fn strategy_name(&self) -> &'static str {
        "Full Liquidation"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partial_strategy_creation() {
        let strategy = PartialLiquidationStrategy::new(50, 50, 10);
        assert_eq!(strategy.target_percentage, 50);
        assert_eq!(strategy.min_profit_margin_bps, 50);
        assert_eq!(strategy.strategy_name(), "Partial Liquidation");
        assert_eq!(strategy.max_liquidation_percentage(), 50);
    }

    #[test]
    #[should_panic(expected = "Target percentage must be between 1 and 100")]
    fn test_partial_strategy_invalid_percentage() {
        let _ = PartialLiquidationStrategy::new(0, 50, 10);
    }

    #[test]
    #[should_panic(expected = "Target percentage must be between 1 and 100")]
    fn test_partial_strategy_percentage_too_high() {
        let _ = PartialLiquidationStrategy::new(101, 50, 10);
    }

    #[test]
    fn test_partial_amount_calculation() {
        let strategy = PartialLiquidationStrategy::new(50, 50, 10);
        let full_amount = U128(1000);
        let partial = strategy.calculate_partial_amount(full_amount);
        assert_eq!(partial.0, 500);

        let strategy_25 = PartialLiquidationStrategy::new(25, 50, 10);
        let partial_25 = strategy_25.calculate_partial_amount(full_amount);
        assert_eq!(partial_25.0, 250);
    }

    #[test]
    fn test_full_strategy_creation() {
        let strategy = FullLiquidationStrategy::new(100, 5);
        assert_eq!(strategy.min_profit_margin_bps, 100);
        assert_eq!(strategy.strategy_name(), "Full Liquidation");
        assert_eq!(strategy.max_liquidation_percentage(), 100);
    }

    #[test]
    fn test_profitability_check() {
        let strategy = PartialLiquidationStrategy::new(50, 50, 10); // 0.5% profit margin

        // Profitable case: collateral > (cost * 1.005)
        // Cost: 1000, Min revenue: 1005, Collateral: 1010
        let is_profitable = strategy
            .should_liquidate(
                U128(900),      // swap input
                U128(1000),     // liquidation amount (for gas calc)
                U128(1010),     // expected collateral
                U128(100),      // gas cost
            )
            .unwrap();
        assert!(is_profitable, "Should be profitable");

        // Not profitable case: collateral < (cost * 1.005)
        // Cost: 1000, Min revenue: 1005, Collateral: 1000
        let is_not_profitable = strategy
            .should_liquidate(
                U128(900),
                U128(1000),
                U128(1000),     // collateral too low
                U128(100),
            )
            .unwrap();
        assert!(!is_not_profitable, "Should not be profitable");
    }

    #[test]
    fn test_gas_cost_check() {
        let strategy = PartialLiquidationStrategy::new(50, 50, 10); // Max 10% gas

        // Gas cost too high: 150 > 10% of 1000
        let too_expensive = strategy
            .should_liquidate(
                U128(900),
                U128(1000),     // liquidation amount
                U128(10000),    // high collateral
                U128(150),      // gas cost > 10%
            )
            .unwrap();
        assert!(!too_expensive, "Gas cost should be too high");

        // Acceptable gas cost: 50 < 10% of 1000
        let acceptable = strategy
            .should_liquidate(
                U128(900),
                U128(1000),
                U128(10000),
                U128(50),       // gas cost < 10%
            )
            .unwrap();
        assert!(acceptable, "Gas cost should be acceptable");
    }

    #[test]
    fn test_default_strategies() {
        let partial = PartialLiquidationStrategy::default_partial();
        assert_eq!(partial.target_percentage, 50);
        assert_eq!(partial.min_profit_margin_bps, 50);

        let conservative = FullLiquidationStrategy::conservative();
        assert_eq!(conservative.min_profit_margin_bps, 100);

        let aggressive = FullLiquidationStrategy::aggressive();
        assert_eq!(aggressive.min_profit_margin_bps, 20);
    }
}
