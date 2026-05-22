use super::*;

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{contract, contractimpl, Env};

#[derive(Clone)]
#[contracttype]
enum MockKey {
    Base,
    Decimals,
    Resolution,
    Assets,
    Price(Asset),
}

#[contract]
struct MockPriceFeed;

#[contractimpl]
impl MockPriceFeed {
    pub fn init(env: Env, base: Asset, decimals: u32, resolution: u32) {
        env.storage().instance().set(&MockKey::Base, &base);
        env.storage().instance().set(&MockKey::Decimals, &decimals);
        env.storage()
            .instance()
            .set(&MockKey::Resolution, &resolution);
        env.storage()
            .persistent()
            .set(&MockKey::Assets, &Vec::<Asset>::new(&env));
    }

    pub fn set_price(env: Env, asset: Asset, price: i128, timestamp: u64) {
        let mut assets = env
            .storage()
            .persistent()
            .get::<_, Vec<Asset>>(&MockKey::Assets)
            .unwrap_or_else(|| Vec::new(&env));
        if !assets.iter().any(|entry| entry == asset) {
            assets.push_back(asset.clone());
            env.storage().persistent().set(&MockKey::Assets, &assets);
        }
        env.storage()
            .persistent()
            .set(&MockKey::Price(asset), &PriceData { price, timestamp });
    }

    pub fn clear_price(env: Env, asset: Asset) {
        env.storage().persistent().remove(&MockKey::Price(asset));
    }
}

#[contractimpl]
impl PriceFeedTrait for MockPriceFeed {
    fn base(env: Env) -> Asset {
        env.storage().instance().get(&MockKey::Base).unwrap()
    }

    fn assets(env: Env) -> Vec<Asset> {
        env.storage()
            .persistent()
            .get(&MockKey::Assets)
            .unwrap_or_else(|| Vec::new(&env))
    }

    fn decimals(env: Env) -> u32 {
        env.storage().instance().get(&MockKey::Decimals).unwrap()
    }

    fn resolution(env: Env) -> u32 {
        env.storage().instance().get(&MockKey::Resolution).unwrap()
    }

    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData> {
        env.storage()
            .persistent()
            .get::<_, PriceData>(&MockKey::Price(asset))
            .filter(|price| price.timestamp == timestamp)
    }

    fn prices(env: Env, asset: Asset, _records: u32) -> Option<Vec<PriceData>> {
        let price = Self::lastprice(env.clone(), asset)?;
        let mut prices = Vec::new(&env);
        prices.push_back(price);
        Some(prices)
    }

    fn lastprice(env: Env, asset: Asset) -> Option<PriceData> {
        env.storage().persistent().get(&MockKey::Price(asset))
    }
}

fn register_mock_source(env: &Env, base: &Asset) -> (Address, MockPriceFeedClient<'static>) {
    let source_id = env.register(MockPriceFeed, ());
    let source = MockPriceFeedClient::new(env, &source_id);
    source.init(base, &8_u32, &1_u32);
    (source_id, source)
}

fn setup() -> (
    Env,
    SorobanProxyOracleClient<'static>,
    MockPriceFeedClient<'static>,
    Asset,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 100,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let (source_id, source) = register_mock_source(&env, &base);
    let proxy_id = env.register(SorobanProxyOracle, (&admin, &base, 8_u32, 1_u32));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);

    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: source_id,
        asset: asset.clone(),
    });
    proxy.set_proxy(
        &asset,
        &Some(ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        }),
    );

    (env, proxy, source, asset)
}

#[test]
fn refresh_updates_sep40_lastprice() {
    let (_env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);

    let result = proxy.refresh(&Vec::from_array(&proxy.env, [asset.clone()]));
    assert!(matches!(
        result.get(0).unwrap().1,
        RefreshStatus::Accepted(_)
    ));

    let price = proxy.lastprice(&asset).unwrap();
    assert_eq!(price.price, 5_000_000_000);
    assert_eq!(price.timestamp, 100);
}

#[test]
fn lastprice_fails_closed_when_cache_is_stale() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    env.ledger().set(LedgerInfo {
        timestamp: 131,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });

    assert_eq!(proxy.lastprice(&asset), None);
}

