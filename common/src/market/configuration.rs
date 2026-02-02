use std::{io::ErrorKind, ops::Deref};

use near_sdk::{borsh, json_types::U64, near, AccountId};

use crate::{
    asset::{
        AssetClass, BorrowAsset, BorrowAssetAmount, CollateralAsset, CollateralAssetAmount,
        FungibleAsset, FungibleAssetAmount,
    },
    borrow::{BorrowStatus, LiquidationReason},
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    number::Decimal,
    price::{Convert, PricePair},
    snapshot::Snapshot,
    time_chunk::TimeChunkConfiguration,
    YEAR_PER_MS,
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

/// Configuration for a single asset-pair borrow market.
///
/// A market's configuration is immutable after deployment.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct MarketConfiguration {
    /// As time passes, the market creates snapshots of its state. These
    /// snapshots are used to calculate the interest charged to borrowers,
    /// yield earned by suppliers, etc. A **time chunk** represents the period
    /// of time over which a snapshot is taken, and is used as a disambiguating
    /// index for snapshots.
    pub time_chunk_configuration: TimeChunkConfiguration,
    /// The borrow asset supported by this market.
    pub borrow_asset: FungibleAsset<BorrowAsset>,
    /// The collateral asset supported by this market.
    pub collateral_asset: FungibleAsset<CollateralAsset>,
    /// The market communicates with a price oracle to determine asset
    /// valuations.
    pub price_oracle_configuration: PriceOracleConfiguration,
    /// A borrow position must satisfy this minimum collateralization ratio
    /// after any modifications (e.g. withdrawing collateral).
    ///
    /// Must be greater than or equal to `borrow_mcr_liquidation`.
    pub borrow_mcr_maintenance: Decimal,
    /// A borrow position is eligible for liquidation if it does not satisfy
    /// this minimu collateralization ratio.
    ///
    /// Must be less than or equal to `borrow_mcr_maintenance`.
    pub borrow_mcr_liquidation: Decimal,
    /// Maintain a reserve of some% of the deposited supply; how much of the
    /// deposited principal may be lent out (up to 100%)?
    /// This is a matter of protection for supply providers.
    pub borrow_asset_maximum_usage_ratio: Decimal,
    /// The origination fee is a one-time amount added to the principal of the
    /// borrow. That is to say, the origination fee is denominated in units of
    /// the borrow asset and is paid by the borrowing account during repayment
    /// (or liquidation).
    pub borrow_origination_fee: Fee<BorrowAsset>,
    /// Interest rate is decided by a function of utilization ratio [0.0, 1.0].
    pub borrow_interest_rate_strategy: InterestRateStrategy,
    /// If a maximum borrow duration is configured, a borrow position is
    /// instantly eligible for liquidation (regardless of collateralization
    /// ratio) after this period has expired.
    pub borrow_maximum_duration_ms: Option<U64>,
    /// A borrow position's principal must be within this range after modification.
    pub borrow_range: ValidAmountRange<BorrowAsset>,
    /// A supply position's deposit must be within this range after modification.
    pub supply_range: ValidAmountRange<BorrowAsset>,
    /// A supply position may only request to withdraw amounts within this range.
    pub supply_withdrawal_range: ValidAmountRange<BorrowAsset>,
    /// A time-bound fee for supply, to discourage extremely short-lived
    /// supply positions.
    pub supply_withdrawal_fee: TimeBasedFee<BorrowAsset>,
    /// Determines how yield is distributed between suppliers (dynamically
    /// allocated based on deposit) and statically-configured accounts (e.g. a
    /// protocol insurance account).
    pub yield_weights: YieldWeights,
    /// For collecting supply withdrawal fees.
    ///
    /// Supply withdrawal fees cannot be distributed to other suppliers
    /// because there may not be any suppliers to earn those fees after the
    /// last one withdraws.
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

        if self.borrow_mcr_maintenance <= 1u32
            || self.borrow_mcr_maintenance < self.borrow_mcr_liquidation
        {
            return Err(error::out_of_bounds("borrow_mcr_maintenance"));
        }

        if self.borrow_mcr_liquidation <= 1u32 {
            return Err(error::out_of_bounds("borrow_mcr_liquidation"));
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

        if let Fee::Flat(amount) = self.supply_withdrawal_fee.fee {
            if amount > self.supply_withdrawal_range.minimum {
                return Err(error::out_of_bounds("supply_withdrawal_fee.fee"));
            }
        }

        if self.liquidation_maximum_spread >= 1u32
            || self.borrow_mcr_liquidation * (Decimal::ONE - self.liquidation_maximum_spread)
                <= Decimal::ONE
        {
            return Err(error::out_of_bounds("liquidation_maximum_spread"));
        }

        Ok(())
    }

    pub fn borrow_status(
        &self,
        collateralization_ratio: Option<Decimal>,
        started_at_block_timestamp_ms: Option<impl Into<u64>>,
        block_timestamp_ms: u64,
    ) -> BorrowStatus {
        if started_at_block_timestamp_ms.is_some_and(|started_at| {
            !self.is_within_maximum_borrow_duration(started_at.into(), block_timestamp_ms)
        }) {
            return BorrowStatus::Liquidation(LiquidationReason::Expiration);
        }

        if let Some(cr) = collateralization_ratio {
            if cr < self.borrow_mcr_liquidation {
                return BorrowStatus::Liquidation(LiquidationReason::Undercollateralization);
            }

            if cr < self.borrow_mcr_maintenance {
                return BorrowStatus::MaintenanceRequired;
            }
        }

        BorrowStatus::Healthy
    }

    fn is_within_maximum_borrow_duration(
        &self,
        started_at_block_timestamp_ms: u64,
        block_timestamp_ms: u64,
    ) -> bool {
        let Some(U64(maximum_duration_ms)) = self.borrow_maximum_duration_ms else {
            return true;
        };
        block_timestamp_ms
            .checked_sub(started_at_block_timestamp_ms)
            .is_none_or(|duration_ms| duration_ms <= maximum_duration_ms)
    }

    pub fn minimum_acceptable_liquidation_amount(
        &self,
        amount: CollateralAssetAmount,
        price_pair: &PricePair,
    ) -> Option<BorrowAssetAmount> {
        ((1u32 - self.liquidation_maximum_spread) * price_pair.convert(amount))
            .to_u128_ceil()
            .map(BorrowAssetAmount::new)
    }

    pub fn single_snapshot_maximum_interest(&self) -> Decimal {
        self.borrow_interest_rate_strategy.at(Decimal::ONE)
            * self.time_chunk_configuration.duration_ms()
            * YEAR_PER_MS
    }

    pub fn supply_yield_rate_from_interest(&self, snapshot: &Snapshot) -> Decimal {
        if snapshot.borrow_asset_deposited_active.is_zero() {
            return Decimal::ZERO;
        }
        let deposited: Decimal = snapshot.borrow_asset_deposited_active.into();
        let borrowed: Decimal = snapshot.borrow_asset_borrowed.into();
        let supply_weight: Decimal = self.yield_weights.supply.get().into();
        let total_weight: Decimal = self.yield_weights.total_weight().get().into();

        snapshot.interest_rate * borrowed * supply_weight / deposited / total_weight
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::{
        json_types::U128,
        serde_json::{self, json},
    };
    use rstest::rstest;

    use crate::{dec, oracle::pyth::PriceIdentifier};

    use super::*;

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

    #[test]
    fn single_snapshot_maximum_interest() {
        let c = MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(600_000),
            borrow_asset: FungibleAsset::nep141("borrow.near".parse().unwrap()),
            collateral_asset: FungibleAsset::nep141("collateral.near".parse().unwrap()),
            price_oracle_configuration: PriceOracleConfiguration {
                account_id: "pyth-oracle.near".parse().unwrap(),
                collateral_asset_price_id: PriceIdentifier([0xcc; 32]),
                collateral_asset_decimals: 24,
                borrow_asset_price_id: PriceIdentifier([0xbb; 32]),
                borrow_asset_decimals: 24,
                price_maximum_age_s: 60,
            },
            borrow_mcr_maintenance: dec!("1.25"),
            borrow_mcr_liquidation: dec!("1.2"),
            borrow_asset_maximum_usage_ratio: dec!("0.99"),
            borrow_origination_fee: Fee::zero(),
            borrow_interest_rate_strategy: InterestRateStrategy::linear(dec!("0.1"), dec!("0.1"))
                .unwrap(),
            borrow_maximum_duration_ms: None,
            borrow_range: (1, None).try_into().unwrap(),
            supply_range: (1, None).try_into().unwrap(),
            supply_withdrawal_range: (1, None).try_into().unwrap(),
            supply_withdrawal_fee: TimeBasedFee::zero(),
            yield_weights: YieldWeights::new_with_supply_weight(9)
                .with_static("revenue.tmplr.near".parse().unwrap(), 1),
            protocol_account_id: "revenue.tmplr.near".parse().unwrap(),
            liquidation_maximum_spread: dec!("0.05"),
        };

        let actual = c.single_snapshot_maximum_interest();

        let apr = dec!("0.1");
        let single_snapshot_duration_ms = dec!("600000");
        let expected =
            apr * single_snapshot_duration_ms / (1000u32 * 60 * 60 * 24) / dec!("365.2425");

        assert!(actual.abs_diff(expected) < Decimal::ONE.mul_pow10(-34).unwrap());
    }
}
