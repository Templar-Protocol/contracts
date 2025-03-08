use near_sdk::near;

use crate::asset::{AssetClass, BorrowAsset, BorrowAssetAmount, FungibleAssetAmount};

#[derive(Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct SupplyPosition {
    borrow_asset_deposit: BorrowAssetAmount,
    pub borrow_asset_yield: YieldRecord<BorrowAsset>,
}

impl SupplyPosition {
    pub fn new(current_snapshot_index: u32) -> Self {
        Self {
            borrow_asset_deposit: 0.into(),
            // We start at next log index so that the supply starts
            // accumulating yield from the _next_ log (since they were not
            // necessarily supplying for all of the current log).
            borrow_asset_yield: YieldRecord::new(current_snapshot_index + 1),
        }
    }

    pub fn get_borrow_asset_deposit(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposit
    }

    pub fn exists(&self) -> bool {
        !self.borrow_asset_deposit.is_zero() || !self.borrow_asset_yield.amount.is_zero()
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct YieldRecord<T: AssetClass> {
    pub amount: FungibleAssetAmount<T>,
    pub until_snapshot_index: u32,
}

impl<T: AssetClass> YieldRecord<T> {
    pub fn new(until_snapshot_index: u32) -> Self {
        Self {
            amount: 0.into(),
            until_snapshot_index,
        }
    }

    pub fn withdraw(&mut self, amount: u128) -> Option<FungibleAssetAmount<T>> {
        self.amount.split(amount)
    }

    pub fn accumulate_yield(
        &mut self,
        additional_yield: FungibleAssetAmount<T>,
        until_snapshot_index: u32,
    ) {
        debug_assert!(until_snapshot_index > self.until_snapshot_index);
        self.amount.join(additional_yield);
        self.until_snapshot_index = until_snapshot_index;
    }
}
