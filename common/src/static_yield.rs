use near_sdk::near;

use crate::{
    accumulator::Accumulator,
    asset::{BorrowAsset, CollateralAsset},
};

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct StaticYieldRecord {
    pub collateral_asset: Accumulator<CollateralAsset>,
    pub borrow_asset: Accumulator<BorrowAsset>,
}

impl Default for StaticYieldRecord {
    fn default() -> Self {
        Self::new(1)
    }
}

impl StaticYieldRecord {
    pub fn new(next_snapshot_index: u32) -> Self {
        Self {
            collateral_asset: Accumulator::new(next_snapshot_index),
            borrow_asset: Accumulator::new(next_snapshot_index),
        }
    }
}
