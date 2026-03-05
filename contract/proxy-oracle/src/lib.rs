#![allow(clippy::needless_pass_by_value)]

use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

use near_sdk::{
    assert_one_yocto, borsh::BorshSerialize, collections::UnorderedMap, env, json_types::U64, near,
    require, serde::de::DeserializeOwned, serde_json, AccountId, BorshStorageKey, Gas,
    IntoStorageKey, PanicOnDefault, PromiseError, PromiseOrValue, PromiseResult,
};
use near_sdk_contract_tools::{owner::Owner, Owner};
use templar_common::{
    contract::list,
    number::Decimal,
    oracle::{
        proxy::{
            governance::{Operation, Proposal, ProxyOracleEvent},
            OracleType, Proxy, ProxyEntry,
        },
        pyth::{self, ext_pyth, OracleResponse, PriceIdentifier},
        redstone::{self, ext_redstone, FeedData},
        OracleRequest, PythRequest, RedStoneRequest,
    },
    self_ext, UnwrapReject,
};

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Proposals,
    Proxies,
}

#[derive(Debug, Owner, PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    pub next_op_id: u32,
    pub proposal_ttl_ms: u64,
    pub proposals: UnorderedMap<u32, Proposal>,
    pub proxies: UnorderedMap<PriceIdentifier, Proxy>,
}

#[near]
impl Contract {
    pub const GAS_FOR_PYTH_REQUEST: Gas = Gas::from_tgas(16).saturating_div(10);
    pub const GAS_FOR_REDSONE_REQUEST: Gas = Gas::from_tgas(16).saturating_div(10);

    #[init]
    pub fn new() -> Self {
        let mut self_ = Self {
            next_op_id: 0,
            proposal_ttl_ms: 0,
            proposals: UnorderedMap::new(StorageKey::Proposals.into_storage_key()),
            proxies: UnorderedMap::new(StorageKey::Proxies.into_storage_key()),
        };

        let deployer = env::predecessor_account_id();

        Owner::init(&mut self_, &deployer);

        self_
    }

