use near_sdk::{json_types::U64, near};

use crate::{asset::BorrowAssetAmount, number::Decimal, time_chunk::TimeChunk};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct Snapshot {
    pub time_chunk: TimeChunk,
    pub timestamp_ms: U64,
    pub deposited: BorrowAssetAmount,
    pub borrowed: BorrowAssetAmount,
    pub yield_distribution: BorrowAssetAmount,
}

impl Snapshot {
    pub fn usage_ratio(&self) -> Decimal {
        if self.deposited.is_zero() {
            Decimal::ZERO
        } else {
            self.borrowed.to_decimal() / self.deposited.to_decimal()
        }
    }
}
