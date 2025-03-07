use near_sdk::near;

use crate::{
    asset::{AssetClass, BorrowAsset, BorrowAssetAmount, FungibleAssetAmount},
    chain_time::ChainTime,
};

#[derive(Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct SupplyPosition {
    borrow_asset_deposit: BorrowAssetAmount,
    pub borrow_asset_yield: YieldRecord<BorrowAsset>,
}

impl SupplyPosition {
    pub fn new(chain_time: ChainTime) -> Self {
        Self {
            borrow_asset_deposit: 0.into(),
            borrow_asset_yield: YieldRecord::new(chain_time),
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
    pub last_updated: ChainTime,
}

impl<T: AssetClass> YieldRecord<T> {
    pub fn new(last_updated: ChainTime) -> Self {
        Self {
            amount: 0.into(),
            last_updated,
        }
    }

    pub fn withdraw(&mut self, amount: u128) -> Option<FungibleAssetAmount<T>> {
        self.amount.split(amount)
    }

    pub fn accumulate_yield(
        &mut self,
        additional_yield: FungibleAssetAmount<T>,
        chain_time: ChainTime,
    ) {
        debug_assert!(chain_time > self.last_updated);
        self.amount.join(additional_yield);
        self.last_updated = chain_time;
    }
}
