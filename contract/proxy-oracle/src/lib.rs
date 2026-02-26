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
        proxy::{Oracle, Proxy, ProxyOracleEvent, Role},
        pyth::{self, ext_pyth, OracleResponse, PriceIdentifier},
        redstone::{self, ext_redstone, FeedData},
        OraclePriceId,
    },
    self_ext,
};

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Proxied,
}

#[derive(Debug, Rbac, PanicOnDefault)]
#[near(contract_state)]
#[rbac(roles = "Role")]
pub struct Contract {
    pub pyth_id: AccountId,
    pub redstone_id: AccountId,
    pub proxies: UnorderedMap<PriceIdentifier, Proxy>,
}

#[near]
impl Contract {
    #[init]
    pub fn new(pyth_id: AccountId, redstone_id: AccountId) -> Self {
        let mut self_ = Self {
            pyth_id,
            redstone_id,
            proxies: UnorderedMap::new(StorageKey::Proxied.into_storage_key()),
        };

        let deployer = env::predecessor_account_id();

        Rbac::add_role(&mut self_, &deployer, &Role::ModifyRole);
        Rbac::add_role(&mut self_, &deployer, &Role::SetOracleId);
        Rbac::add_role(&mut self_, &deployer, &Role::AddProxy);
        Rbac::add_role(&mut self_, &deployer, &Role::Upgrade);

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

    pub fn oracle_id(&self, oracle: Oracle) -> &AccountId {
        match oracle {
            Oracle::Pyth => &self.pyth_id,
            Oracle::RedStone => &self.redstone_id,
        }
    }

    #[payable]
    pub fn set_oracle_id(&mut self, oracle: Oracle, account_id: AccountId) {
        assert_one_yocto();
        self.assert_role_or_self(Role::SetOracleId);

        match oracle {
            Oracle::Pyth => {
                self.pyth_id = account_id.clone();
            }
            Oracle::RedStone => {
                self.redstone_id = account_id.clone();
            }
        }

        ProxyOracleEvent::SetOracleId {
            oracle,
            oracle_id: account_id,
        }
        .emit();
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
                ext_pyth::ext(self.pyth_id.clone())
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

    // TODO: Recalculate gas values
    // GAS:
    // Base: 3 (underlying oracle) + 2 (entry) + 3 (callback) + n*3 (redemption rate calls)
    // Max should be 3 + 2 + 3 + 2 * 3 = 14, plus a bit of buffer => 15

    pub fn list_ema_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> PromiseOrValue<OracleResponse> {
        if price_ids.is_empty() {
            return PromiseOrValue::Value(OracleResponse::new());
        }

        let max_age_ms = age * 1000;

        let mut pyth_price_ids = HashSet::with_capacity(price_ids.len());
        let mut redstone_price_ids = HashSet::with_capacity(price_ids.len());
        let mut promises = Vec::with_capacity(price_ids.len());

        for price_id in &price_ids {
            let Some(proxy) = self.proxies.get(price_id) else {
                pyth_price_ids.insert(*price_id);
                continue;
            };

            match proxy {
                Proxy::Transformer(transformer) => {
                    match transformer.price_id {
                        OraclePriceId::Pyth(id) => {
                            pyth_price_ids.insert(id);
                        }
                        OraclePriceId::RedStone(id) => {
                            redstone_price_ids.insert(id);
                        }
                    }
                    promises.push(transformer.call.promise());
                }
                Proxy::List(ids) => {
                    for id in ids {
                        match id {
                            OraclePriceId::Pyth(id) => {
                                pyth_price_ids.insert(id);
                            }
                            OraclePriceId::RedStone(id) => {
                                redstone_price_ids.insert(id);
                            }
                        }
                    }
                }
            }
        }

        let mut oracles = Vec::with_capacity(2);

        let pyth_promise = (!pyth_price_ids.is_empty()).then(|| {
            ext_pyth::ext(self.pyth_id.clone())
                .with_static_gas(Gas::from_tgas(3))
                .list_ema_prices_no_older_than(Vec::from_iter(pyth_price_ids), age)
        });
        if pyth_promise.is_some() {
            oracles.push(Oracle::Pyth);
        }

        let redstone_promise = (!redstone_price_ids.is_empty()).then(|| {
            ext_redstone::ext(self.redstone_id.clone())
                .with_static_gas(Gas::from_tgas(3))
                .read_price_data(Vec::from_iter(redstone_price_ids))
        });
        if redstone_promise.is_some() {
            oracles.push(Oracle::RedStone);
        }

        let mut it = [pyth_promise, redstone_promise]
            .into_iter()
            .flatten()
            .chain(promises);

        let mut promise = it
            .next()
            .unwrap_or_else(|| templar_common::panic_with_message("No oracle invoked"));

        for p in it {
            promise = promise.and(p);
        }

        PromiseOrValue::Promise(promise.then(
            self_ext!(Gas::from_tgas(3)).list_ema_prices_no_older_than_01_consume_results(
                oracles,
                price_ids,
                U64(max_age_ms),
            ),
        ))
    }

    #[private]
    pub fn list_ema_prices_no_older_than_01_consume_results(
        &self,
        oracles: Vec<Oracle>,
        original_price_ids: Vec<PriceIdentifier>,
        max_age_ms: U64,
    ) -> OracleResponse {
        fn callback_result<T: DeserializeOwned>(index: u64) -> Option<T> {
            match env::promise_result(index) {
                PromiseResult::Successful(vec) => serde_json::from_slice(&vec).ok(),
                PromiseResult::Failed => None,
            }
        }

        let oracle_ix = |oracle: Oracle| -> u64 {
            match &oracles[..] {
                [a] | [a, _] if a == &oracle => 0,
                [_, a] if a == &oracle => 1,
                _ => templar_common::panic_with_message("Invariant violation: oracle not invoked"),
            }
        };

        let pyth_result = |price_identifier: &PriceIdentifier| -> Option<pyth::Price> {
            static RESPONSE: OnceLock<Option<OracleResponse>> = OnceLock::new();
            RESPONSE
                .get_or_init(|| callback_result(oracle_ix(Oracle::Pyth)))
                .as_ref()?
                .get(price_identifier)?
                .clone()
        };

        #[allow(clippy::cast_possible_truncation)]
        let redstone_result = |feed_id: &redstone::FeedId| -> Option<pyth::Price> {
            static RESPONSE: OnceLock<Option<HashMap<redstone::FeedId, FeedData>>> =
                OnceLock::new();
            RESPONSE
                .get_or_init(|| callback_result(oracle_ix(Oracle::RedStone)))
                .as_ref()?
                .get(feed_id)
                .cloned()
                .and_then(|p| p.to_pyth_price())
        };

        let now_ms = env::block_timestamp_ms();
        let get_price = |price_id: OraclePriceId| {
            let price = match price_id {
                OraclePriceId::Pyth(id) => pyth_result(&id),
                OraclePriceId::RedStone(id) => redstone_result(&id),
            }?;

            // Filter for staleness
            let price_age_ms =
                now_ms.saturating_sub(u64::try_from(price.publish_time).unwrap_or(0));
            (price_age_ms <= max_age_ms.0).then_some(price)
        };

        let mut result = OracleResponse::with_capacity(original_price_ids.len());

        let mut i = oracles.len() as u64;
        for price_id in original_price_ids {
            let Some(proxy) = self.proxies.get(&price_id) else {
                result.insert(price_id, pyth_result(&price_id));
                continue;
            };

            match proxy {
                Proxy::Transformer(transformer) => {
                    let price = get_price(transformer.price_id);
                    let input = callback_result::<Decimal>(i);
                    i += 1;

                    result.insert(
                        price_id,
                        price
                            .zip(input)
                            .and_then(|(price, input)| transformer.action.apply(price, input)),
                    );
                }
                Proxy::List(ids) => {
                    let price = ids.into_iter().find_map(get_price);

                    result.insert(price_id, price);
                }
            }
        }

        result
    }

    // TODO: Upgradability
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
