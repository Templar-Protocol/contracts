use std::{io::ErrorKind, ops::Deref};

use near_sdk::{borsh, json_types::U64, near, AccountId};

use crate::{
    asset::{
        AssetClass, BorrowAsset, BorrowAssetAmount, CollateralAsset, CollateralAssetAmount,
        FungibleAsset, FungibleAssetAmount,
    },
    borrow::{BorrowPosition, BorrowStatus, LiquidationReason},
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    number::Decimal,
    price::{PricePair, Valuation},
    time_chunk::TimeChunkConfiguration,
};

use super::{PriceOracleConfiguration, YieldWeights};

/// Reject >10,000,000% APY interest rates as misconfigurations.
/// This also guarantees a reasonable upper-limit to interest rates to help avoid overflows.
pub const APY_LIMIT: u128 = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
#[serde(try_from = "AmountRange::<A>")]
pub struct ValidAmountRange<A: AssetClass + PartialOrd>(
    #[borsh(deserialize_with = "deserialize_valid_amount_range")] AmountRange<A>,
);

fn deserialize_valid_amount_range<
    R: borsh::io::Read,
    A: AssetClass + PartialOrd + borsh::BorshDeserialize,
>(
    reader: &mut R,
) -> ::core::result::Result<AmountRange<A>, borsh::io::Error> {
    <AmountRange<A> as borsh::BorshDeserialize>::deserialize_reader(reader)?.validate()
}

impl<A: AssetClass + PartialOrd> Deref for ValidAmountRange<A> {
    type Target = AmountRange<A>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<A: AssetClass + PartialOrd> TryFrom<AmountRange<A>> for ValidAmountRange<A> {
    type Error = std::io::Error;

    fn try_from(value: AmountRange<A>) -> Result<Self, Self::Error> {
        Ok(Self(value.validate()?))
    }
}

impl<A: AssetClass + PartialOrd, T: Into<FungibleAssetAmount<A>>> TryFrom<(T, Option<T>)>
    for ValidAmountRange<A>
{
    type Error = std::io::Error;

    fn try_from((minimum, maximum): (T, Option<T>)) -> Result<Self, Self::Error> {
        AmountRange {
            minimum: minimum.into(),
            maximum: maximum.map(Into::into),
        }
        .try_into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct AmountRange<A: AssetClass> {
    pub minimum: FungibleAssetAmount<A>,
    pub maximum: Option<FungibleAssetAmount<A>>,
}

impl<A: AssetClass + PartialOrd> AmountRange<A> {
    pub fn contains(&self, amount: FungibleAssetAmount<A>) -> bool {
        amount >= self.minimum && self.maximum.is_none_or(|max| amount <= max)
    }

    pub fn validate(self) -> std::io::Result<Self> {
        if self.is_valid() {
            Ok(self)
        } else {
            Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "Invalid range specified",
            ))
        }
    }

    pub fn is_valid(&self) -> bool {
        self.maximum
            .is_none_or(|max| !max.is_zero() && max >= self.minimum)
    }

    pub fn new(
        minimum: FungibleAssetAmount<A>,
        maximum: Option<FungibleAssetAmount<A>>,
    ) -> std::io::Result<Self> {
        Self { minimum, maximum }.validate()
    }
}

// look up what is "PIF" and specification of seconds.

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct MarketConfiguration {
    pub time_chunk_configuration: TimeChunkConfiguration,
    pub borrow_asset: FungibleAsset<BorrowAsset>,
    pub collateral_asset: FungibleAsset<CollateralAsset>,
    pub price_oracle_configuration: PriceOracleConfiguration,
    pub borrow_mcr_initial: Decimal,
    pub borrow_mcr: Decimal,
    /// How much of the deposited principal may be lent out (up to 100%)?
    /// This is a matter of protection for supply providers.
    /// Set to 99% for starters.
    pub borrow_asset_maximum_usage_ratio: Decimal,
    /// The origination fee is a one-time amount added to the principal of the
    /// borrow. That is to say, the origination fee is denominated in units of
    /// the borrow asset and is paid by the borrowing account during repayment
    /// (or liquidation).
    pub borrow_origination_fee: Fee<BorrowAsset>,
    pub borrow_interest_rate_strategy: InterestRateStrategy,
    pub borrow_maximum_duration_ms: Option<U64>,
    pub borrow_range: ValidAmountRange<BorrowAsset>,
    pub supply_range: ValidAmountRange<BorrowAsset>,
    pub supply_withdrawal_range: ValidAmountRange<BorrowAsset>,
    pub supply_withdrawal_fee: TimeBasedFee<BorrowAsset>,
    pub yield_weights: YieldWeights,
    pub protocol_account_id: AccountId,
    /// How far below market rate to accept liquidation? This is effectively the liquidator's spread.
    ///
    /// For example, if a 100USDC borrow is (under)collateralized with $110 of
    /// NEAR, a "maximum liquidator spread" of 1% would mean that a liquidator
    /// could liquidate this borrow by sending 108.9USDC, netting the liquidator
    /// $110 * 1% = $1.1 of NEAR.
    pub liquidation_maximum_spread: Decimal,
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
        MustNotEqual(&'static str),
    }

    impl Display for InvalidFieldReason {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::OutOfBounds => write!(f, "out of bounds"),
                Self::MustNotEqual(other) => write!(f, "must not equal `{other}`"),
            }
        }
    }

    pub(super) fn out_of_bounds(field: &'static str) -> ConfigurationValidationError {
        ConfigurationValidationError {
            field,
            reason: InvalidFieldReason::OutOfBounds,
        }
    }

    pub(super) fn must_not_equal(
        field: &'static str,
        other: &'static str,
    ) -> ConfigurationValidationError {
        ConfigurationValidationError {
            field,
            reason: InvalidFieldReason::MustNotEqual(other),
        }
    }
}

