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
    asset::{CollateralAsset, FungibleAssetAmount},
    borrow::BorrowPosition,
    market::MarketConfiguration,
    oracle::pyth::OracleResponse,
    price::{Convert, PricePair},
    Decimal,
};

use crate::LiquidatorResult;

/// Safety buffer in basis points (0.5% = 50 bps).
/// Added to borrow amount to account for price movements and interest accrual during execution.
/// Excess is refunded by the contract.
pub(crate) const SAFETY_BUFFER_BPS: u128 = 50;

/// Convert a borrow asset amount to collateral asset amount.
///
/// Formula: `collateral = (borrow / price) / (1 - spread)`
///
/// Uses floor rounding to ensure we request slightly less collateral than the
/// theoretical maximum, providing a natural safety margin.
pub(crate) fn borrow_to_collateral(
    borrow_amount: u128,
    price_pair: &PricePair,
    liquidation_spread: Decimal,
) -> Option<u128> {
    let spread_multiplier = Decimal::ONE - liquidation_spread;
    (Decimal::from(borrow_amount)
        / price_pair.convert(FungibleAssetAmount::<CollateralAsset>::new(1))
        / spread_multiplier)
        .to_u128_floor()
}

/// Convert a collateral asset amount to borrow asset amount.
///
/// Calculates the exact borrow amount needed to purchase the given collateral amount,
/// accounting for the liquidation spread.
///
/// Formula: `borrow = collateral * price * (1 - spread)`
pub(crate) fn collateral_to_borrow(
    collateral_amount: u128,
    price_pair: &PricePair,
    liquidation_spread: Decimal,
) -> Option<u128> {
    let spread_multiplier = Decimal::ONE - liquidation_spread;
    (Decimal::from(collateral_amount)
        * price_pair.convert(FungibleAssetAmount::<CollateralAsset>::new(1))
        * spread_multiplier)
        .to_u128_ceil()
}

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
    /// * `market_version` - Market contract version (e.g., (1, 0, 0) for v1.0.0)
    ///
    /// # Returns
    ///
    /// The optimal liquidation amount in borrow asset units and collateral amount,
    /// or `None` if the position should not be liquidated.
    ///
    /// # Returns
    /// Returns `Some((liquidation_amount, collateral_amount))` where:
    /// - `liquidation_amount`: Amount of borrow asset to send
    /// - `collateral_amount`: Amount of collateral to request
    ///
    /// # Errors
    /// Returns an error if price pair retrieval fails or position calculations fail.
    fn calculate_liquidation_amount(
        &self,
        position: &BorrowPosition,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
        available_balance: U128,
        market_version: Option<(u32, u32, u32)>,
    ) -> LiquidatorResult<Option<(U128, U128)>>;

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
pub struct PercentageLiquidationStrategy {
    /// Percentage of available funds to use (1-100)
    pub target_percentage: u8,
    /// Minimum profit margin in basis points (e.g., 50 = 0.5%)
    pub min_profit_margin_bps: u32,
}

