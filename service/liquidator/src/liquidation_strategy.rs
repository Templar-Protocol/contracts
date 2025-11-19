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

/// Minimum liquidation amount for 6-decimal tokens (e.g., USDC, USDT)
/// This represents approximately $0.02 USD for stablecoins
const MIN_LIQUIDATION_AMOUNT_6_DECIMALS: u128 = 20_000;

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
/// This strategy uses a configured percentage of available funds to minimize
/// capital deployment while still profiting from liquidations.
///
/// # Benefits
///
/// - Controlled capital deployment
/// - Risk management through partial fund usage
/// - Faster execution with smaller amounts
/// - Multiple liquidation opportunities can be pursued
///
/// # Tradeoffs
///
/// - May not fully liquidate positions
/// - Requires multiple transactions for full capital deployment
/// - Position may remain partially underwater
#[derive(Debug, Clone, Copy)]
pub struct PartialLiquidationStrategy {
    /// Percentage of available funds to use (1-100)
    pub target_percentage: u8,
    /// Minimum profit margin in basis points (e.g., 50 = 0.5%)
    pub min_profit_margin_bps: u32,
}

impl PartialLiquidationStrategy {
    /// Creates a new partial liquidation strategy.
    ///
    /// # Arguments
    ///
    /// * `target_percentage` - Percentage of available funds to use (1-100)
    /// * `min_profit_margin_bps` - Minimum profit margin in basis points
    ///
    /// # Panics
    ///
    /// Panics if `target_percentage` is 0 or > 100.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use templar_liquidator::liquidation_strategy::PartialLiquidationStrategy;
    ///
    /// // Use 50% of available funds, require 0.5% profit margin
    /// let strategy = PartialLiquidationStrategy::new(50, 50);
    /// ```
    #[must_use]
    pub fn new(target_percentage: u8, min_profit_margin_bps: u32) -> Self {
        assert!(
            target_percentage > 0 && target_percentage <= 100,
            "Target percentage must be between 1 and 100"
        );

        Self {
            target_percentage,
            min_profit_margin_bps,
        }
    }

