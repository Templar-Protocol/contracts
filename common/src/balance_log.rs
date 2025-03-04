use std::cmp::Ordering;

use borsh::BorshDeserialize;
use near_sdk::{collections::Vector, json_types::U64, near};

use crate::asset::{AssetClass, FungibleAssetAmount};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct BalanceLog<A: AssetClass> {
    pub epoch_height: U64,
    pub amount: FungibleAssetAmount<A>,
}

impl<A: AssetClass> BalanceLog<A> {
    pub fn new(epoch_height: u64, amount: FungibleAssetAmount<A>) -> Self {
        Self {
            epoch_height: epoch_height.into(),
            amount,
        }
    }
}

pub enum SearchResult<T: AssetClass> {
    Found { index: u64, log: BalanceLog<T> },
    NotFound { index_below: Option<u64> },
}

pub fn search_balance_logs<T: AssetClass + BorshDeserialize>(
    logs: &Vector<BalanceLog<T>>,
    search_epoch_height: u64,
) -> SearchResult<T> {
    if logs.is_empty() {
        return SearchResult::NotFound { index_below: None };
    }

    let mut bottom = 0;
    let mut top = logs.len() - 1;

    while bottom <= top {
        let i = (bottom + top) / 2;
        let log = logs.get(i).unwrap_or_else(|| {
            near_sdk::env::panic_str("Invariant violation: All vector elements in range exist")
        });
        match log.epoch_height.0.cmp(&search_epoch_height) {
            Ordering::Less => {
                bottom = i + 1;
            }
            Ordering::Equal => {
                return SearchResult::Found { index: i, log };
            }
            Ordering::Greater => {
                if top == 0 {
                    return SearchResult::NotFound { index_below: None };
                }
                top = i - 1;
            }
        }
    }

    SearchResult::NotFound {
        index_below: Some(top),
    }
}
