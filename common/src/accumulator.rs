use near_sdk::{json_types::U128, near, require};

use crate::asset::{AssetClass, FungibleAssetAmount};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct Accumulator<T: AssetClass> {
    total: FungibleAssetAmount<T>,
    fraction_as_u128_dividend: U128,
    next_snapshot_index: u32,
}

impl<T: AssetClass> Accumulator<T> {
    pub fn new(next_snapshot_index: u32) -> Self {
        Self {
            total: 0.into(),
            fraction_as_u128_dividend: U128(0),
            next_snapshot_index,
        }
    }

    pub fn get_next_snapshot_index(&self) -> u32 {
        self.next_snapshot_index
    }

    pub fn get_total(&self) -> FungibleAssetAmount<T> {
        self.total
    }

    pub fn clear(&mut self, next_snapshot_index: u32) {
        self.total = 0.into();
        self.next_snapshot_index = next_snapshot_index;
    }

    pub fn remove(&mut self, amount: FungibleAssetAmount<T>) {
        self.total -= amount;
    }

    pub fn add_once(&mut self, amount: FungibleAssetAmount<T>) {
        self.total += amount;
    }

    pub fn accumulate(
        &mut self,
        AccumulationRecord {
            mut amount,
            fraction_as_u128_dividend: fraction,
            next_snapshot_index,
        }: AccumulationRecord<T>,
    ) {
        require!(
            next_snapshot_index >= self.next_snapshot_index,
            "Invariant violation: Asset accumulations cannot occur retroactively.",
        );
        let (fraction, carry) = self.fraction_as_u128_dividend.0.overflowing_add(fraction);
        if carry {
            amount += 1;
        }
        self.add_once(amount);
        self.fraction_as_u128_dividend.0 = fraction;
        self.next_snapshot_index = next_snapshot_index;
    }
}

#[must_use]
#[derive(Debug, Clone)]
pub struct AccumulationRecord<T: AssetClass> {
    pub(crate) amount: FungibleAssetAmount<T>,
    pub(crate) fraction_as_u128_dividend: u128,
    pub(crate) next_snapshot_index: u32,
}

impl<T: AssetClass> AccumulationRecord<T> {
    pub fn get_amount(&self) -> FungibleAssetAmount<T> {
        self.amount
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction() {
        let mut a = Accumulator::<crate::asset::BorrowAsset>::new(1);

        a.accumulate(AccumulationRecord {
            amount: 100.into(),
            fraction_as_u128_dividend: 1 << 127,
            next_snapshot_index: 2,
        });

        assert_eq!(a.get_total(), 100.into());

        a.accumulate(AccumulationRecord {
            amount: 100.into(),
            fraction_as_u128_dividend: 1 << 127,
            next_snapshot_index: 3,
        });

        assert_eq!(a.get_total(), 201.into());
    }
}
