use near_sdk::{env, json_types::U64, near};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    asset_op,
    interest_rate_strategy::InterestRateStrategy,
    number::Decimal,
    time_chunk::TimeChunk,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct Snapshot {
    pub time_chunk: TimeChunk,
    pub end_timestamp_ms: U64,
    pub borrow_asset_deposited_active: BorrowAssetAmount,
    pub borrow_asset_deposited_incoming: BorrowAssetAmount,
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
            borrow_asset_deposited_active: 0.into(),
            borrow_asset_deposited_incoming: 0.into(),
            borrow_asset_borrowed: 0.into(),
            collateral_asset_deposited: 0.into(),
            yield_distribution: BorrowAssetAmount::zero(),
            interest_rate: Decimal::ZERO,
        }
    }

    pub fn update(
        &mut self,
        active: BorrowAssetAmount,
        incoming: BorrowAssetAmount,
        interest_rate_strategy: &InterestRateStrategy,
    ) {
        self.borrow_asset_deposited_incoming = incoming;
        self.borrow_asset_deposited_active = active;
        asset_op!(
            self.borrow_asset_deposited_active += incoming;
        );
        self.interest_rate = interest_rate_strategy.at(self.usage_ratio());
    }

    pub fn usage_ratio(&self) -> Decimal {
        if self.borrow_asset_deposited_active.is_zero() || self.borrow_asset_borrowed.is_zero() {
            Decimal::ZERO
        } else if self.borrow_asset_borrowed >= self.borrow_asset_deposited_active {
            Decimal::ONE
        } else {
            Decimal::from(self.borrow_asset_borrowed)
                / Decimal::from(self.borrow_asset_deposited_active)
        }
    }
}