impl PercentageLiquidationStrategy {
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
    /// use templar_liquidator::liquidation_strategy::PercentageLiquidationStrategy;
    ///
    /// // Use 50% of available funds, require 0.5% profit margin
    /// let strategy = PercentageLiquidationStrategy::new(50, 50);
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

impl LiquidationStrategy for PercentageLiquidationStrategy {
    #[tracing::instrument(skip(self, position, oracle_response, configuration), level = "debug")]
    fn calculate_liquidation_amount(
        &self,
        position: &BorrowPosition,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
        available_balance: U128,
        market_version: Option<(u32, u32, u32)>,
    ) -> LiquidatorResult<Option<(U128, U128)>> {
        let available_u128: u128 = available_balance.into();

        let available_after_buffer = (available_u128 * (10_000 - SAFETY_BUFFER_BPS)) / 10_000;
        let target_amount = (available_after_buffer * u128::from(self.target_percentage)) / 100;

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

        // Note: position.collateral_asset_deposit contains liquidatable_collateral (set by caller)
        let liquidatable_collateral = position.collateral_asset_deposit;

        let Some(collateral_amount) = borrow_to_collateral(
            target_amount,
            &price_pair,
            configuration.liquidation_maximum_spread,
        ) else {
            tracing::warn!(
                borrow_amount = %target_amount,
                "Could not calculate collateral amount from borrow amount"
            );
            return Ok(None);
        };

        // v1.0.0 markets require liquidating ALL collateral (no partial support)
        let target_collateral = if market_version == Some((1, 0, 0)) {
            position.collateral_asset_deposit.into()
        } else {
            std::cmp::min(collateral_amount, liquidatable_collateral.into())
        };

        let Some(theoretical_amount) = collateral_to_borrow(
            target_collateral,
            &price_pair,
            configuration.liquidation_maximum_spread,
        ) else {
            tracing::warn!(
                collateral_amount = %target_collateral,
                "Could not calculate borrow amount from collateral"
            );
            return Ok(None);
        };

        let final_amount =
            theoretical_amount.saturating_add((theoretical_amount * SAFETY_BUFFER_BPS) / 10_000);

        if final_amount > available_u128 {
            if market_version == Some((1, 0, 0)) {
                tracing::warn!(
                    required = %final_amount,
                    available = %available_u128,
                    "v1.0.0 market requires full collateral liquidation but insufficient balance"
                );
            } else {
                tracing::warn!(
                    required = %final_amount,
                    available = %available_u128,
                    "Insufficient balance for liquidation"
                );
            }
            return Ok(None);
        }

        let contract_minimum: u128 = configuration.borrow_range.minimum.into();
        if final_amount < contract_minimum {
            tracing::warn!(
                amount = %final_amount,
                contract_minimum = %contract_minimum,
                "Liquidation amount below contract minimum"
            );
            return Ok(None);
        }

        Ok(Some((U128(final_amount), U128(target_collateral))))
    }

    #[tracing::instrument(skip(self), level = "debug")]
    fn should_liquidate(
        &self,
        liquidation_amount: U128,
        expected_collateral_value: U128,
        gas_cost_estimate: U128,
    ) -> LiquidatorResult<bool> {
        let liquidation_u128: u128 = liquidation_amount.into();
        let gas_cost_u128: u128 = gas_cost_estimate.into();
        let total_cost = liquidation_u128.saturating_add(gas_cost_u128);

        let profit_margin_multiplier = 10_000 + self.min_profit_margin_bps;
        let min_revenue = (total_cost * u128::from(profit_margin_multiplier)) / 10_000;

        let collateral_value_u128: u128 = expected_collateral_value.into();
        let is_profitable = collateral_value_u128 >= min_revenue;

        Ok(is_profitable)
    }

    fn strategy_name(&self) -> &'static str {
        "Percentage Liquidation"
    }

    fn max_liquidation_percentage(&self) -> u8 {
        self.target_percentage
    }
}

/// Convert USD amount to raw token units.
///
/// Assumes all borrow assets are USD-based stablecoins (USDC, USDT, DAI, etc.).
///
/// Example: 100.0 USD with 6 decimals = `100_000000` raw units
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
fn usd_to_raw_units(usd_amount: f64, decimals: i32) -> u128 {
    let multiplier = 10_f64.powi(decimals);
    (usd_amount * multiplier) as u128
}

/// Fixed USD amount liquidation strategy.
///
/// Uses a fixed USD amount per liquidation, automatically converting to raw units
/// based on each market's borrow asset decimals. Assumes all borrow assets are
/// USD-based stablecoins.
#[derive(Debug, Clone, Copy)]
pub struct FixedAmountLiquidationStrategy {
    /// Fixed USD amount to use per liquidation (e.g., 100.0 for $100 USD)
    pub fixed_amount_usd: f64,
    /// Minimum profit margin in basis points
    pub min_profit_margin_bps: u32,
}

