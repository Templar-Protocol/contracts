use near_sdk::{near, require};

use crate::asset::{AssetClass, FungibleAssetAmount};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct Accumulator<T: AssetClass> {
    total: FungibleAssetAmount<T>,
    next_snapshot_index: u32,
    #[borsh(skip)]
    #[serde(default, skip_serializing_if = "FungibleAssetAmount::is_zero")]
    pub pending_estimate: FungibleAssetAmount<T>,
    amortized: FungibleAssetAmount<T>,
}

impl<T: AssetClass> Accumulator<T> {
    pub fn new(next_snapshot_index: u32) -> Self {
        Self {
            total: 0.into(),
            next_snapshot_index,
            pending_estimate: 0.into(),
            amortized: 0.into(),
        }
    }

    pub fn get_next_snapshot_index(&self) -> u32 {
        self.next_snapshot_index
    }

    pub fn get_total(&self) -> FungibleAssetAmount<T> {
        self.total
        // let mut total = self.total;
        // match total.split(self.amortized) {
        //     Some(_) => total,
        //     None => FungibleAssetAmount::zero(),
        // }
    }

    pub fn clear(&mut self, next_snapshot_index: u32) {
        self.total = 0.into();
        self.amortized = 0.into();
        self.next_snapshot_index = next_snapshot_index;
    }

    pub fn remove(&mut self, amount: FungibleAssetAmount<T>) -> Option<FungibleAssetAmount<T>> {
        self.total.split(amount)
    }

    pub fn add_once(&mut self, mut amount: FungibleAssetAmount<T>) -> Option<()>
    where
        T: PartialOrd,
    {
        #[allow(clippy::unwrap_used, reason = "If statement guarantees safety")]
        if amount > self.amortized {
            amount.split(self.amortized).unwrap();
            self.amortized = 0.into();
            self.total.join(amount)?;
        } else {
            self.amortized.split(amount).unwrap();
        }
        Some(())
    }

    pub fn amortize(&mut self, amount: FungibleAssetAmount<T>) -> Option<()> {
        self.total.join(amount)?;
        if self.amortized.join(amount).is_none() {
            #[allow(clippy::unwrap_used, reason = "Simply reverses above operation")]
            self.total.split(amount).unwrap();
            None
        } else {
            Some(())
        }
    }

    pub fn accumulate(
        &mut self,
        AccumulationRecord {
            amount,
            next_snapshot_index,
        }: AccumulationRecord<T>,
    ) -> Option<()>
    where
        T: PartialOrd,
    {
        require!(
            next_snapshot_index >= self.next_snapshot_index,
            "Invariant violation: Asset accumulations cannot occur retroactively.",
        );
        self.add_once(amount)?;
        self.next_snapshot_index = next_snapshot_index;
        Some(())
    }
}

#[must_use]
pub struct AccumulationRecord<T: AssetClass> {
    pub(crate) amount: FungibleAssetAmount<T>,
    pub(crate) next_snapshot_index: u32,
}

impl<T: AssetClass> AccumulationRecord<T> {
    pub fn empty(next_snapshot_index: u32) -> Self {
        Self {
            amount: FungibleAssetAmount::zero(),
            next_snapshot_index,
        }
    }

    pub fn get_amount(&self) -> FungibleAssetAmount<T> {
        self.amount
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amortization() {
        let mut a = Accumulator::<crate::asset::BorrowAsset>::new(1);

        a.accumulate(AccumulationRecord {
            amount: 100.into(),
            next_snapshot_index: 2,
        });

        assert_eq!(a.get_total(), 100.into());

        a.amortize(25.into());

        assert_eq!(a.get_total(), 125.into());

        a.accumulate(AccumulationRecord {
            amount: 100.into(),
            next_snapshot_index: 3,
        });

        assert_eq!(a.get_total(), 200.into());
    }
}
