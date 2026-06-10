//! Mock upstream SEP-40 price feed.
//!
//! The proxy-oracle runtime configures each source via `ProxyConfig.sources`
//! and reads it through the generated `PriceFeedClient` (calls `base()`,
//! `decimals()`, `lastprice(asset)`). This contract implements that surface
//! with instance storage you can poke from test code.

use soroban_sdk::{contract, contractimpl, contracttype, Env, Symbol, Vec};
use templar_proxy_oracle_soroban_common::{Asset, PriceData, PriceFeedTrait};

#[derive(Clone)]
#[contracttype]
enum Key {
    Base,
    Decimals,
    Resolution,
    Price(Asset),
}

#[contract]
pub struct MockOracle;

#[contractimpl]
impl MockOracle {
    pub fn __constructor(env: Env, base: Asset, decimals: u32, resolution: u32) {
        env.storage().instance().set(&Key::Base, &base);
        env.storage().instance().set(&Key::Decimals, &decimals);
        env.storage().instance().set(&Key::Resolution, &resolution);
    }

    pub fn set_price(env: Env, asset: Asset, price: i128, timestamp: u64) {
        env.storage()
            .persistent()
            .set(&Key::Price(asset), &PriceData { price, timestamp });
    }
}

#[contractimpl]
impl PriceFeedTrait for MockOracle {
    fn base(env: Env) -> Asset {
        env.storage()
            .instance()
            .get(&Key::Base)
            .unwrap_or(Asset::Other(Symbol::new(&env, "USD")))
    }

    fn assets(env: Env) -> Vec<Asset> {
        Vec::new(&env)
    }

    fn decimals(env: Env) -> u32 {
        env.storage().instance().get(&Key::Decimals).unwrap_or(8)
    }

    fn resolution(env: Env) -> u32 {
        env.storage().instance().get(&Key::Resolution).unwrap_or(1)
    }

    fn price(env: Env, asset: Asset, _timestamp: u64) -> Option<PriceData> {
        env.storage().persistent().get(&Key::Price(asset))
    }

    fn prices(_env: Env, _asset: Asset, _records: u32) -> Option<Vec<PriceData>> {
        None
    }

    fn lastprice(env: Env, asset: Asset) -> Option<PriceData> {
        env.storage().persistent().get(&Key::Price(asset))
    }
}