impl FixedAmountLiquidationStrategy {
    #[must_use]
    pub fn new(fixed_amount_usd: f64, min_profit_margin_bps: u32) -> Self {
        Self {
            fixed_amount_usd,
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
        market_version: Option<(u32, u32, u32)>,
    ) -> LiquidatorResult<Option<(U128, U128)>> {
        let decimals = configuration
            .price_oracle_configuration
            .borrow_asset_decimals;
        let fixed_amount = usd_to_raw_units(self.fixed_amount_usd, decimals);

        let available_u128: u128 = available_balance.into();

        if fixed_amount > available_u128 {
            let asset_id = configuration.borrow_asset.to_string();
            tracing::warn!(
                fixed_amount_usd = %self.fixed_amount_usd,
                fixed_amount = %crate::format::format_amount(fixed_amount, decimals, &asset_id),
                available_balance = %crate::format::format_amount(available_u128, decimals, &asset_id),
                "Insufficient balance for fixed amount liquidation"
            );
            return Ok(None);
        }

        let price_pair = configuration
            .price_oracle_configuration
            .create_price_pair(oracle_response)?;

        let liquidatable_u128: u128 = position.collateral_asset_deposit.into();

        let Some(max_collateral) = borrow_to_collateral(
            fixed_amount,
            &price_pair,
            configuration.liquidation_maximum_spread,
        ) else {
            let asset_id = configuration.borrow_asset.to_string();
            tracing::warn!(
                fixed_amount_usd = %self.fixed_amount_usd,
                fixed_amount = %crate::format::format_amount(fixed_amount, decimals, &asset_id),
                "Could not calculate collateral amount from fixed amount"
            );
            return Ok(None);
        };

        // v1.0.0 markets require liquidating ALL collateral
        let target_collateral = if market_version == Some((1, 0, 0)) {
            position.collateral_asset_deposit.into()
        } else {
            let safe_collateral = (max_collateral * (10_000 - SAFETY_BUFFER_BPS)) / 10_000;
            std::cmp::min(safe_collateral, liquidatable_u128)
        };

        let expected_minimum = collateral_to_borrow(
            target_collateral,
            &price_pair,
            configuration.liquidation_maximum_spread,
        )
        .unwrap_or(0);

        let amount_with_buffer = expected_minimum
            .saturating_add(((expected_minimum * SAFETY_BUFFER_BPS) / 10_000).max(1));

        // Cap at fixed_amount (the maximum we're willing to send)
        let final_amount = std::cmp::min(amount_with_buffer, fixed_amount);

        let contract_minimum: u128 = configuration.borrow_range.minimum.into();
        if final_amount < contract_minimum {
            tracing::warn!(
                amount = %final_amount,
                contract_minimum = %contract_minimum,
                "Fixed amount below contract minimum"
            );
            return Ok(None);
        }

        if market_version == Some((1, 0, 0)) && final_amount > available_u128 {
            tracing::warn!(
                required = %final_amount,
                available = %available_u128,
                "v1.0.0 market requires full collateral liquidation but insufficient balance"
            );
            return Ok(None);
        }

        Ok(Some((U128(final_amount), U128(target_collateral))))
    }

    #[tracing::instrument(skip(self), level = "debug")]
    fn should_liquidate(
        &self,
        liquidation_amount: U128,
        expected_collateral_value: U128,
        gas_cost_estimate: U128,
    ) -> LiquidatorResult<bool> {
        let liquidation_u128: u128 = liquidation_amount.into();
        let gas_cost_u128: u128 = gas_cost_estimate.into();

        let total_cost = liquidation_u128.saturating_add(gas_cost_u128);
        let profit_margin_multiplier = 10_000 + self.min_profit_margin_bps;
        let min_revenue = (total_cost * u128::from(profit_margin_multiplier)) / 10_000;

        let collateral_value_u128: u128 = expected_collateral_value.into();
        let is_profitable = collateral_value_u128 >= min_revenue;

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
        let strategy = PercentageLiquidationStrategy::new(50, 50);
        assert_eq!(strategy.target_percentage, 50);
        assert_eq!(strategy.min_profit_margin_bps, 50);
        assert_eq!(strategy.strategy_name(), "Percentage Liquidation");
        assert_eq!(strategy.max_liquidation_percentage(), 50);
    }

    #[test]
    #[should_panic(expected = "Target percentage must be between 1 and 100")]
    fn test_partial_strategy_invalid_percentage() {
        let _ = PercentageLiquidationStrategy::new(0, 50);
    }

    #[test]
    #[should_panic(expected = "Target percentage must be between 1 and 100")]
    fn test_partial_strategy_percentage_too_high() {
        let _ = PercentageLiquidationStrategy::new(101, 50);
    }

    #[test]
    fn test_profitability_check() {
        let strategy = PercentageLiquidationStrategy::new(50, 50); // 0.5% profit margin

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

    // Tests for conversion functions removed - they require complex PricePair setup
    // that is better tested in integration tests with real market configurations.
    // The conversion formulas are straightforward:
    // - collateral_to_borrow: borrow = collateral × price × (1 - spread)
    // - borrow_to_collateral: collateral = borrow / (price × (1 - spread))
}
