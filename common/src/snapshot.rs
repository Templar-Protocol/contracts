use near_sdk::{env, json_types::U64, near};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    interest_rate_strategy::InterestRateStrategy,
    number::Decimal,
    time_chunk::TimeChunk,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct Snapshot {
    time_chunk: TimeChunk,
    end_timestamp_ms: U64,
    borrow_asset_deposited_active: BorrowAssetAmount,
    borrow_asset_deposited_incoming: BorrowAssetAmount,
    borrow_asset_borrowed: BorrowAssetAmount,
    collateral_asset_deposited: CollateralAssetAmount,
    yield_distribution: BorrowAssetAmount,
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
        borrow_asset_deposited_active: BorrowAssetAmount,
        borrow_asset_borrowed: BorrowAssetAmount,
        collateral_asset_deposited: CollateralAssetAmount,
        interest_rate_strategy: &InterestRateStrategy,
    ) {
        self.end_timestamp_ms = env::block_timestamp_ms().into();
        self.borrow_asset_deposited_active = borrow_asset_deposited_active;
        self.borrow_asset_borrowed = borrow_asset_borrowed;
        self.collateral_asset_deposited = collateral_asset_deposited;
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
    
    pub fn set_time_chunk(&mut self, time_chunk: TimeChunk) {
        self.time_chunk = time_chunk;
    }
    
    pub fn set_borrow_asset_deposited_incoming(
        &mut self,
        amount: BorrowAssetAmount,
    ) {
        self.borrow_asset_deposited_incoming = amount;
    }
    
    pub fn set_yield_distribution(&mut self, amount: BorrowAssetAmount) {
        self.yield_distribution = amount;
    }

    pub fn time_chunk(&self) -> &TimeChunk {
        &self.time_chunk
    }
    
    pub fn end_timestamp_ms(&self) -> U64 {
        self.end_timestamp_ms
    }

    pub fn borrow_asset_deposited_incoming(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposited_incoming
    }

    pub fn collateral_asset_deposited(&self) -> CollateralAssetAmount {
        self.collateral_asset_deposited
    }

    pub fn yield_distribution(&self) -> BorrowAssetAmount {
        self.yield_distribution
    }

    pub fn interest_rate(&self) -> Decimal {
        self.interest_rate
    }

    pub fn borrow_asset_deposited_active(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposited_active
    }

    pub fn borrow_asset_borrowed(&self) -> BorrowAssetAmount {
        self.borrow_asset_borrowed
    }
}
