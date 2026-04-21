use std::collections::HashMap;

use near_sdk::{
    env,
    json_types::{Base64VecU8, U64},
    near,
    store::LookupMap,
    AccountId, PanicOnDefault,
};
use templar_common::{
    oracle::{
        pyth::{Price, PriceIdentifier, Pyth},
        redstone::{
            config, Config, FeedData, FeedId, GetPrices, RedStoneContractInterface, Role,
            SerializableU256,
        },
    },
    time::Nanoseconds,
};

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    redstone_prices: LookupMap<FeedId, FeedData>,
    pyth_prices: LookupMap<PriceIdentifier, Price>,
    modify_roles: Vec<AccountId>,
    trusted_updaters: Vec<AccountId>,
    last_pyth_update_data: Option<String>,
    pyth_update_count: u64,
}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        Self {
            redstone_prices: LookupMap::new(b"r"),
            pyth_prices: LookupMap::new(b"p"),
            modify_roles: Vec::new(),
            trusted_updaters: Vec::new(),
            last_pyth_update_data: None,
            pyth_update_count: 0,
        }
    }

    pub fn get_config(&self) -> Config {
        config::test()
    }

    pub fn list_role(&self, role: Role) -> Vec<AccountId> {
        match role {
            Role::ModifyRoles => self.modify_roles.clone(),
            Role::TrustedUpdater => self.trusted_updaters.clone(),
        }
    }

    #[payable]
    pub fn set_role(&mut self, account_id: AccountId, role: Role, set: Option<bool>) {
        let set = set.unwrap_or(true);
        let entries = match role {
            Role::ModifyRoles => &mut self.modify_roles,
            Role::TrustedUpdater => &mut self.trusted_updaters,
        };

        if set {
            if !entries.contains(&account_id) {
                entries.push(account_id);
            }
        } else {
            entries.retain(|entry| entry != &account_id);
        }
    }

    pub fn set_redstone_price(&mut self, feed_id: FeedId, data: Option<FeedData>) {
        if let Some(data) = data {
            self.redstone_prices.insert(feed_id, data);
        } else {
            self.redstone_prices.remove(&feed_id);
        }
    }

    pub fn set_pyth_price(&mut self, price_identifier: PriceIdentifier, price: Option<Price>) {
        if let Some(price) = price {
            self.pyth_prices.insert(price_identifier, price);
        } else {
            self.pyth_prices.remove(&price_identifier);
        }
    }

    #[payable]
    pub fn update_price_feeds(&mut self, data: String) {
        self.last_pyth_update_data = Some(data);
        self.pyth_update_count += 1;
    }

    pub fn last_pyth_update_data(&self) -> Option<String> {
        self.last_pyth_update_data.clone()
    }

    pub fn pyth_update_count(&self) -> U64 {
        self.pyth_update_count.into()
    }
}

#[near]
impl Pyth for Contract {
    fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> bool {
        self.pyth_prices.contains_key(&price_identifier)
    }

    fn list_ema_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> HashMap<PriceIdentifier, Option<Price>> {
        let _ = age;
        let mut r = HashMap::new();
        for price_id in price_ids {
            r.insert(price_id, self.pyth_prices.get(&price_id).cloned());
        }
        r
    }

    fn list_ema_prices_unsafe(
        &self,
        price_ids: Vec<PriceIdentifier>,
    ) -> HashMap<PriceIdentifier, Option<Price>> {
        let mut r = HashMap::new();
        for price_id in price_ids {
            r.insert(price_id, self.pyth_prices.get(&price_id).cloned());
        }
        r
    }
}

#[allow(unused_variables)]
#[near]
impl RedStoneContractInterface for Contract {
    fn unique_signer_threshold(&self) -> U64 {
        U64(3)
    }

    fn get_prices(&self, feed_ids: Vec<FeedId>, payload: Base64VecU8) -> GetPrices {
        env::abort()
    }

    fn read_prices(&self, feed_ids: Vec<FeedId>) -> HashMap<FeedId, SerializableU256> {
        feed_ids
            .into_iter()
            .flat_map(|feed_id| {
                let price = self.redstone_prices.get(&feed_id)?.price;
                Some((feed_id, price))
            })
            .collect()
    }

    fn read_timestamp(&self, feed_id: FeedId) -> Option<Nanoseconds> {
        Some(self.redstone_prices.get(&feed_id)?.package_timestamp)
    }

    fn read_price_data_for_feed(&self, feed_id: FeedId) -> Option<FeedData> {
        self.redstone_prices.get(&feed_id).cloned()
    }

    fn read_price_data(&self, feed_ids: Vec<FeedId>) -> HashMap<FeedId, FeedData> {
        feed_ids
            .into_iter()
            .flat_map(|feed_id| {
                let data = self.redstone_prices.get(&feed_id)?;
                Some((feed_id, data.clone()))
            })
            .collect()
    }

    fn write_prices(&mut self, feed_ids: Vec<FeedId>, payload: Base64VecU8) {
        let now = Nanoseconds::from_ms(env::block_timestamp_ms());
        for feed_id in feed_ids {
            self.redstone_prices.insert(
                feed_id,
                FeedData {
                    price: primitive_types::U256::from((payload.0.len() as u128).max(1)).into(),
                    package_timestamp: now,
                    write_timestamp: now,
                },
            );
        }
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