impl MarketConfiguration {
    /// # Errors
    ///
    /// If the configuration is invalid.
    pub fn validate(&self) -> Result<(), error::ConfigurationValidationError> {
        if self.borrow_asset == self.collateral_asset.clone().coerce() {
            return Err(error::must_not_equal("borrow_asset", "collateral_asset"));
        }

        if self.borrow_mcr_initial < 1u32 || self.borrow_mcr_initial < self.borrow_mcr {
            return Err(error::out_of_bounds("borrow_mcr_initial"));
        }

        if self.borrow_mcr < 1u32 {
            return Err(error::out_of_bounds("borrow_mcr"));
        }

        if self.borrow_asset_maximum_usage_ratio.is_zero()
            || self.borrow_asset_maximum_usage_ratio > 1u32
        {
            return Err(error::out_of_bounds("borrow_asset_maximum_usage_ratio"));
        }

        if self.borrow_interest_rate_strategy.at(Decimal::ONE) > APY_LIMIT {
            return Err(error::out_of_bounds("borrow_interest_rate_strategy"));
        }

        if self.supply_withdrawal_range.minimum > self.supply_range.minimum {
            return Err(error::out_of_bounds("supply_withdrawal_range.minimum"));
        }

        if self.liquidation_maximum_spread >= 1u32 {
            return Err(error::out_of_bounds("liquidation_maximum_spread"));
        }

        Ok(())
    }

