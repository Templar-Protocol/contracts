#![allow(clippy::needless_pass_by_value)]

use std::sync::OnceLock;

use near_sdk::{
    assert_one_yocto, borsh::BorshSerialize, collections::UnorderedMap, env, near, require,
    serde::de::DeserializeOwned, serde_json, AccountId, BorshStorageKey, Gas, IntoStorageKey,
    PanicOnDefault, PromiseError, PromiseOrValue, PromiseResult,
};
use near_sdk_contract_tools::{rbac::Rbac, Rbac};
use templar_common::{
    contract::list,
    number::Decimal,
    oracle::{
        proxy::Proxy,
        pyth::{self, ext_pyth, OracleResponse, PriceIdentifier},
        redstone::{ext_redstone, feed_data::FeedData},
        OraclePriceId,
    },
    self_ext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub enum Oracle {
    Pyth,
    RedStone,
}

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Proxied,
}

#[derive(Debug, Clone, BorshStorageKey)]
#[near(serializers = [json, borsh])]
pub enum Role {
    ModifyRoles,
    SetOracleId,
    AddProxy,
    Upgrade,
}

#[derive(Debug, Rbac, PanicOnDefault)]
#[near(contract_state)]
#[rbac(roles = "Role")]
pub struct Contract {
    pub redstone_id: AccountId,
    pub pyth_id: AccountId,
    pub proxies: UnorderedMap<PriceIdentifier, Proxy>,
}

#[near]
impl Contract {
    #[init]
    pub fn new(redstone_id: AccountId, pyth_id: AccountId) -> Self {
        let mut self_ = Self {
            redstone_id,
            pyth_id,
            proxies: UnorderedMap::new(StorageKey::Proxied.into_storage_key()),
        };

        let deployer = env::predecessor_account_id();

        Rbac::add_role(&mut self_, &deployer, &Role::ModifyRoles);
        Rbac::add_role(&mut self_, &deployer, &Role::SetOracleId);
        Rbac::add_role(&mut self_, &deployer, &Role::AddProxy);
        Rbac::add_role(&mut self_, &deployer, &Role::Upgrade);

        self_
    }

    #[allow(clippy::unused_self)]
    fn assert_role_or_self(&self, role: Role) {
        let predecessor = env::predecessor_account_id();
        let current = env::current_account_id();
        require!(
            predecessor == current || <Self as Rbac>::has_role(&predecessor, &role),
            "missing role"
        );
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
        self.assert_role_or_self(Role::ModifyRoles);

        let set = set.unwrap_or(true);
        let allow_removing_final_member = allow_removing_final_member.unwrap_or(false);

        if set {
            for role in roles {
                <Self as Rbac>::with_members_of_mut(&role, |r| {
                    for account_id in &account_ids {
                        r.insert(account_id);
                    }
                });
            }
        } else {
            for role in roles {
                <Self as Rbac>::with_members_of_mut(&role, |r| {
                    for account_id in &account_ids {
                        r.remove(account_id);
                    }

                    if !allow_removing_final_member {
                        require!(!r.is_empty(), "removing final member disallowed");
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
                self.pyth_id = account_id;
            }
            Oracle::RedStone => {
                self.redstone_id = account_id;
            }
        }
    }

    pub fn list_proxies(&self, offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier> {
        list(self.proxies.keys(), offset, count)
    }

    pub fn get_proxy(&self, id: PriceIdentifier) -> Option<Proxy> {
        self.proxies.get(&id)
    }

    #[payable]
    pub fn add_proxy(&mut self, id: PriceIdentifier, proxy: Proxy) {
        assert_one_yocto();
        self.assert_role_or_self(Role::AddProxy);

        if self.proxies.insert(&id, &proxy).is_some() {
            templar_common::panic_with_message("Price identifier collision");
        }
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

        let mut pyth_price_ids = Vec::with_capacity(price_ids.len());
        let mut redstone_price_ids = Vec::with_capacity(price_ids.len());
        let mut promises = Vec::with_capacity(price_ids.len());

        for price_id in &price_ids {
            let Some(proxy) = self.proxies.get(price_id) else {
                pyth_price_ids.push(*price_id);
                continue;
            };

            match proxy {
                Proxy::Transformer(transformer) => {
                    match transformer.price_id {
                        OraclePriceId::Pyth(id) => {
                            pyth_price_ids.push(id);
                        }
                        OraclePriceId::RedStone(id) => {
                            redstone_price_ids.push(id);
                        }
                    }
                    promises.push(transformer.call.promise());
                }
                Proxy::List(ids) => {
                    for id in ids {
                        match id {
                            OraclePriceId::Pyth(id) => {
                                pyth_price_ids.push(id);
                            }
                            OraclePriceId::RedStone(id) => {
                                redstone_price_ids.push(id);
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
                .list_ema_prices_no_older_than(pyth_price_ids, age)
        });
        if pyth_promise.is_some() {
            oracles.push(Oracle::Pyth);
        }

        let redstone_promise = (!redstone_price_ids.is_empty()).then(|| {
            ext_redstone::ext(self.redstone_id.clone())
                .with_static_gas(Gas::from_tgas(3))
                .read_price_data(redstone_price_ids.clone())
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
                redstone_price_ids,
                price_ids,
            ),
        ))
    }

    #[private]
    pub fn list_ema_prices_no_older_than_01_consume_results(
        &self,
        oracles: Vec<Oracle>,
        redstone_price_ids: Vec<String>,
        original_price_ids: Vec<PriceIdentifier>,
    ) -> OracleResponse {
        fn callback_result<T: DeserializeOwned>(index: u64) -> T {
            match env::promise_result(index) {
                PromiseResult::Successful(vec) => serde_json::from_slice(&vec)
                    .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string())),
                PromiseResult::Failed => {
                    templar_common::panic_with_message(&format!("Promise index {index} failed"))
                }
            }
        }

        let pyth_result = |price_identifier: &PriceIdentifier| -> Option<pyth::Price> {
            static RESPONSE: OnceLock<OracleResponse> = OnceLock::new();
            RESPONSE
                .get_or_init(|| {
                    callback_result::<OracleResponse>(
                        oracles
                            .iter()
                            .position(|o| *o == Oracle::Pyth)
                            .unwrap_or_else(|| {
                                templar_common::panic_with_message(
                                    "Invariant violation: oracle not invoked",
                                )
                            }) as u64,
                    )
                })
                .get(price_identifier)?
                .clone()
        };

        #[allow(clippy::cast_possible_truncation)]
        let redstone_result = |index: u64| -> Option<FeedData> {
            static RESPONSE: OnceLock<Vec<FeedData>> = OnceLock::new();
            RESPONSE
                .get_or_init(|| {
                    callback_result::<Vec<FeedData>>(
                        oracles
                            .iter()
                            .position(|o| *o == Oracle::RedStone)
                            .unwrap_or_else(|| {
                                templar_common::panic_with_message(
                                    "Invariant violation: oracle not invoked",
                                )
                            }) as u64,
                    )
                })
                .get(index as usize)
                .cloned()
        };

        let get_price = |price_id: OraclePriceId| match price_id {
            OraclePriceId::Pyth(id) => pyth_result(&id),
            OraclePriceId::RedStone(id) => redstone_result(
                redstone_price_ids
                    .iter()
                    .position(|p| p == &id)
                    .unwrap_or_else(|| {
                        templar_common::panic_with_message(
                            "Invariant violation: RedStone ID not requested",
                        )
                    }) as u64,
            )
            .and_then(|f| f.to_pyth_price(8)),
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
                        price.and_then(|price| transformer.action.apply(price, input)),
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
