use near_sdk::{env, json_types::U64, near};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    number::Decimal,
    time_chunk::TimeChunk,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct Snapshot {
    pub time_chunk: TimeChunk,
    pub end_timestamp_ms: U64,
    pub borrow_asset_deposited_active_real: BorrowAssetAmount,
    pub borrow_asset_deposited_active_virtual: BorrowAssetAmount,
    pub borrow_asset_borrowed: BorrowAssetAmount,
    pub collateral_asset_deposited: CollateralAssetAmount,
    pub yield_distribution: BorrowAssetAmount,
    pub interest_rate: Decimal,
}

impl Snapshot {
    pub fn new(time_chunk: TimeChunk) -> Self {
        Self {
            time_chunk,
            end_timestamp_ms: env::block_timestamp_ms().into(),
            borrow_asset_deposited_active_real: 0.into(),
            borrow_asset_deposited_active_virtual: 0.into(),
            borrow_asset_borrowed: 0.into(),
            collateral_asset_deposited: 0.into(),
            yield_distribution: BorrowAssetAmount::zero(),
            interest_rate: Decimal::ZERO,
        }
    }

    pub fn active_supply(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposited_active_real + self.borrow_asset_deposited_active_virtual
    }
}
