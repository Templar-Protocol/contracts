#![allow(clippy::needless_pass_by_value)]

use std::collections::{HashMap, HashSet};

use near_sdk::{
    borsh::BorshSerialize, collections::UnorderedMap, env, json_types::U64, near, AccountId,
    BorshStorageKey, Gas, IntoStorageKey, PanicOnDefault, PromiseError, PromiseOrValue,
};
use near_sdk_contract_tools::{owner::Owner, Owner};
use templar_common::{
    contract::list,
    number::Decimal,
    oracle::{
        proxy::{
            governance::{Governance, Operation},
            OracleType, Proxy, Source,
        },
        pyth::{ext_pyth, OracleResponse, PriceIdentifier},
        redstone::{self, ext_redstone},
        OracleRequest,
    },
    self_ext, UnwrapReject,
};

mod callback_handler;
use callback_handler::{callback_result, CallbackHandler};
mod impl_governance;

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Governance,
    Proxies,
}

#[derive(Debug, Owner, PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    pub governance: Governance<Operation>,
    pub proxies: UnorderedMap<PriceIdentifier, Proxy>,
}

#[near]
impl Contract {
    pub const GAS_FOR_PYTH_REQUEST: Gas = Gas::from_tgas(16).saturating_div(10);
    pub const GAS_FOR_REDSONE_REQUEST: Gas = Gas::from_tgas(16).saturating_div(10);

    #[init]
    pub fn new() -> Self {
        let mut self_ = Self {
            governance: Governance::new(StorageKey::Governance),
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

        let max_age_ms = age.saturating_mul(1000);

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

            for entry in proxy.entries {
                let request = match entry.source {
                    Source::Request(request) => request,
                    Source::Transformer(transformer) => {
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
        // TODO: Race condition if the owner changes the oracle definition during the callback.
        let callback = CallbackHandler::new(&oracle_order, max_age_ms.0);
        let mut result = OracleResponse::with_capacity(original_price_ids.len());

        let mut i = oracle_order.len() as u64;
        for price_id in original_price_ids {
            let Some(proxy) = self.proxies.get(&price_id) else {
                // Skip unknown.
                continue;
            };

            let mut value = None;

            for entry in proxy.entries {
                let entry_result = match entry.source {
                    Source::Transformer(transformer) => {
                        let price = callback.get(transformer.request);
                        let input = callback_result::<Decimal>(i);
                        i += 1;

                        price
                            .zip(input)
                            .and_then(|(price, input)| transformer.action.apply(price, input))
                    }
                    Source::Request(p) => callback.get(p),
                };

                value = value.or(entry_result);
            }

            result.insert(price_id, value);
        }

        result
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