#[test]
fn manual_trip_blocks_refresh_and_cached_read() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.set_manual_trip(&asset, &true);

    let result = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        result.get(0).unwrap().1,
        RefreshStatus::Blocked(1)
    ));
    assert_eq!(proxy.lastprice(&asset), None);
}

#[test]
fn prices_returns_cached_history() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    env.ledger().set(LedgerInfo {
        timestamp: 101,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });
    source.set_price(&asset, &5_100_000_000_i128, &101_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    let prices = proxy.prices(&asset, &2).unwrap();
    assert_eq!(prices.len(), 2);
    assert_eq!(prices.get(0).unwrap().price, 5_000_000_000);
    assert_eq!(prices.get(1).unwrap().price, 5_100_000_000);
    assert_eq!(proxy.price(&asset, &100).unwrap().price, 5_000_000_000);
}

#[test]
fn same_timestamp_refresh_replaces_history_entry() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    source.set_price(&asset, &5_100_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    let prices = proxy.prices(&asset, &2).unwrap();
    assert_eq!(prices.len(), 1);
    assert_eq!(prices.get(0).unwrap().price, 5_100_000_000);
    assert_eq!(proxy.price(&asset, &100).unwrap().price, 5_100_000_000);
}

#[test]
fn failed_refresh_overwrites_accepted_cache_fail_closed() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(proxy.lastprice(&asset).is_some());

    source.clear_price(&asset);
    let result = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    assert!(matches!(
        result.get(0).unwrap().1,
        RefreshStatus::SourceUnavailable
    ));
    assert_eq!(proxy.lastprice(&asset), None);
    assert!(matches!(
        proxy.get_cached(&asset).unwrap().status,
        CachedStatus::ResolveFailed(5)
    ));
}

#[test]
fn refresh_rejects_source_with_wrong_base_asset() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 100,
        ..Default::default()
    });
    let governance = Address::generate(&env);
    let usd = Asset::Other(Symbol::new(&env, "USD"));
    let eur = Asset::Other(Symbol::new(&env, "EUR"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let (source_id, source) = register_mock_source(&env, &eur);
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &usd, 8_u32, 1_u32));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: source_id,
        asset: asset.clone(),
    });
    proxy.set_proxy(
        &asset,
        &Some(ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        }),
    );

    let result = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    assert!(matches!(
        result.get(0).unwrap().1,
        RefreshStatus::SourceUnavailable
    ));
    assert_eq!(proxy.lastprice(&asset), None);
}

#[test]
fn refresh_rejects_future_source_beyond_clock_drift() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &106_u64);

    let result = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    assert!(matches!(
        result.get(0).unwrap().1,
        RefreshStatus::ResolveFailed(1)
    ));
    assert_eq!(proxy.lastprice(&asset), None);
}

#[test]
fn set_proxy_rejects_unreachable_min_sources() {
    let env = Env::default();
    env.mock_all_auths();
    let governance = Address::generate(&env);
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &base, 8_u32, 1_u32));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: Address::generate(&env),
        asset: asset.clone(),
    });

    assert_eq!(
        proxy.try_set_proxy(
            &asset,
            &Some(ProxyConfig {
                sources: sources.clone(),
                min_sources: 0,
                max_age_secs: Some(30),
                max_clock_drift_secs: Some(5),
            }),
        ),
        Err(Ok(ContractError::InvalidInput))
    );
    assert_eq!(
        proxy.try_set_proxy(
            &asset,
            &Some(ProxyConfig {
                sources,
                min_sources: 2,
                max_age_secs: Some(30),
                max_clock_drift_secs: Some(5),
            }),
        ),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn prices_with_zero_records_returns_none() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    assert_eq!(proxy.prices(&asset, &0), None);
}

#[test]
fn direct_governed_mutation_requires_governance_auth() {
    let env = Env::default();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 100,
        ..Default::default()
    });
    let governance = Address::generate(&env);
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &base, 8_u32, 1_u32));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: Address::generate(&env),
        asset: asset.clone(),
    });

    let result = proxy.try_set_proxy(
        &asset,
        &Some(ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        }),
    );

    assert!(result.is_err());
}
