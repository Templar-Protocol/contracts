use std::{collections::HashMap, sync::OnceLock};

use near_sdk::{env, serde::de::DeserializeOwned, serde_json, AccountId, PromiseResult};
use templar_common::{
    oracle::{
        proxy::OracleType,
        pyth::{self, OracleResponse},
        redstone::{self, FeedData},
        OracleRequest, PythRequest, RedStoneRequest,
    },
    UnwrapReject,
};

static ERR_ORACLE_NOT_INVOKED: &str = "Invariant violation: oracle not invoked";

pub struct CallbackHandler<'a> {
    oracle_order: &'a [OracleType],
    pyth_results: HashMap<AccountId, OnceLock<Option<OracleResponse>>>,
    redstone_results: HashMap<AccountId, OnceLock<Option<HashMap<redstone::FeedId, FeedData>>>>,
    now_ms: u64,
    max_age_ms: u64,
}

impl<'a> CallbackHandler<'a> {
    pub fn new(oracle_order: &'a [OracleType], max_age_ms: u64) -> Self {
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
            now_ms: env::block_timestamp_ms(),
            max_age_ms,
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
        let publish_time = match u64::try_from(price.publish_time) {
            Ok(p) => p,
            Err(e) => {
                near_sdk::log!("Failed to convert publish_time to u64: {e}");
                return None;
            }
        };
        let price_age_ms = self.now_ms.saturating_sub(publish_time);
        if price_age_ms > self.max_age_ms {
            return None;
        }

        Some(price)
    }
}

pub fn callback_result<T: DeserializeOwned>(index: u64) -> Option<T> {
    match env::promise_result(index) {
        PromiseResult::Successful(vec) => serde_json::from_slice(&vec).ok(),
        PromiseResult::Failed => None,
    }
}
