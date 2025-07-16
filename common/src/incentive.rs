use std::{collections::BTreeSet, ops::Deref};

use near_sdk::{json_types::U64, near};

use crate::{
    asset::{BorrowAsset, BorrowAssetAmount, FungibleAsset, IncentiveAsset, IncentiveAssetAmount},
    number::Decimal,
    oracle::pyth::PriceIdentifier,
    price::Price,
};

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct Incentive {
    // pub asset: FungibleAsset<IncentiveAsset>,
    pub oracle_asset_id: PriceIdentifier,
    entries: Vec<IncentiveEntry>,
}

impl Incentive {
    pub fn new(oracle_asset_id: PriceIdentifier) -> Self {
        Self {
            oracle_asset_id,
            entries: Vec::new(),
        }
    }

    pub fn add_incentive(&mut self, entry: IncentiveEntry) {
        self.entries.push(entry);
    }

    pub fn distribute_for_snapshot(
        &mut self,
        prev_snapshot_end_timestamp_ms: u64,
        supply_deposited: BorrowAssetAmount,
        yield_rate: Decimal,
        borrow_asset_price: Price<BorrowAsset>,
        incentive_asset_price: Price<IncentiveAsset>,
    ) -> IncentiveAssetAmount {
        let mut entries = self
            .entries
            .iter()
            .filter(|e| e.start_timestamp_ms.0 <= prev_snapshot_end_timestamp_ms)
            .collect::<Vec<_>>();

        while !entries.is_empty() {}
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct IncentiveEntryInner {
    start_timestamp_ms: U64,
    rate: Decimal,
    amount: IncentiveAssetAmount,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum IncentiveEntry {
    Boost(IncentiveEntryInner),
    Target(IncentiveEntryInner),
}

impl Deref for IncentiveEntry {
    type Target = IncentiveEntryInner;

    fn deref(&self) -> &<Self as Deref>::Target {
        match self {
            Self::Boost(ref inner) | Self::Target(ref inner) => inner,
        }
    }
}

impl IncentiveEntry {
    pub fn effective_rate(&self, yield_rate: Decimal) -> Decimal {
        todo!()
    }
}
