use near_sdk::{json_types::U64, near};
use templar_primitives::number::Decimal;

use crate::asset::{AssetClass, FungibleAssetAmount};

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Fee<T: AssetClass> {
    Flat(FungibleAssetAmount<T>),
    Proportional(Decimal),
}

impl<T: AssetClass> Fee<T> {
    pub fn zero() -> Self {
        Self::Flat(FungibleAssetAmount::zero())
    }

    pub fn of(&self, amount: FungibleAssetAmount<T>) -> Option<FungibleAssetAmount<T>> {
        match self {
            Fee::Flat(f) => Some(*f),
            Fee::Proportional(factor) => (factor * u128::from(amount))
                .to_u128_ceil()
                .map(FungibleAssetAmount::new),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct TimeBasedFee<T: AssetClass> {
    pub fee: Fee<T>,
    pub duration: U64,
    pub behavior: TimeBasedFeeFunction,
}

impl<T: AssetClass> TimeBasedFee<T> {
    pub fn zero() -> Self {
        Self {
            fee: Fee::Flat(0.into()),
            duration: 0.into(),
            behavior: TimeBasedFeeFunction::Fixed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub enum TimeBasedFeeFunction {
    Fixed,
    Linear,
}

impl<T: AssetClass> TimeBasedFee<T> {
    pub fn of(
        &self,
        amount: FungibleAssetAmount<T>,
        duration: u64,
    ) -> Option<FungibleAssetAmount<T>> {
        let base_fee = self.fee.of(amount)?;

        if self.duration.0 == 0 {
            return Some(0.into());
        }

        match self.behavior {
            TimeBasedFeeFunction::Fixed => {
                if duration >= self.duration.0 {
                    Some(0.into())
                } else {
                    Some(base_fee)
                }
            }
            TimeBasedFeeFunction::Linear => {
                (Decimal::from(self.duration.0.saturating_sub(duration)) * u128::from(base_fee)
                    / Decimal::from(self.duration.0))
                .to_u128_ceil()
                .map(FungibleAssetAmount::new)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use templar_primitives::dec;

    use crate::asset::BorrowAsset;

    use super::{TimeBasedFeeFunction::*, *};

    type Amount = FungibleAssetAmount<BorrowAsset>;

    fn time_based(
        fee: Fee<BorrowAsset>,
        duration_ms: u64,
        behavior: TimeBasedFeeFunction,
    ) -> TimeBasedFee<BorrowAsset> {
        TimeBasedFee {
            fee,
            duration: duration_ms.into(),
            behavior,
        }
    }

    #[test]
    fn flat_fee_is_constant() {
        assert_eq!(
            Fee::<BorrowAsset>::Flat(100.into()).of(1_000.into()),
            Some(Amount::new(100)),
        );
    }

    #[test]
    fn proportional_fee_rounds_up() {
        // 0.1% of 1005 = 1.005, rounded up to 2.
        assert_eq!(
            Fee::<BorrowAsset>::Proportional(dec!("0.001")).of(1_005.into()),
            Some(Amount::new(2)),
        );
    }

    #[test]
    fn fixed_before_expiry_charges_full_fee() {
        let f = time_based(Fee::Flat(100.into()), 1000 * 60 * 60 * 24 * 30, Fixed);
        assert_eq!(f.of(1_000.into(), 10_000), Some(Amount::new(100)));
    }

    #[test]
    fn fixed_at_or_after_expiry_charges_nothing() {
        let f = time_based(Fee::Flat(100.into()), 1000, Fixed);
        assert_eq!(f.of(1_000.into(), 1000), Some(Amount::new(0)));
        assert_eq!(f.of(1_000.into(), 2000), Some(Amount::new(0)));
    }

    #[test]
    fn zero_configured_duration_is_always_free() {
        let f = time_based(Fee::Flat(100.into()), 0, Fixed);
        assert_eq!(f.of(1_000.into(), 0), Some(Amount::new(0)));
    }

    #[test]
    fn linear_interpolates_toward_expiry_and_rounds_up() {
        let f = time_based(Fee::Flat(100.into()), 1000, Linear);
        // At the start the full fee applies; it decays linearly to zero at expiry.
        assert_eq!(f.of(1_000.into(), 0), Some(Amount::new(100)));
        // remaining 750/1000 of 100 = 75.
        assert_eq!(f.of(1_000.into(), 250), Some(Amount::new(75)));
        // remaining 749/1000 of 100 = 74.9, rounded up to 75.
        assert_eq!(f.of(1_000.into(), 251), Some(Amount::new(75)));
    }

    #[test]
    fn linear_past_expiry_saturates_to_zero() {
        let f = time_based(Fee::Flat(100.into()), 1000, Linear);
        assert_eq!(f.of(1_000.into(), 5000), Some(Amount::new(0)));
    }
}