    /// Creates a strategy that uses 50% of available funds (recommended default).
    #[must_use]
    pub fn default_partial() -> Self {
        Self {
            target_percentage: 50,
            min_profit_margin_bps: 50, // 0.5% profit margin
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
        // For partial liquidation with fund-based strategy:
        // 1. Calculate percentage of available funds to use
        // 2. Ensure we don't exceed what's needed for the position
        // 3. Calculate corresponding collateral amount

        let available_u128: u128 = available_balance.into();

        // Calculate target liquidation amount (percentage of available funds)
        let target_amount = (available_u128 * u128::from(self.target_percentage)) / 100;

        if target_amount == 0 {
            tracing::warn!(
                available_balance = %available_u128,
                percentage = %self.target_percentage,
                "Target liquidation amount is zero"
            );
            return Ok(None);
        }

        let price_pair = configuration
            .price_oracle_configuration
            .create_price_pair(oracle_response)?;

        // Calculate liquidatable collateral for the position
        let liquidatable_collateral = position.liquidatable_collateral(
            &price_pair,
            configuration.borrow_mcr_liquidation,
            configuration.liquidation_maximum_spread,
        );

        // Calculate the minimum borrow amount needed to get all liquidatable collateral
        let max_liquidation_amount = configuration
            .minimum_acceptable_liquidation_amount(liquidatable_collateral, &price_pair);

        let Some(max_amount) = max_liquidation_amount else {
            tracing::warn!(
                liquidatable_collateral = %u128::from(liquidatable_collateral),
                "Could not calculate maximum liquidation amount from liquidatable collateral"
            );
            return Ok(None);
        };

        let max_amount_u128: u128 = max_amount.into();

        // Use the minimum of target amount and max liquidation amount
        let final_amount = std::cmp::min(target_amount, max_amount_u128);

        // Ensure the amount is economically viable (at least ~$0.02 USD value)
        // This prevents wasting gas on dust liquidations
        if final_amount < MIN_LIQUIDATION_AMOUNT_6_DECIMALS {
            tracing::warn!(
                amount = %final_amount,
                minimum_threshold = %MIN_LIQUIDATION_AMOUNT_6_DECIMALS,
                available_balance = %available_u128,
                "Liquidation amount too small to be economically viable (< $0.02)"
            );
            return Ok(None);
        }

        debug!(
            available_balance = %available_u128,
            target_amount = %target_amount,
            max_liquidation = %max_amount_u128,
            final_amount = %final_amount,
            percentage = %self.target_percentage,
            liquidatable_collateral = %u128::from(liquidatable_collateral),
            "Calculated partial liquidation amount based on available funds"
        );

        Ok(Some(U128(final_amount)))
    }

    #[tracing::instrument(skip(self), level = "debug")]
    fn should_liquidate(
        &self,
        liquidation_amount: U128,
        expected_collateral_value: U128,
        gas_cost_estimate: U128,
    ) -> LiquidatorResult<bool> {
        // Calculate total cost (liquidation amount + gas)
        // In inventory model: we spend liquidation_amount from inventory + gas
        let liquidation_u128: u128 = liquidation_amount.into();
        let gas_cost_u128: u128 = gas_cost_estimate.into();
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
}

impl FullLiquidationStrategy {
    /// Creates a new full liquidation strategy.
    #[must_use]
    pub fn new(min_profit_margin_bps: u32) -> Self {
        Self {
            min_profit_margin_bps,
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
        let buffer = amount_u128 / 1000; // 0.1% buffer
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
        let gas_cost_u128: u128 = gas_cost_estimate.into();

        let total_cost = liquidation_u128.saturating_add(gas_cost_u128);
        let profit_margin_multiplier = 10_000 + self.min_profit_margin_bps;
        let min_revenue = (total_cost * u128::from(profit_margin_multiplier)) / 10_000;

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
            is_profitable = %is_profitable,
            "Full liquidation profitability check (inventory-based)"
        );

        Ok(is_profitable)
    }

    fn strategy_name(&self) -> &'static str {
        "Full Liquidation"
    }
}

/// Fixed amount liquidation strategy.
///
/// This strategy uses a fixed amount (in token base units) per liquidation iteration.
/// Ideal for loop liquidation where you want consistent liquidation sizes.
///
/// # Benefits
///
/// - Predictable liquidation amounts
/// - Works well with loop liquidation
/// - Easy to reason about capital deployment
/// - Balance grows predictably with each profitable liquidation
///
/// # Use Cases
///
/// - Loop liquidation with consistent chunk sizes
/// - Risk management with fixed exposure per transaction
/// - Testing and simulation with predictable amounts
#[derive(Debug, Clone, Copy)]
pub struct FixedAmountLiquidationStrategy {
    /// Fixed amount to use per liquidation (in token base units, e.g., `1000_000000` for 1000 USDC)
    pub fixed_amount: u128,
    /// Minimum profit margin in basis points
    pub min_profit_margin_bps: u32,
}

impl FixedAmountLiquidationStrategy {
    /// Creates a new fixed amount liquidation strategy.
    ///
    /// # Arguments
    ///
    /// * `fixed_amount` - Fixed amount to use per liquidation (in token base units)
    /// * `min_profit_margin_bps` - Minimum profit margin in basis points
    ///
    /// # Example
    ///
    /// ```ignore
    /// use templar_liquidator::liquidation_strategy::FixedAmountLiquidationStrategy;
    ///
    /// // Use 1000 USDC (6 decimals) per liquidation
    /// let strategy = FixedAmountLiquidationStrategy::new(1000_000000, 50);
    /// ```
    #[must_use]
    pub fn new(fixed_amount: u128, min_profit_margin_bps: u32) -> Self {
        Self {
            fixed_amount,
            min_profit_margin_bps,
        }
    }
}

impl LiquidationStrategy for FixedAmountLiquidationStrategy {
    #[tracing::instrument(skip(self, position, oracle_response, configuration), level = "debug")]
    fn calculate_liquidation_amount(
        &self,
        position: &BorrowPosition,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
        available_balance: U128,
    ) -> LiquidatorResult<Option<U128>> {
        let available_u128: u128 = available_balance.into();

        // Check if we have enough balance for the fixed amount
        if self.fixed_amount > available_u128 {
            tracing::warn!(
                fixed_amount = %self.fixed_amount,
                available_balance = %available_u128,
                "Insufficient balance for fixed amount liquidation"
            );
            return Ok(None);
        }

        let price_pair = configuration
            .price_oracle_configuration
            .create_price_pair(oracle_response)?;

        // Calculate liquidatable collateral for the position
        let liquidatable_collateral = position.liquidatable_collateral(
            &price_pair,
            configuration.borrow_mcr_liquidation,
            configuration.liquidation_maximum_spread,
        );

        // Calculate the maximum borrow amount needed for all liquidatable collateral
        let max_liquidation_amount = configuration
            .minimum_acceptable_liquidation_amount(liquidatable_collateral, &price_pair);

        let Some(max_amount) = max_liquidation_amount else {
            tracing::warn!(
                liquidatable_collateral = %u128::from(liquidatable_collateral),
                "Could not calculate maximum liquidation amount from liquidatable collateral"
            );
            return Ok(None);
        };

        let max_amount_u128: u128 = max_amount.into();

        // Use the minimum of fixed amount and max liquidation amount
        let final_amount = std::cmp::min(self.fixed_amount, max_amount_u128);

        // Ensure the amount is economically viable
        if final_amount < MIN_LIQUIDATION_AMOUNT_6_DECIMALS {
            tracing::warn!(
                amount = %final_amount,
                minimum_threshold = %MIN_LIQUIDATION_AMOUNT_6_DECIMALS,
                "Liquidation amount too small to be economically viable (< $0.02)"
            );
            return Ok(None);
        }

        debug!(
            available_balance = %available_u128,
            fixed_amount = %self.fixed_amount,
            max_liquidation = %max_amount_u128,
            final_amount = %final_amount,
            liquidatable_collateral = %u128::from(liquidatable_collateral),
            "Calculated fixed amount liquidation"
        );

        Ok(Some(U128(final_amount)))
    }

