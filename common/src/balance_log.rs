use std::cmp::Ordering;

use borsh::BorshDeserialize;
use near_sdk::{collections::Vector, near};

use crate::{
    asset::{AssetClass, FungibleAssetAmount},
    chain_time::ChainTime,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct BalanceLog<A: AssetClass> {
    pub chain_time: ChainTime,
    pub amount: FungibleAssetAmount<A>,
}

impl<A: AssetClass> BalanceLog<A> {
    pub fn new(chain_time: ChainTime, amount: FungibleAssetAmount<A>) -> Self {
        Self { chain_time, amount }
    }
}

pub enum SearchResult<T: AssetClass> {
    Found { index: u64, log: BalanceLog<T> },
    NotFound { index_below: Option<u64> },
}

pub fn search_balance_logs<T: AssetClass + BorshDeserialize>(
    logs: &Vector<BalanceLog<T>>,
    target: ChainTime,
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
        match log.chain_time.cmp(&target) {
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
