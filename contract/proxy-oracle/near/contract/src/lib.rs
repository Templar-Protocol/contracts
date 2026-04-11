#![allow(clippy::needless_pass_by_value)]

use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};

use near_sdk::{env, near, AccountId, Gas, PanicOnDefault, PromiseOrValue};
use near_sdk_contract_tools::{owner::Owner, Owner};
use templar_common::{
    contract::list,
    number::Decimal,
    oracle::{
        pyth::{ext_pyth, OracleResponse, PriceIdentifier},
        redstone::{self, ext_redstone},
    },
    self_ext,
    time::Nanoseconds,
    versioned_state::{impl_versioned_state, StateVersion, VersionedState},
    UnwrapReject,
};
use templar_proxy_oracle_kernel::{
    proxy::{Proxy, Source},
    request::OracleRequest,
    state,
};

mod callback_handler;
use callback_handler::{callback_result, CallbackHandler, OracleType};
mod impl_governance;

type State = state::v1::State;

#[derive(Debug, Owner, PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    pub state: VersionedState<State>,
}
impl_versioned_state!(Contract, State, state::migration::Migration);

impl Deref for Contract {
    type Target = State;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for Contract {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

#[near]
impl Contract {
    pub const GAS_FOR_PYTH_REQUEST: Gas = Gas::from_tgas(16).saturating_div(10);
    pub const GAS_FOR_REDSONE_REQUEST: Gas = Gas::from_tgas(17).saturating_div(10);

    #[init]
    pub fn new() -> Self {
        let mut self_ = Self {
            state: State::new(()),
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

    // impl Pyth:

    pub fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> bool {
        self.proxies.get(&price_identifier).is_some()
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
        let price_ids = HashSet::<PriceIdentifier>::from_iter(price_ids);

        let max_age = Nanoseconds::from_secs(age);

        let mut invoked = Vec::with_capacity(price_ids.len());
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

            invoked.push((*price_id, proxy.clone()));

            for source in proxy.sources() {
                let request = match source {
                    Source::Request(request) => request,
                    Source::Transformer(transformer) => {
                        transformer_promises.push(transformer.call.promise());
                        &transformer.request
                    }
                };

                match request {
                    OracleRequest::Pyth(p) => {
                        pyth_requests
                            .entry(p.oracle_id.clone())
                            .or_default()
                            .insert(p.price_id);
                    }
                    OracleRequest::RedStone(p) => {
                        redstone_requests
                            .entry(p.oracle_id.clone())
                            .or_default()
                            .insert(p.price_id.clone());
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
                        invoked,
                        max_age,
                    ),
            ),
        )
    }

    pub const GAS_FOR_LIST_01_CALLBACK: Gas = Gas::from_tgas(19).saturating_div(10);
    #[private]
    pub fn list_ema_prices_no_older_than_01_consume_results(
        &self,
        oracle_order: Vec<OracleType>,
        invoked: Vec<(PriceIdentifier, Proxy)>,
        max_age: Nanoseconds,
    ) -> OracleResponse {
        let callback = CallbackHandler::new(&oracle_order, max_age);
        let mut results = OracleResponse::with_capacity(invoked.len());

        let now = Nanoseconds::now();

        let mut i = oracle_order.len() as u64;
        for (price_id, proxy) in invoked {
            let mut prices = vec![];

            for source in proxy.sources() {
                let source_result = match source {
                    Source::Transformer(transformer) => {
                        let price = callback.get(&transformer.request);
                        let input = callback_result::<Decimal>(i);
                        i += 1;

                        price
                            .zip(input)
                            .and_then(|(price, input)| transformer.action.apply(price, input))
                    }
                    Source::Request(request) => callback.get(request),
                };

                prices.push(source_result);
            }

            let result = proxy.resolve(prices, now);

            if let Err(ref error) = result {
                near_sdk::log!("Aggregation error: {error}");
            }

            results.insert(price_id, result.ok());
        }

        results
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
