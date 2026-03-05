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
use near_sdk_contract_tools::{rbac::Rbac, Rbac};
use templar_common::{
    contract::list,
    number::Decimal,
    oracle::{
        proxy::{OracleType, Proxy, ProxyEntry, ProxyOracleEvent, Role},
        pyth::{self, ext_pyth, OracleResponse, PriceIdentifier},
        redstone::{self, ext_redstone, FeedData},
        OracleRequest, PythRequest, RedStoneRequest,
    },
    self_ext, UnwrapReject,
};

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Proxies,
}

#[derive(Debug, Rbac, PanicOnDefault)]
#[near(contract_state)]
#[rbac(roles = "Role")]
pub struct Contract {
    pub passthrough_pyth_id: AccountId,
    pub proxies: UnorderedMap<PriceIdentifier, Proxy>,
}

#[near]
impl Contract {
    pub const GAS_FOR_PYTH_REQUEST: Gas = Gas::from_tgas(16).saturating_div(10);
    pub const GAS_FOR_REDSONE_REQUEST: Gas = Gas::from_tgas(16).saturating_div(10);

    #[init]
    pub fn new(passthrough_pyth_id: AccountId) -> Self {
        let mut self_ = Self {
            passthrough_pyth_id,
            proxies: UnorderedMap::new(StorageKey::Proxies.into_storage_key()),
        };

        let deployer = env::predecessor_account_id();

        Rbac::add_role(&mut self_, &deployer, &Role::ModifyRole);
        Rbac::add_role(&mut self_, &deployer, &Role::AddProxy);

        self_
    }

    #[allow(clippy::unused_self)]
    fn assert_role_or_self(&self, role: Role) {
        let predecessor = env::predecessor_account_id();
        let current = env::current_account_id();
        if !(predecessor == current || <Self as Rbac>::has_role(&predecessor, &role)) {
            templar_common::panic_with_message(&format!("Missing role: {role:?}"));
        }
    }

    #[payable]
    pub fn set_role(
        &mut self,
        account_ids: Vec<AccountId>,
        roles: Vec<Role>,
        set: Option<bool>,
        allow_removing_final_member: Option<bool>,
    ) {
        assert_one_yocto();
        self.assert_role_or_self(Role::ModifyRole);

        let set = set.unwrap_or(true);
        let allow_removing_final_member = allow_removing_final_member.unwrap_or(false);

        if set {
            for role in roles {
                <Self as Rbac>::with_members_of_mut(&role, |r| {
                    for account_id in &account_ids {
                        if r.insert(account_id) {
                            ProxyOracleEvent::ModifyRole {
                                account_id: account_id.clone(),
                                role: role.clone(),
                                set: true,
                            }
                            .emit();
                        }
                    }
                });
            }
        } else {
            for role in roles {
                <Self as Rbac>::with_members_of_mut(&role, |r| {
                    for account_id in &account_ids {
                        if r.remove(account_id) {
                            ProxyOracleEvent::ModifyRole {
                                account_id: account_id.clone(),
                                role: role.clone(),
                                set: false,
                            }
                            .emit();
                        }
                    }

                    if !allow_removing_final_member {
                        require!(!r.is_empty(), "Deny removing final member");
                    }
                });
            }
        }
    }

    pub fn passthrough_pyth_id(&self) -> AccountId {
        self.passthrough_pyth_id.clone()
    }

    pub fn list_proxies(&self, offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier> {
        list(self.proxies.keys(), offset, count)
    }

    pub fn get_proxy(&self, id: PriceIdentifier) -> Option<Proxy> {
        self.proxies.get(&id)
    }

    #[payable]
    pub fn add_proxy(&mut self, proxy: Proxy) -> PriceIdentifier {
        assert_one_yocto();
        self.assert_role_or_self(Role::AddProxy);

        let id = proxy.id().unwrap_or_else(|e| {
            templar_common::panic_with_message(&format!("Failed to calculate proxy ID: {e}"))
        });

        if self.proxies.insert(&id, &proxy).is_some() {
            templar_common::panic_with_message(&format!("Proxy identifier collision: {id}"));
        }

        ProxyOracleEvent::AddProxy { id, proxy }.emit();

        id
    }

    // impl Pyth:

    pub fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> PromiseOrValue<bool> {
        if self.proxies.get(&price_identifier).is_some() {
            PromiseOrValue::Value(true)
        } else {
            PromiseOrValue::Promise(
                ext_pyth::ext(self.passthrough_pyth_id.clone())
                    .with_static_gas(Gas::from_tgas(2))
                    .price_feed_exists(price_identifier)
                    .then(self_ext!(Gas::from_tgas(1)).price_feed_exists_01_consume_result()),
            )
        }
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
                pyth_requests
                    .entry(self.passthrough_pyth_id.clone())
                    .or_default()
                    .insert(*price_id);
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
                result.insert(
                    price_id,
                    callback.pyth(&PythRequest {
                        oracle_id: self.passthrough_pyth_id.clone(),
                        price_id,
                    }),
                );
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
