use std::collections::HashMap;

use near_sdk::{near, store::LookupMap, PanicOnDefault};
use templar_common::oracle::pyth::{Price, PriceIdentifier, Pyth};

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    prices: LookupMap<PriceIdentifier, Price>,
}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        Self {
            prices: LookupMap::new(b"p"),
        }
    }

    pub fn set_price(&mut self, price_identifier: PriceIdentifier, price: Price) {
        self.prices.insert(price_identifier, price);
    }
}

#[near]
impl Pyth for Contract {
    fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> bool {
        self.prices.contains_key(&price_identifier)
    }

    fn list_ema_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> HashMap<PriceIdentifier, Option<Price>> {
        let _ = age;
        let mut r = HashMap::new();
        for price_id in price_ids {
            r.insert(price_id, self.prices.get(&price_id).cloned());
        }
        r
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
