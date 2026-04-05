use std::{collections::HashMap, sync::OnceLock};

use near_sdk::{env, near, serde::de::DeserializeOwned, serde_json, AccountId};
use templar_common::{
    oracle::{
        pyth::{self, OracleResponse},
        redstone::{self, FeedData},
        OracleRequest, PythRequest, RedStoneRequest,
    },
    time::Nanoseconds,
    UnwrapReject,
};

static ERR_ORACLE_NOT_INVOKED: &str = "Invariant violation: oracle not invoked";

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
pub enum OracleType {
    Pyth(AccountId),
    RedStone(AccountId),
}

pub struct CallbackHandler<'a> {
    oracle_order: &'a [OracleType],
    pyth_results: HashMap<AccountId, OnceLock<Option<OracleResponse>>>,
    redstone_results: HashMap<AccountId, OnceLock<Option<HashMap<redstone::FeedId, FeedData>>>>,
    now: Nanoseconds,
    max_age: Nanoseconds,
}

impl<'a> CallbackHandler<'a> {
    pub fn new(oracle_order: &'a [OracleType], max_age: Nanoseconds) -> Self {
        let (pyth_results, redstone_results) = oracle_order.iter().fold(
            (HashMap::new(), HashMap::new()),
            |(mut pyth_results, mut redstone_results), oracle| {
                match oracle {
                    OracleType::Pyth(id) => {
                        pyth_results.insert(id.clone(), OnceLock::new());
                    }
                    OracleType::RedStone(id) => {
                        redstone_results.insert(id.clone(), OnceLock::new());
                    }
                }
                (pyth_results, redstone_results)
            },
        );
        Self {
            oracle_order,
            pyth_results,
            redstone_results,
            now: Nanoseconds::now(),
            max_age,
        }
    }

    fn oracle_index(&self, oracle: OracleType) -> u64 {
        self.oracle_order
            .iter()
            .position(|o| o == &oracle)
            .expect_or_reject(ERR_ORACLE_NOT_INVOKED) as u64
    }

    fn pyth(&self, request: &PythRequest) -> Option<pyth::Price> {
        self.pyth_results
            .get(&request.oracle_id)
            .expect_or_reject(ERR_ORACLE_NOT_INVOKED)
            .get_or_init(|| {
                let i = self.oracle_index(OracleType::Pyth(request.oracle_id.clone()));
                callback_result(i)
            })
            .as_ref()?
            .get(&request.price_id)?
            .clone()
    }

    fn redstone(&self, request: &RedStoneRequest) -> Option<pyth::Price> {
        self.redstone_results
            .get(&request.oracle_id)
            .expect_or_reject(ERR_ORACLE_NOT_INVOKED)
            .get_or_init(|| {
                let i = self.oracle_index(OracleType::RedStone(request.oracle_id.clone()));
                callback_result(i)
            })
            .as_ref()?
            .get(&request.price_id)
            .cloned()
            .and_then(|p| p.to_pyth_price())
    }

    pub fn get(&self, request: OracleRequest) -> Option<pyth::Price> {
        let price = match request {
            OracleRequest::Pyth(p) => self.pyth(&p),
            OracleRequest::RedStone(p) => self.redstone(&p),
        }?;

        // Filter for staleness
        let Some(publish_time) = Nanoseconds::try_from_pyth(price.publish_time) else {
            near_sdk::log!("Failed to convert publish_time");
            return None;
        };

        if self.now >= publish_time {
            let age = self.now.saturating_sub(publish_time);
            if age > self.max_age {
                near_sdk::log!("Price is stale: age={}, max_age={}", age, self.max_age);
                return None;
            }
        } else {
            // Future price/clock drift is handled by the Aggregator
        }

        Some(price)
    }
}

pub fn callback_result<T: DeserializeOwned>(index: u64) -> Option<T> {
    match env::promise_result(index) {
        near_sdk::PromiseResult::Successful(vec) => serde_json::from_slice(&vec).ok(),
        near_sdk::PromiseResult::Failed => None,
    }
}
