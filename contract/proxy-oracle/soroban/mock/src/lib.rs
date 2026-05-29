#![no_std]
#![allow(clippy::needless_pass_by_value)]

use soroban_sdk::{contract, contractimpl, contractmeta, contracttype, Address, Env, Symbol, Vec};

const TTL_THRESHOLD: u32 = 518_400;
const TTL_EXTEND_TO: u32 = 3_110_400;
const MAX_HISTORY_RECORDS: u32 = 32;

contractmeta!(key = "sep", val = "40");

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum Asset {
    Stellar(Address),
    Other(Symbol),
}

#[derive(Clone)]
#[contracttype]
enum DataKey {
    Base,
    Decimals,
    Resolution,
    Assets,
    LastPrice(Asset),
    History(Asset),
}

#[contract]
pub struct MockSep40Oracle;

#[contractimpl]
impl MockSep40Oracle {
    pub fn __constructor(env: Env, base: Asset, decimals: u32, resolution: u32) {
        extend_instance_ttl(&env);
        if resolution == 0 {
            panic!("resolution must be non-zero");
        }
        if decimals > 18 {
            panic!("decimals must be <= 18");
        }
        let storage = env.storage().instance();
        if storage.has(&DataKey::Base) {
            panic!("already initialized");
        }
        storage.set(&DataKey::Base, &base);
        storage.set(&DataKey::Decimals, &decimals);
        storage.set(&DataKey::Resolution, &resolution);
        storage.set(&DataKey::Assets, &Vec::<Asset>::new(&env));
    }

    pub fn set_price(env: Env, asset: Asset, price: i128, timestamp: u64) {
        extend_instance_ttl(&env);
        ensure_asset(&env, &asset);
        let price_data = PriceData { price, timestamp };
        env.storage()
            .persistent()
            .set(&DataKey::LastPrice(asset.clone()), &price_data);

        let mut history = env
            .storage()
            .persistent()
            .get::<_, Vec<PriceData>>(&DataKey::History(asset.clone()))
            .unwrap_or_else(|| Vec::new(&env));
        history.push_back(price_data);
        while history.len() > MAX_HISTORY_RECORDS {
            history.pop_front();
        }
        env.storage()
            .persistent()
            .set(&DataKey::History(asset), &history);
    }

    pub fn clear_price(env: Env, asset: Asset) {
        extend_instance_ttl(&env);
        env.storage()
            .persistent()
            .remove(&DataKey::LastPrice(asset.clone()));
        env.storage().persistent().remove(&DataKey::History(asset));
    }

    pub fn base(env: Env) -> Asset {
        extend_instance_ttl(&env);
        env.storage().instance().get(&DataKey::Base).unwrap()
    }

    pub fn assets(env: Env) -> Vec<Asset> {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::Assets)
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn decimals(env: Env) -> u32 {
        extend_instance_ttl(&env);
        env.storage().instance().get(&DataKey::Decimals).unwrap()
    }

    pub fn resolution(env: Env) -> u32 {
        extend_instance_ttl(&env);
        env.storage().instance().get(&DataKey::Resolution).unwrap()
    }

    pub fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData> {
        extend_instance_ttl(&env);
        let history = env
            .storage()
            .persistent()
            .get::<_, Vec<PriceData>>(&DataKey::History(asset))?;
        history
            .iter()
            .find(|price_data| price_data.timestamp == timestamp)
    }

    pub fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>> {
        extend_instance_ttl(&env);
        let history = env
            .storage()
            .persistent()
            .get::<_, Vec<PriceData>>(&DataKey::History(asset))?;
        let mut result = Vec::new(&env);
        if records == 0 {
            return Some(result);
        }
        let start = history.len().saturating_sub(records);
        for i in start..history.len() {
            if let Some(price_data) = history.get(i) {
                result.push_back(price_data);
            }
        }
        Some(result)
    }

    pub fn lastprice(env: Env, asset: Asset) -> Option<PriceData> {
        extend_instance_ttl(&env);
        env.storage().persistent().get(&DataKey::LastPrice(asset))
    }
}

fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
}

fn ensure_asset(env: &Env, asset: &Asset) {
    let mut assets = env
        .storage()
        .instance()
        .get::<_, Vec<Asset>>(&DataKey::Assets)
        .unwrap_or_else(|| Vec::new(env));
    if !assets.iter().any(|entry| entry == *asset) {
        assets.push_back(asset.clone());
        env.storage().instance().set(&DataKey::Assets, &assets);
    }
}

#[cfg(test)]
mod tests {
    use soroban_sdk::{symbol_short, vec, Env};

    use super::{Asset, MockSep40Oracle, MockSep40OracleClient};

    #[test]
    fn reports_latest_and_historical_prices() {
        let env = Env::default();
        let base = Asset::Other(symbol_short!("USD"));
        let asset = Asset::Other(symbol_short!("BTC"));
        let contract_id = env.register(MockSep40Oracle, (&base, 8_u32, 1_u32));
        let client = MockSep40OracleClient::new(&env, &contract_id);

        client.set_price(&asset, &5_000_000_000, &100);
        client.set_price(&asset, &5_100_000_000, &101);

        assert_eq!(client.base(), base);
        assert_eq!(client.decimals(), 8);
        assert_eq!(client.resolution(), 1);
        assert_eq!(client.assets(), vec![&env, asset.clone()]);
        assert_eq!(client.lastprice(&asset).unwrap().price, 5_100_000_000);
        assert_eq!(client.price(&asset, &100).unwrap().price, 5_000_000_000);
        assert_eq!(client.prices(&asset, &2).unwrap().len(), 2);
    }

    #[test]
    fn can_clear_prices() {
        let env = Env::default();
        let asset = Asset::Other(symbol_short!("BTC"));
        let contract_id = env.register(
            MockSep40Oracle,
            (&Asset::Other(symbol_short!("USD")), 8_u32, 1_u32),
        );
        let client = MockSep40OracleClient::new(&env, &contract_id);

        client.set_price(&asset, &5_000_000_000, &100);
        client.clear_price(&asset);

        assert_eq!(client.lastprice(&asset), None);
        assert_eq!(client.prices(&asset, &1), None);
    }
}