    pub fn list_proxies(&self, offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier> {
        list(self.proxies.keys(), offset, count)
    }

    pub fn get_proxy(&self, id: PriceIdentifier) -> Option<Proxy> {
        self.proxies.get(&id)
    }

    pub fn get_proposal_ttl_ms(&self) -> U64 {
        U64(self.proposal_ttl_ms)
    }

    fn internal_execute(&mut self, op_id: u32, operation: Operation) {
        match &operation {
            Operation::SetProxy { id, proxy } => {
                if let Some(proxy) = proxy {
                    self.proxies.insert(id, proxy);
                } else {
                    self.proxies.remove(id);
                }
            }
            Operation::SetActionTtl { new_ttl_ms } => {
                self.proposal_ttl_ms = new_ttl_ms.0;
            }
        }

        ProxyOracleEvent::Execution { op_id, operation }.emit();
    }

    pub fn get_proposal(&self, op_id: u32) -> Option<Proposal> {
        self.proposals.get(&op_id)
    }

    #[payable]
    pub fn execute(&mut self, op_id: u32) {
        assert_one_yocto();
        self.assert_owner();

        let proposal = self
            .proposals
            .remove(&op_id)
            .expect_or_reject("No proposal with the given ID");

        require!(
            proposal.can_execute(env::block_timestamp_ms(), self.proposal_ttl_ms),
            "Cannot execute proposal before TTL has passed"
        );

        self.internal_execute(op_id, proposal.operation);
    }

    #[payable]
    pub fn cancel(&mut self, op_id: u32) {
        assert_one_yocto();
        self.assert_owner();

        let proposal = self
            .proposals
            .remove(&op_id)
            .expect_or_reject("No proposal with the given ID");

        ProxyOracleEvent::Cancellation { op_id, proposal }.emit();
    }

    #[payable]
    pub fn propose(&mut self, operation: Operation) -> u32 {
        assert_one_yocto();
        self.assert_owner();

        let op_id = self.next_op_id;
        self.next_op_id = self
            .next_op_id
            .checked_add(1)
            .expect_or_reject("Governance action ID overflow");

        let proposal = Proposal {
            operation,
            created_at_ms: U64(env::block_timestamp_ms()),
        };

        ProxyOracleEvent::Proposal {
            op_id,
            proposal: proposal.clone(),
        }
        .emit();

        if self.proposal_ttl_ms == 0 {
            // If TTL is 0, execute immediately
            self.internal_execute(op_id, proposal.operation);
        } else {
            self.proposals.insert(&op_id, &proposal);
        }

        op_id
    }

    // impl Pyth:

    pub fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> bool {
        self.proxies.get(&price_identifier).is_some()
    }

    pub fn price_feed_exists_01_consume_result(
        &self,
        #[callback_result] result: Result<bool, PromiseError>,
    ) -> bool {
        result.unwrap_or(false)
    }

    pub const GAS_FOR_LIST_00_ENTRY: Gas = Gas::from_tgas(35).saturating_div(10);
    pub fn list_ema_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> PromiseOrValue<OracleResponse> {
        if price_ids.is_empty() {
            return PromiseOrValue::Value(OracleResponse::new());
        }

        let max_age_ms = age * 1000;

        let mut pyth_requests =
            HashMap::<AccountId, HashSet<PriceIdentifier>>::with_capacity(price_ids.len());
        let mut redstone_requests =
            HashMap::<AccountId, HashSet<redstone::FeedId>>::with_capacity(price_ids.len());
        let mut transformer_promises = Vec::with_capacity(price_ids.len());

        for price_id in &price_ids {
            let Some(proxy) = self.proxies.get(price_id) else {
                // Skip unknown.
                continue;
            };

            for entry in proxy.0 {
                let request = match entry {
                    ProxyEntry::Request(request) => request,
                    ProxyEntry::Transformer(transformer) => {
                        transformer_promises.push(transformer.call.promise());
                        transformer.request
                    }
                };

                match request {
                    OracleRequest::Pyth(p) => {
                        pyth_requests
                            .entry(p.oracle_id)
                            .or_default()
                            .insert(p.price_id);
                    }
                    OracleRequest::RedStone(p) => {
                        redstone_requests
                            .entry(p.oracle_id)
                            .or_default()
                            .insert(p.price_id);
                    }
                }
            }
        }

        let mut oracle_order = Vec::with_capacity(pyth_requests.len() + redstone_requests.len());
        let mut oracle_promises = Vec::with_capacity(pyth_requests.len() + redstone_requests.len());

        for (oracle_id, price_ids) in pyth_requests {
            oracle_order.push(OracleType::Pyth(oracle_id.clone()));
            oracle_promises.push(
                ext_pyth::ext(oracle_id)
                    .with_static_gas(Self::GAS_FOR_PYTH_REQUEST)
                    .list_ema_prices_no_older_than(Vec::from_iter(price_ids), age),
            );
        }

        for (oracle_id, price_ids) in redstone_requests {
            oracle_order.push(OracleType::RedStone(oracle_id.clone()));
            oracle_promises.push(
                ext_redstone::ext(oracle_id)
                    .with_static_gas(Self::GAS_FOR_REDSONE_REQUEST)
                    .read_price_data(Vec::from_iter(price_ids)),
            );
        }

        let promise = oracle_promises
            .into_iter()
            .chain(transformer_promises)
            .reduce(near_sdk::Promise::and)
            .expect_or_reject("No oracle invoked");

        PromiseOrValue::Promise(
            promise.then(
                self_ext!(Self::GAS_FOR_LIST_01_CALLBACK)
                    .list_ema_prices_no_older_than_01_consume_results(
                        oracle_order,
                        price_ids,
                        U64(max_age_ms),
                    ),
            ),
        )
    }

    pub const GAS_FOR_LIST_01_CALLBACK: Gas = Gas::from_tgas(19).saturating_div(10);
    #[private]
    pub fn list_ema_prices_no_older_than_01_consume_results(
        &self,
        oracle_order: Vec<OracleType>,
        original_price_ids: Vec<PriceIdentifier>,
        max_age_ms: U64,
    ) -> OracleResponse {
        let callback = CallbackHandler::new(&oracle_order, max_age_ms.0);
        let mut result = OracleResponse::with_capacity(original_price_ids.len());

        let mut i = oracle_order.len() as u64;
        for price_id in original_price_ids {
            let Some(proxy) = self.proxies.get(&price_id) else {
                // Skip unknown.
                continue;
            };

            let mut value = None;

            for entry in proxy.0 {
                let entry_result = match entry {
                    ProxyEntry::Transformer(transformer) => {
                        let price = callback.get(transformer.request);
                        let input = callback_result::<Decimal>(i);
                        i += 1;

                        price
                            .zip(input)
                            .and_then(|(price, input)| transformer.action.apply(price, input))
                    }
                    ProxyEntry::Request(p) => callback.get(p),
                };

                value = value.or(entry_result);
            }

            result.insert(price_id, value);
        }

        result
    }
}

static ERR_ORACLE_NOT_INVOKED: &str = "Invariant violation: oracle not invoked";

struct CallbackHandler<'a> {
    oracle_order: &'a [OracleType],
    pyth_results: HashMap<AccountId, OnceLock<Option<OracleResponse>>>,
    redstone_results: HashMap<AccountId, OnceLock<Option<HashMap<redstone::FeedId, FeedData>>>>,
    now_ms: u64,
    max_age_ms: u64,
}

impl<'a> CallbackHandler<'a> {
    fn new(oracle_order: &'a [OracleType], max_age_ms: u64) -> Self {
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

    fn get(&self, request: OracleRequest) -> Option<pyth::Price> {
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

fn callback_result<T: DeserializeOwned>(index: u64) -> Option<T> {
    match env::promise_result(index) {
        PromiseResult::Successful(vec) => serde_json::from_slice(&vec).ok(),
        PromiseResult::Failed => None,
    }
}

#[cfg(target_arch = "wasm32")]
mod custom_getrandom {
    #![allow(clippy::no_mangle_with_rust_abi)]

    use getrandom::{register_custom_getrandom, Error};
    use near_sdk::env;

    register_custom_getrandom!(custom_getrandom);

    #[allow(clippy::unnecessary_wraps)]
    pub fn custom_getrandom(buf: &mut [u8]) -> Result<(), Error> {
        buf.copy_from_slice(&env::random_seed_array());
        Ok(())
    }
}
