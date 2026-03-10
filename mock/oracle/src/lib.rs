use std::collections::HashMap;

use near_sdk::{
    env,
    json_types::{Base64VecU8, U64},
    near,
    store::LookupMap,
    PanicOnDefault,
};
use templar_common::oracle::{
    pyth::{Price, PriceIdentifier, Pyth},
    redstone::{FeedData, FeedId, GetPrices, RedStoneContractInterface, SerializableU256},
};

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    redstone_prices: LookupMap<FeedId, FeedData>,
    pyth_prices: LookupMap<PriceIdentifier, Price>,
}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        Self {
            redstone_prices: LookupMap::new(b"r"),
            pyth_prices: LookupMap::new(b"p"),
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

    fn read_timestamp(&self, feed_id: FeedId) -> Option<U64> {
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
        env::abort()
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
