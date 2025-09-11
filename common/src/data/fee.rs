use near_sdk::{json_types::U64, near};

use crate::{
    asset::{AssetClass, FungibleAssetAmount},
    number::Decimal,
};

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
                (Decimal::from(self.duration.0.saturating_sub(duration))
                    / Decimal::from(self.duration.0)
                    * u128::from(base_fee))
                .to_u128_ceil()
                .map(FungibleAssetAmount::new)
            }
        }
    }
}
