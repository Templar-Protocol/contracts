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
    borrow::BorrowPosition, market::MarketConfiguration, oracle::pyth::OracleResponse,
};
use tracing::debug;

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
    /// In the inventory-based model, we liquidate using available inventory,
    /// so there's no swap cost. Profitability is based purely on:
    /// - Expected collateral value vs liquidation amount
    /// - Gas cost
    ///
    /// # Arguments
    ///
    /// * `liquidation_amount` - Amount to be used for liquidation (borrow asset)
    /// * `expected_collateral_value` - Expected value of collateral in borrow asset units
    /// * `gas_cost_estimate` - Estimated gas cost in borrow asset units
    ///
    /// # Returns
    ///
    /// `true` if the liquidation should proceed, `false` otherwise.
    ///
    /// # Errors
    /// Returns an error if profitability calculations fail.
    fn should_liquidate(
        &self,
        liquidation_amount: U128,
        expected_collateral_value: U128,
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
            min_profit_margin_bps: 50,   // 0.5% profit margin
            max_gas_cost_percentage: 10, // Max 10% gas cost
        }
    }
}

impl LiquidationStrategy for PartialLiquidationStrategy {
    #[tracing::instrument(skip(self, position, oracle_response, configuration), level = "debug")]
    fn calculate_liquidation_amount(
        &self,
        position: &BorrowPosition,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
        available_balance: U128,
    ) -> LiquidatorResult<Option<U128>> {
        // For partial liquidation:
        // 1. Calculate target collateral (percentage of total)
        // 2. Calculate minimum borrow amount needed for that collateral
        // This ensures the liquidation amount matches the collateral we'll request

        let price_pair = configuration
            .price_oracle_configuration
            .create_price_pair(oracle_response)?;

        // Calculate target collateral amount (e.g., 50% of total)
        let total_collateral = position.collateral_asset_deposit;
        let target_collateral_u128 =
            u128::from(total_collateral) * u128::from(self.target_percentage) / 100;
        let target_collateral = templar_common::asset::FungibleAssetAmount::<
            templar_common::asset::CollateralAsset,
        >::new(target_collateral_u128);

        // Calculate minimum acceptable liquidation amount for this collateral
        let min_for_target =
            configuration.minimum_acceptable_liquidation_amount(target_collateral, &price_pair);

        let Some(liquidation_amount) = min_for_target else {
            tracing::warn!(
                target_collateral = %target_collateral_u128,
                "Could not calculate minimum liquidation amount from target collateral"
            );
            return Ok(None);
        };

        // Add a small buffer (0.1%) to account for rounding differences
        let liquidation_u128: u128 = liquidation_amount.into();
        let buffer = (liquidation_u128 * 1) / 1000; // 0.1% buffer
        let liquidation_with_buffer = liquidation_u128.saturating_add(buffer.max(1));

        // Ensure we don't exceed available balance
        let available_u128: u128 = available_balance.into();

        let final_liquidation_amount = if liquidation_with_buffer > available_u128 {
            debug!(
                requested = %liquidation_with_buffer,
                available = %available_u128,
                "Insufficient balance, using available amount"
            );
            available_balance
        } else {
            U128(liquidation_with_buffer)
        };

        // Ensure the amount is still economically viable
        // (at least 10% of full liquidation, or we're wasting gas)
        let full_amount =
            configuration.minimum_acceptable_liquidation_amount(total_collateral, &price_pair);

        if let Some(full) = full_amount {
            let full_u128: u128 = full.into();
            let minimum_viable = U128((full_u128 * 10) / 100);
            let final_u128: u128 = final_liquidation_amount.into();
            let min_viable_u128: u128 = minimum_viable.into();

            if final_u128 < min_viable_u128 {
                tracing::warn!(
                    amount = %final_u128,
                    minimum_viable = %min_viable_u128,
                    full_amount = %full_u128,
                    available_balance = %available_u128,
                    "Liquidation amount too small to be economically viable (< 10% of full amount)"
                );
                return Ok(None);
            }
        }

        debug!(
            target_collateral = %target_collateral_u128,
            total_collateral = %u128::from(total_collateral),
            liquidation_amount = %liquidation_with_buffer,
            base_amount = %liquidation_u128,
            buffer = %buffer,
            percentage = %self.target_percentage,
            "Calculated partial liquidation amount with buffer"
        );

        Ok(Some(final_liquidation_amount))
    }

