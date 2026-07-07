use std::{collections::HashMap, sync::OnceLock};

use near_sdk::{env, near, serde::de::DeserializeOwned, serde_json, AccountId};
use templar_common::{
    oracle::{
        pyth::{self, FeedIdOracleResponse, OracleResponse},
        redstone::{self, FeedData},
    },
    UnwrapReject,
};
use templar_proxy_oracle_near_common::request::{
    LazerRequest, OracleRequest, PythRequest, RedStoneRequest,
};

static ERR_ORACLE_NOT_INVOKED: &str = "Invariant violation: oracle not invoked";

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
pub enum OracleType {
    Pyth(AccountId),
    RedStone(AccountId),
    Lazer(AccountId),
}

pub struct CallbackHandler<'a> {
    oracle_order: &'a [OracleType],
    pyth_results: HashMap<AccountId, OnceLock<Option<OracleResponse>>>,
    redstone_results: HashMap<AccountId, OnceLock<Option<HashMap<redstone::FeedId, FeedData>>>>,
    lazer_results: HashMap<AccountId, OnceLock<Option<FeedIdOracleResponse>>>,
}

impl<'a> CallbackHandler<'a> {
    pub fn new(oracle_order: &'a [OracleType]) -> Self {
        let mut pyth_results = HashMap::new();
        let mut redstone_results = HashMap::new();
        let mut lazer_results = HashMap::new();
        for oracle in oracle_order {
            match oracle {
                OracleType::Pyth(id) => {
                    pyth_results.insert(id.clone(), OnceLock::new());
                }
                OracleType::RedStone(id) => {
                    redstone_results.insert(id.clone(), OnceLock::new());
                }
                OracleType::Lazer(id) => {
                    lazer_results.insert(id.clone(), OnceLock::new());
                }
            }
        }
        Self {
            oracle_order,
            pyth_results,
            redstone_results,
            lazer_results,
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

    fn lazer(&self, request: &LazerRequest) -> Option<pyth::Price> {
        self.lazer_results
            .get(&request.oracle_id)
            .expect_or_reject(ERR_ORACLE_NOT_INVOKED)
            .get_or_init(|| {
                let i = self.oracle_index(OracleType::Lazer(request.oracle_id.clone()));
                callback_result(i)
            })
            .as_ref()?
            .get(&request.feed_id)?
            .clone()
    }

    pub fn get(&self, request: &OracleRequest) -> Option<pyth::Price> {
        match request {
            OracleRequest::Pyth(p) => self.pyth(p),
            OracleRequest::RedStone(p) => self.redstone(p),
            OracleRequest::Lazer(p) => self.lazer(p),
        }
    }
}

pub fn callback_result<T: DeserializeOwned>(index: u64) -> Option<T> {
    #[allow(deprecated)]
    match env::promise_result(index) {
        near_sdk::PromiseResult::Successful(vec) => serde_json::from_slice(&vec).ok(),
        near_sdk::PromiseResult::Failed => None,
    }
}
