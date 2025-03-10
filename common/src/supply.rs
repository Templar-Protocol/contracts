use near_sdk::near;

use crate::{
    accumulator::Accumulator,
    asset::{BorrowAsset, BorrowAssetAmount},
};

#[derive(Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct SupplyPosition {
    borrow_asset_deposit: BorrowAssetAmount,
    pub borrow_asset_yield: Accumulator<BorrowAsset>,
    #[borsh(skip)]
    #[serde(default, skip_serializing_if = "BorrowAssetAmount::is_zero")]
    pub pending_yield_estimate: BorrowAssetAmount,
}

impl SupplyPosition {
    pub fn new(current_snapshot_index: u32) -> Self {
        Self {
            borrow_asset_deposit: 0.into(),
            // We start at next log index so that the supply starts
            // accumulating yield from the _next_ log (since they were not
            // necessarily supplying for all of the current log).
            borrow_asset_yield: Accumulator::new(current_snapshot_index + 1),
            pending_yield_estimate: BorrowAssetAmount::zero(),
        }
    }

    pub fn get_borrow_asset_deposit(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposit
    }

    pub fn exists(&self) -> bool {
        !self.borrow_asset_deposit.is_zero() || !self.borrow_asset_yield.total.is_zero()
    }

    /// MUST always be paired with a yield recalculation!
    pub(crate) fn increase_borrow_asset_deposit(
        &mut self,
        amount: BorrowAssetAmount,
    ) -> Option<()> {
        self.borrow_asset_deposit.join(amount)
    }

    /// MUST always be paired with a yield recalculation!
    pub(crate) fn decrease_borrow_asset_deposit(
        &mut self,
        amount: BorrowAssetAmount,
    ) -> Option<BorrowAssetAmount> {
        self.borrow_asset_deposit.split(amount)
    }
}
