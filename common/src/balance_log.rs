use std::cmp::Ordering;

use borsh::{BorshDeserialize, BorshSerialize};
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

pub fn add_or_update_balance_log<A: AssetClass + BorshDeserialize + BorshSerialize, T>(
    logs: &mut Vector<BalanceLog<A>>,
    chain_time: ChainTime,
    update_fn: impl FnOnce(&mut BalanceLog<A>) -> T,
) -> T {
    if let Some((last_index, mut last)) = logs.len().checked_sub(1).and_then(|last_index| {
        logs.get(last_index)
            .filter(|log| log.chain_time == chain_time)
            .map(|log| (last_index, log))
    }) {
        let s = update_fn(&mut last);
        logs.replace(last_index, &last);
        s
    } else {
        let mut new_log = BalanceLog::new(chain_time, FungibleAssetAmount::<A>::zero());
        let s = update_fn(&mut new_log);
        logs.push(&new_log);
        s
    }
}
