use near_sdk::{env, json_types::U64, near};

use crate::asset::CollateralAssetAmount;
use crate::{
    asset::BorrowAssetAmount, interest_rate_strategy::InterestRateStrategy, number::Decimal,
    time_chunk::TimeChunk,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct Snapshot {
    pub time_chunk: TimeChunk,
    pub end_timestamp_ms: U64,
    borrow_asset_deposited_active: BorrowAssetAmount,
    pub borrow_asset_deposited_incoming: BorrowAssetAmount,
    borrow_asset_borrowed: BorrowAssetAmount,
    pub collateral_asset_deposited: CollateralAssetAmount,
    pub yield_distribution: BorrowAssetAmount,
    interest_rate: Decimal,
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

    pub fn add_yield(&mut self, additional_yield: BorrowAssetAmount) {
        self.yield_distribution
            .join(additional_yield)
            .unwrap_or_else(|| env::panic_str("Snapshot yield distribution amount overflow"));
    }

    pub fn update_active(
        &mut self,
        borrow_deposited_active: BorrowAssetAmount,
        borrowed: BorrowAssetAmount,
        collateral_deposited: CollateralAssetAmount,
        interest_rate_strategy: &InterestRateStrategy,
    ) {
        self.end_timestamp_ms = env::block_timestamp_ms().into();
        self.borrow_asset_deposited_active = borrow_deposited_active;
        self.borrow_asset_borrowed = borrowed;
        self.collateral_asset_deposited = collateral_deposited;
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

    pub fn interest_rate(&self) -> Decimal {
        self.interest_rate
    }

    pub fn deposited_active(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposited_active
    }

    pub fn borrowed(&self) -> BorrowAssetAmount {
        self.borrow_asset_borrowed
    }
}
