use near_sdk::{json_types::U64, near};

use crate::{
    asset::{
        BorrowAsset, BorrowAssetAmount, CollateralAsset, CollateralAssetAmount, FungibleAsset,
    },
    borrow::{BorrowPosition, BorrowStatus, LiquidationReason},
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    number::Decimal,
};

use super::{AssetConversion, AssetValuation, BalanceOracleConfiguration, PricePair, YieldWeights};

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct MarketConfiguration {
    pub borrow_asset: FungibleAsset<BorrowAsset>,
    pub collateral_asset: FungibleAsset<CollateralAsset>,
    pub balance_oracle: BalanceOracleConfiguration,
    pub minimum_initial_collateral_ratio: Decimal,
    pub minimum_collateral_ratio_per_borrow: Decimal,
    /// How much of the deposited principal may be lent out (up to 100%)?
    /// This is a matter of protection for supply providers.
    /// Set to 99% for starters.
    pub maximum_borrow_asset_usage_ratio: Decimal,
    /// The origination fee is a one-time amount added to the principal of the
    /// borrow. That is to say, the origination fee is denominated in units of
    /// the borrow asset and is paid by the borrowing account during repayment
    /// (or liquidation).
    pub borrow_origination_fee: Fee<BorrowAsset>,
    pub borrow_interest_rate_strategy: InterestRateStrategy,
    pub maximum_borrow_duration_ms: Option<U64>,
    pub minimum_borrow_amount: BorrowAssetAmount,
    pub maximum_borrow_amount: BorrowAssetAmount,
    pub supply_withdrawal_fee: TimeBasedFee<CollateralAsset>,
    pub yield_weights: YieldWeights,
    /// How far below market rate to accept liquidation? This is effectively the liquidator's spread.
    ///
    /// For example, if a 100USDC borrow is (under)collateralized with $110 of
    /// NEAR, a "maximum liquidator spread" of 10% would mean that a liquidator
    /// could liquidate this borrow by sending 109USDC, netting the liquidator
    /// ($110 - $100) * 10% = $1 of NEAR.
    pub maximum_liquidator_spread: Decimal,
}

pub mod error {
    use std::fmt::Display;

    use thiserror::Error;

    #[derive(Debug, Clone, Error)]
    #[error("Invalid configuration field `{field}`: {reason}")]
    pub struct ConfigurationValidationError {
        field: &'static str,
        reason: InvalidFieldReason,
    }

    #[derive(Debug, Clone)]
    pub enum InvalidFieldReason {
        OutOfBounds,
    }

    impl Display for InvalidFieldReason {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "out of bounds")
        }
    }

    pub(super) fn out_of_bounds(field: &'static str) -> ConfigurationValidationError {
        ConfigurationValidationError {
            field,
            reason: InvalidFieldReason::OutOfBounds,
        }
    }
}

impl MarketConfiguration {
    /// # Errors
    ///
    /// If the configuration is invalid.
    pub fn validate(&self) -> Result<(), error::ConfigurationValidationError> {
        if self.minimum_initial_collateral_ratio < 1u32 {
            return Err(error::out_of_bounds("minimum_initial_collateral_ratio"));
        }

        if self.minimum_collateral_ratio_per_borrow < 1u32 {
            return Err(error::out_of_bounds("minimum_collateral_ratio_per_borrow"));
        }

        if self.maximum_borrow_asset_usage_ratio.is_zero()
            || self.maximum_borrow_asset_usage_ratio > 1u32
        {
            return Err(error::out_of_bounds("maximum_borrow_asset_usage_ratio"));
        }

        if self.maximum_borrow_amount < self.minimum_borrow_amount {
            return Err(error::out_of_bounds("maximum_borrow_amount"));
        }

        if self.maximum_liquidator_spread >= 1u32 {
            return Err(error::out_of_bounds("maximum_liquidator_spread"));
        }

        Ok(())
    }

    pub fn borrow_status(
        &self,
        borrow_position: &BorrowPosition,
        price_pair: &PricePair,
        block_timestamp_ms: u64,
    ) -> BorrowStatus {
        if !self.is_within_minimum_collateral_ratio(borrow_position, price_pair) {
            return BorrowStatus::Liquidation(LiquidationReason::Undercollateralization);
        }

        if !self.is_within_maximum_borrow_duration(borrow_position, block_timestamp_ms) {
            return BorrowStatus::Liquidation(LiquidationReason::Expiration);
        }

        BorrowStatus::Healthy
    }

    fn is_within_maximum_borrow_duration(
        &self,
        borrow_position: &BorrowPosition,
        block_timestamp_ms: u64,
    ) -> bool {
        if let Some(U64(maximum_duration_ms)) = self.maximum_borrow_duration_ms {
            borrow_position
                .started_at_block_timestamp_ms
                .and_then(|U64(started_at_ms)| block_timestamp_ms.checked_sub(started_at_ms))
                .is_none_or(|duration_ms| duration_ms <= maximum_duration_ms)
        } else {
            true
        }
    }

    pub fn is_within_minimum_initial_collateral_ratio(
        &self,
        borrow_position: &BorrowPosition,
        oracle_price_proof: &PricePair,
    ) -> bool {
        is_within_mcr(
            &self.minimum_initial_collateral_ratio,
            borrow_position,
            oracle_price_proof,
        )
    }

    pub fn is_within_minimum_collateral_ratio(
        &self,
        borrow_position: &BorrowPosition,
        oracle_price_proof: &PricePair,
    ) -> bool {
        is_within_mcr(
            &self.minimum_collateral_ratio_per_borrow,
            borrow_position,
            oracle_price_proof,
        )
    }

    #[allow(clippy::missing_panics_doc)]
    pub fn minimum_acceptable_liquidation_amount(
        &self,
        amount: CollateralAssetAmount,
        price_pair: &PricePair,
    ) -> BorrowAssetAmount {
        BorrowAssetAmount::new(
            // Safe because the factor is guaranteed to be <=1, so the result
            // must still fit in u128.
            #[allow(clippy::unwrap_used)]
            ((1u32 - self.maximum_liquidator_spread)
                * price_pair.convert_pessimistic(amount).as_u128())
            .to_u128_ceil()
            .unwrap(),
        )
    }
}

fn is_within_mcr(mcr: &Decimal, borrow_position: &BorrowPosition, price_pair: &PricePair) -> bool {
    let scaled_collateral_value =
        price_pair.value_pessimistic(borrow_position.collateral_asset_deposit);
    let scaled_borrow_value =
        price_pair.value_optimistic(borrow_position.get_total_borrow_asset_liability());

    scaled_collateral_value >= scaled_borrow_value * mcr
}

#[cfg(test)]
mod tests {
    use crate::{borrow::InterestAccumulationProof, dec, oracle::pyth};

    use super::*;

    #[test]
    fn test_is_within_mcr() {
        let mut b = BorrowPosition::new(0);
        b.increase_collateral_asset_deposit(121u128.into());
        b.increase_borrow_asset_principal(InterestAccumulationProof::test(), 100u128.into(), 0);
        assert!(is_within_mcr(
            &dec!("1.2"),
            &b,
            &PricePair::new(
                18,
                &pyth::Price {
                    price: near_sdk::json_types::I64(10000),
                    conf: U64(1),
                    expo: -4,
                    publish_time: 0,
                },
                18,
                &pyth::Price {
                    price: near_sdk::json_types::I64(10000),
                    conf: U64(1),
                    expo: -4,
                    publish_time: 0,
                },
            )
            .unwrap()
        ));
    }
}