    #[tracing::instrument(skip(self), level = "debug")]
    fn should_liquidate(
        &self,
        liquidation_amount: U128,
        expected_collateral_value: U128,
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

        // Calculate total cost (liquidation amount + gas)
        // In inventory model: we spend liquidation_amount from inventory + gas
        let total_cost = liquidation_u128.saturating_add(gas_cost_u128);

        // Calculate minimum acceptable revenue based on profit margin
        let profit_margin_multiplier = 10_000 + self.min_profit_margin_bps;
        let min_revenue = (total_cost * u128::from(profit_margin_multiplier)) / 10_000;

        // Check if expected collateral value meets minimum revenue requirement
        let collateral_value_u128: u128 = expected_collateral_value.into();
        let is_profitable = collateral_value_u128 >= min_revenue;

        let net_profit = collateral_value_u128.saturating_sub(total_cost);

        debug!(
            liquidation_amount = %liquidation_u128,
            gas_cost = %gas_cost_u128,
            total_cost = %total_cost,
            expected_collateral_value = %collateral_value_u128,
            min_revenue = %min_revenue,
            net_profit = %net_profit,
            profit_margin_bps = %self.min_profit_margin_bps,
            is_profitable = %is_profitable,
            "Profitability check (inventory-based)"
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
            min_profit_margin_bps: 20,   // 0.2% profit margin
            max_gas_cost_percentage: 15, // Max 15% gas cost
        }
    }
}

impl LiquidationStrategy for FullLiquidationStrategy {
    #[tracing::instrument(skip(self, position, oracle_response, configuration), level = "debug")]
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
            .minimum_acceptable_liquidation_amount(position.collateral_asset_deposit, &price_pair);

        let Some(amount) = full_amount else {
            tracing::warn!(
                collateral_deposit = %position.collateral_asset_deposit,
                "Could not calculate full liquidation amount from collateral"
            );
            return Ok(None);
        };

        // Add a small buffer (0.1%) to account for rounding differences
        // between bot calculation and contract calculation
        let amount_u128: u128 = amount.into();
        let buffer = (amount_u128 * 1) / 1000; // 0.1% buffer
        let amount_with_buffer = amount_u128.saturating_add(buffer.max(1));

        // Check if we have enough balance
        let available_u128: u128 = available_balance.into();

        if amount_with_buffer > available_u128 {
            tracing::warn!(
                required = %amount_with_buffer,
                available = %available_u128,
                "Insufficient inventory balance for full liquidation"
            );
            return Ok(None);
        }

        debug!(
            amount = %amount_with_buffer,
            base_amount = %amount_u128,
            buffer = %buffer,
            "Calculated full liquidation amount with buffer"
        );

        Ok(Some(U128(amount_with_buffer)))
    }

    #[tracing::instrument(skip(self), level = "debug")]
    fn should_liquidate(
        &self,
        liquidation_amount: U128,
        expected_collateral_value: U128,
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

        let total_cost = liquidation_u128.saturating_add(gas_cost_u128);
        let profit_margin_multiplier = 10_000 + self.min_profit_margin_bps;
        let min_revenue = (total_cost * u128::from(profit_margin_multiplier)) / 10_000;

        let collateral_value_u128: u128 = expected_collateral_value.into();
        let is_profitable = collateral_value_u128 >= min_revenue;

        debug!(
            total_cost = %total_cost,
            expected_collateral_value = %collateral_value_u128,
            min_revenue = %min_revenue,
            is_profitable = %is_profitable,
            "Full liquidation profitability check (inventory-based)"
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

        // Profitable case: collateral_value > (liquidation_amount + gas) * 1.005
        // Cost: 1100 (1000 liquidation + 100 gas), Min revenue: 1105, Collateral: 1110
        let is_profitable = strategy
            .should_liquidate(
                U128(1000), // liquidation amount
                U128(1110), // expected collateral value
                U128(100),  // gas cost
            )
            .unwrap();
        assert!(is_profitable, "Should be profitable");

        // Not profitable case: collateral_value < (liquidation_amount + gas) * 1.005
        // Cost: 1100, Min revenue: 1105, Collateral: 1100
        let is_not_profitable = strategy
            .should_liquidate(
                U128(1000), // liquidation amount
                U128(1100), // collateral value too low
                U128(100),  // gas cost
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
                U128(1000),  // liquidation amount
                U128(10000), // high collateral value
                U128(150),   // gas cost > 10%
            )
            .unwrap();
        assert!(!too_expensive, "Gas cost should be too high");

        // Acceptable gas cost: 50 < 10% of 1000
        let acceptable = strategy
            .should_liquidate(
                U128(1000),  // liquidation amount
                U128(10000), // high collateral value
                U128(50),    // gas cost < 10%
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