    pub fn borrow_status(
        &self,
        borrow_position: &BorrowPosition,
        price_pair: &PricePair,
        block_timestamp_ms: u64,
    ) -> BorrowStatus {
        if !self.satisfies_minimum_collateral_ratio(borrow_position, price_pair) {
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
        let Some(U64(maximum_duration_ms)) = self.borrow_maximum_duration_ms else {
            return true;
        };
        borrow_position
            .started_at_block_timestamp_ms
            .and_then(|U64(started_at_ms)| block_timestamp_ms.checked_sub(started_at_ms))
            .is_none_or(|duration_ms| duration_ms <= maximum_duration_ms)
    }

    pub fn satisfies_minimum_initial_collateral_ratio(
        &self,
        borrow_position: &BorrowPosition,
        oracle_price_proof: &PricePair,
    ) -> bool {
        satisfies_minimum_collateral_ratio(
            self.borrow_mcr_initial,
            borrow_position,
            oracle_price_proof,
        )
    }

    pub fn satisfies_minimum_collateral_ratio(
        &self,
        borrow_position: &BorrowPosition,
        oracle_price_proof: &PricePair,
    ) -> bool {
        satisfies_minimum_collateral_ratio(self.borrow_mcr, borrow_position, oracle_price_proof)
    }

    pub fn minimum_acceptable_liquidation_amount(
        &self,
        amount: CollateralAssetAmount,
        price_pair: &PricePair,
    ) -> Option<BorrowAssetAmount> {
        ((1u32 - self.liquidation_maximum_spread)
            * Valuation::pessimistic(amount, &price_pair.collateral).ratio(
                Valuation::optimistic(BorrowAssetAmount::new(1), &price_pair.borrow),
            )?)
        .to_u128_ceil()
        .map(BorrowAssetAmount::new)
    }
}

fn satisfies_minimum_collateral_ratio(
    mcr: Decimal,
    borrow_position: &BorrowPosition,
    price_pair: &PricePair,
) -> bool {
    let borrow_liability = borrow_position.get_total_borrow_asset_liability();
    if borrow_liability.is_zero() {
        return true;
    }

    let collateral_valuation = Valuation::pessimistic(
        borrow_position.collateral_asset_deposit,
        &price_pair.collateral,
    );
    let borrow_valuation = Valuation::optimistic(borrow_liability, &price_pair.borrow);

    collateral_valuation
        .ratio(borrow_valuation)
        .is_some_and(|ratio| ratio >= mcr)
}

#[cfg(test)]
mod tests {
    use near_sdk::{
        json_types::U128,
        serde_json::{self, json},
    };
    use rstest::rstest;

    use crate::{borrow::InterestAccumulationProof, dec, oracle::pyth};

    use super::*;

    #[test]
    fn test_satisfies_minimum_collateral_ratio() {
        let mut b = BorrowPosition::new(0);
        b.increase_collateral_asset_deposit(121u128.into());
        b.increase_borrow_asset_principal(InterestAccumulationProof::test(), 100u128.into(), 0);
        assert!(satisfies_minimum_collateral_ratio(
            dec!("1.2"),
            &b,
            &PricePair::new(
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
                18,
            )
            .unwrap()
        ));
    }

    #[rstest]
    #[case(1, 0)]
    #[case(0, 0)]
    #[case(u128::MAX, 0)]
    #[case(u128::MAX, u128::MAX - 1)]
    #[case(500, 10)]
    #[should_panic = "Invalid range specified"]
    fn invalid_amount_range(#[case] min: u128, #[case] max: u128) {
        ValidAmountRange::<BorrowAsset>::try_from((min, Some(max))).unwrap();
    }

    #[rstest]
    #[case(1, 0)]
    #[case(0, 0)]
    #[case(u128::MAX, 0)]
    #[case(u128::MAX, u128::MAX - 1)]
    #[case(500, 10)]
    #[should_panic = "Invalid range specified"]
    fn invalid_amount_range_json(#[case] min: u128, #[case] max: u128) {
        serde_json::from_value::<ValidAmountRange<BorrowAsset>>(json!({
            "minimum": U128(min),
            "maximum": U128(max),
        }))
        .unwrap();
    }

    #[rstest]
    #[case(1, 1)]
    #[case(0, u128::MAX)]
    #[case(1, u128::MAX)]
    #[case(u128::MAX, u128::MAX)]
    #[case(u128::MAX - 1, u128::MAX)]
    #[case(10, 500)]
    fn valid_amount_range(#[case] min: u128, #[case] max: u128) {
        ValidAmountRange::<BorrowAsset>::try_from((min, Some(max))).unwrap();
    }

    #[rstest]
    #[case(1, 1)]
    #[case(0, u128::MAX)]
    #[case(1, u128::MAX)]
    #[case(u128::MAX, u128::MAX)]
    #[case(u128::MAX - 1, u128::MAX)]
    #[case(10, 500)]
    fn valid_amount_range_json(#[case] min: u128, #[case] max: u128) {
        serde_json::from_value::<ValidAmountRange<BorrowAsset>>(json!({
            "minimum": U128(min),
            "maximum": U128(max),
        }))
        .unwrap();
    }
}