    #[tracing::instrument(skip(self), level = "debug")]
    fn should_liquidate(
        &self,
        liquidation_amount: U128,
        expected_collateral_value: U128,
        gas_cost_estimate: U128,
    ) -> LiquidatorResult<bool> {
        // Same profitability logic as other strategies
        let liquidation_u128: u128 = liquidation_amount.into();
        let gas_cost_u128: u128 = gas_cost_estimate.into();

        let total_cost = liquidation_u128.saturating_add(gas_cost_u128);
        let profit_margin_multiplier = 10_000 + self.min_profit_margin_bps;
        let min_revenue = (total_cost * u128::from(profit_margin_multiplier)) / 10_000;

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
            is_profitable = %is_profitable,
            "Fixed amount liquidation profitability check"
        );

        Ok(is_profitable)
    }

    fn strategy_name(&self) -> &'static str {
        "Fixed Amount Liquidation"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partial_strategy_creation() {
        let strategy = PartialLiquidationStrategy::new(50, 50);
        assert_eq!(strategy.target_percentage, 50);
        assert_eq!(strategy.min_profit_margin_bps, 50);
        assert_eq!(strategy.strategy_name(), "Partial Liquidation");
        assert_eq!(strategy.max_liquidation_percentage(), 50);
    }

    #[test]
    #[should_panic(expected = "Target percentage must be between 1 and 100")]
    fn test_partial_strategy_invalid_percentage() {
        let _ = PartialLiquidationStrategy::new(0, 50);
    }

    #[test]
    #[should_panic(expected = "Target percentage must be between 1 and 100")]
    fn test_partial_strategy_percentage_too_high() {
        let _ = PartialLiquidationStrategy::new(101, 50);
    }

    #[test]
    fn test_full_strategy_creation() {
        let strategy = FullLiquidationStrategy::new(100);
        assert_eq!(strategy.min_profit_margin_bps, 100);
        assert_eq!(strategy.strategy_name(), "Full Liquidation");
        assert_eq!(strategy.max_liquidation_percentage(), 100);
    }

    #[test]
    fn test_profitability_check() {
        let strategy = PartialLiquidationStrategy::new(50, 50); // 0.5% profit margin

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

    // Note: Gas cost check removed - gas costs are negligible on NEAR
    // (typically < 0.1% of liquidation value even with 150 TGas at $100 NEAR)
}
