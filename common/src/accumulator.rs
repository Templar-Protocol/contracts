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
}

impl<T: AssetClass> Accumulator<T> {
    pub fn new(next_snapshot_index: u32) -> Self {
        Self {
            total: 0.into(),
            next_snapshot_index,
            pending_estimate: 0.into(),
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

    pub fn remove(&mut self, amount: FungibleAssetAmount<T>) -> Option<FungibleAssetAmount<T>> {
        self.total.split(amount)
    }

    pub fn add_once(&mut self, amount: FungibleAssetAmount<T>) -> Option<()> {
        self.total.join(amount)
    }

    pub fn accumulate(
        &mut self,
        AccumulationRecord {
            amount,
            next_snapshot_index,
        }: AccumulationRecord<T>,
    ) -> Option<()> {
        require!(
            next_snapshot_index >= self.next_snapshot_index,
            "Invariant violation: Asset accumulations cannot occur retroactively.",
        );
        self.total.join(amount)?;
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
