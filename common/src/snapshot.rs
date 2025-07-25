use near_sdk::{env, json_types::U64, near};

use crate::{
    asset::BorrowAssetAmount, interest_rate_strategy::InterestRateStrategy, number::Decimal,
    time_chunk::TimeChunk,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct Snapshot {
    pub time_chunk: TimeChunk,
    pub end_timestamp_ms: U64,
    deposited_active: BorrowAssetAmount,
    pub deposited_incoming: BorrowAssetAmount,
    borrowed: BorrowAssetAmount,
    pub yield_distribution: BorrowAssetAmount,
    interest_rate: Decimal,
}

impl Snapshot {
    pub fn new(time_chunk: TimeChunk) -> Self {
        Self {
            time_chunk,
            end_timestamp_ms: near_sdk::env::block_timestamp_ms().into(),
            deposited_active: 0.into(),
            deposited_incoming: 0.into(),
            borrowed: 0.into(),
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
        deposited_active: BorrowAssetAmount,
        borrowed: BorrowAssetAmount,
        interest_rate_strategy: &InterestRateStrategy,
    ) {
        self.end_timestamp_ms = env::block_timestamp_ms().into();
        self.deposited_active = deposited_active;
        self.borrowed = borrowed;
        self.interest_rate = interest_rate_strategy.at(self.usage_ratio());
    }

    pub fn usage_ratio(&self) -> Decimal {
        if self.deposited_active.is_zero() || self.borrowed.is_zero() {
            Decimal::ZERO
        } else if self.borrowed >= self.deposited_active {
            Decimal::ONE
        } else {
            Decimal::from(self.borrowed) / Decimal::from(self.deposited_active)
        }
    }

    pub fn interest_rate(&self) -> Decimal {
        self.interest_rate
    }

    pub fn deposited_active(&self) -> BorrowAssetAmount {
        self.deposited_active
    }

    pub fn borrowed(&self) -> BorrowAssetAmount {
        self.borrowed
    }
}
