#![allow(clippy::needless_pass_by_value)]
// Soroban host error messages are not stable strings; specifying `expected`
// would couple tests to internal diagnostic formatting.
#![allow(clippy::should_panic_without_expect)]

use super::*;

use alloc::vec;
use alloc::vec::Vec as StdVec;
use soroban_sdk::testutils::{Address as _, Events as _, Ledger, LedgerInfo};
use soroban_sdk::{contract, contractimpl, Bytes, Env, Event, Symbol};
use templar_primitives::Decimal;
use templar_proxy_oracle_soroban_common::{
    normalized_to_sep40, MAX_CLOCK_DRIFT_SECS, MAX_SOURCE_AGE_SECS,
};

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

    pub fn set_decimals(env: Env, decimals: u32) {
        env.storage().instance().set(&MockKey::Decimals, &decimals);
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

    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>> {
        let _ = records;
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

struct MockSources {
    clients: StdVec<MockPriceFeedClient<'static>>,
}

impl MockSources {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn set_price(&self, asset: &Asset, price: &i128, timestamp: &u64) {
        for client in &self.clients {
            client.set_price(asset, price, timestamp);
        }
    }

    fn clear_price(&self, asset: &Asset) {
        for client in &self.clients {
            client.clear_price(asset);
        }
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn set_decimals(&self, decimals: &u32) {
        for client in &self.clients {
            client.set_decimals(decimals);
        }
    }
}

fn register_mock_sources(env: &Env, base: &Asset) -> (Vec<Address>, MockSources) {
    let (first_id, first) = register_mock_source(env, base);
    let (second_id, second) = register_mock_source(env, base);
    let (third_id, third) = register_mock_source(env, base);
    (
        Vec::from_array(env, [first_id, second_id, third_id]),
        MockSources {
            clients: vec![first, second, third],
        },
    )
}

fn proxy_sources(env: &Env, base: &Asset, asset: &Asset) -> (MockSources, Vec<SourceConfig>) {
    let (source_ids, sources) = register_mock_sources(env, base);
    let mut config_sources = Vec::new(env);
    for source_id in source_ids.iter() {
        config_sources.push_back(SourceConfig {
            oracle: source_id,
            asset: asset.clone(),
        });
    }
    (sources, config_sources)
}

fn setup() -> (Env, SorobanProxyOracleClient<'static>, MockSources, Asset) {
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
    let (source, sources) = proxy_sources(&env, &base, &asset);
    let proxy_id = env.register(SorobanProxyOracle, (&admin, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);

    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources,
            min_sources: 3,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        },
    );

    (env, proxy, source, asset)
}

/// Helpers that mimic the pre-refactor SEP-40 surface (decimals=8) by reading
/// the new `aggregated_latest` API and scaling. Used so the bulk of the test
/// suite, which was written against the SEP-40 surface, doesn't need to be
/// rewritten — the surface is now adapter-side, but the underlying semantics
/// (cache, freshness, breakers) are unchanged.
const TEST_LEGACY_DECIMALS: u32 = 8;

fn legacy_lastprice(proxy: &SorobanProxyOracleClient, asset: &Asset) -> Option<PriceData> {
    proxy
        .aggregated_latest(asset)
        .and_then(|p| normalized_to_sep40(&p, TEST_LEGACY_DECIMALS).ok())
}

fn legacy_prices(
    env: &Env,
    proxy: &SorobanProxyOracleClient,
    asset: &Asset,
    records: u32,
) -> Option<Vec<PriceData>> {
    let history = proxy.aggregated_history(asset, &records)?;
    let mut out = Vec::new(env);
    for entry in history.iter() {
        out.push_back(normalized_to_sep40(&entry, TEST_LEGACY_DECIMALS).ok()?);
    }
    Some(out)
}

fn legacy_price(
    proxy: &SorobanProxyOracleClient,
    asset: &Asset,
    timestamp: u64,
) -> Option<PriceData> {
    let history = proxy.aggregated_history(asset, &MAX_HISTORY_RECORDS)?;
    for entry in history.iter().rev() {
        if entry.timestamp == timestamp {
            return normalized_to_sep40(&entry, TEST_LEGACY_DECIMALS).ok();
        }
    }
    None
}

fn contract_events(env: &Env, contract_id: &Address) -> StdVec<soroban_sdk::xdr::ContractEvent> {
    env.events()
        .all()
        .filter_by_contract(contract_id)
        .events()
        .to_vec()
}

fn set_ledger(env: &Env, timestamp: u64) {
    env.ledger().set(LedgerInfo {
        timestamp,
        protocol_version: 25,
        sequence_number: u32::try_from(timestamp).unwrap_or(u32::MAX),
        ..Default::default()
    });
}

fn stored_breakers(env: &Env, contract_id: &Address, asset: &Asset) -> CircuitBreakerSet {
    env.as_contract(contract_id, || {
        let bytes = env
            .storage()
            .persistent()
            .get::<_, Bytes>(&DataKey::Breakers(asset.clone()))
            .unwrap();
        postcard::from_bytes(&bytes.to_alloc_vec()).unwrap()
    })
}

fn assert_refresh_failure_event(env: &Env, proxy: &SorobanProxyOracleClient, _asset: &Asset) {
    let events = contract_events(env, &proxy.address);
    assert_eq!(events.len(), 1);
}

#[test]
fn parity_refresh_resolution_matrix_matches_near_baseline_semantics() {
    let (env, proxy, source, asset) = setup();

    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    let accepted = proxy.refresh(&asset.clone());
    assert!(matches!(accepted, RefreshStatus::Accepted(_)));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![RefreshSuccess {
            asset: asset.clone(),
            mantissa: 5_000_000_000,
            expo: -8,
            timestamp: 100,
        }
        .to_xdr(&env, &proxy.address)]
    );
    assert_eq!(
        legacy_lastprice(&proxy, &asset).unwrap().price,
        5_000_000_000
    );

    source.set_price(&asset, &5_100_000_000_i128, &69_u64);
    let stale = proxy.refresh(&asset.clone());
    assert!(matches!(stale, RefreshStatus::ResolveFailed(_)));
    assert_refresh_failure_event(&env, &proxy, &asset);
    assert!(legacy_lastprice(&proxy, &asset).is_none());
    assert!(matches!(
        proxy.get_cached(&asset).unwrap().status,
        CachedStatus::ResolveFailed(_)
    ));

    source.clear_price(&asset);
    let unavailable = proxy.refresh(&asset.clone());
    assert!(matches!(unavailable, RefreshStatus::SourceUnavailable));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![RefreshFailure {
            asset: asset.clone(),
            code: SOURCE_UNAVAILABLE_CODE,
        }
        .to_xdr(&env, &proxy.address)]
    );

    let base = Asset::Other(Symbol::new(&env, "USD"));
    let (second_source_id, second_source) = register_mock_source(&env, &base);
    let mut sources = proxy.get_proxy(&asset).unwrap().sources;
    sources.push_back(SourceConfig {
        oracle: second_source_id,
        asset: asset.clone(),
    });
    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources,
            min_sources: 4,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        },
    );
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    second_source.clear_price(&asset);
    let quorum = proxy.refresh(&asset.clone());
    assert!(matches!(quorum, RefreshStatus::ResolveFailed(_)));
    assert_refresh_failure_event(&env, &proxy, &asset);

    let eur = Asset::Other(Symbol::new(&env, "EUR"));
    let (wrong_base_source, wrong_base_sources) = proxy_sources(&env, &eur, &asset);
    wrong_base_source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources: wrong_base_sources,
            min_sources: 3,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        },
    );
    let base_mismatch = proxy.refresh(&asset.clone());
    assert!(matches!(base_mismatch, RefreshStatus::SourceUnavailable));
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
}

#[test]
fn cached_read_requires_fresh_cache_and_source_timestamps() {
    let accepted = |updated_at, timestamp| CachedProxyPrice {
        updated_at,
        status: CachedStatus::Accepted(NormalizedPrice {
            mantissa: 5_000_000_000,
            expo: -8,
            timestamp,
        }),
    };

    assert!(cached_accepted_no_older_than(&accepted(70, 100), 30, 100).is_some());
    assert!(cached_accepted_no_older_than(&accepted(70, 100), 30, 101).is_none());
    assert!(cached_accepted_no_older_than(&accepted(100, 70), 30, 100).is_some());
    assert!(cached_accepted_no_older_than(&accepted(100, 70), 30, 101).is_none());
}

#[test]
fn breaker_block_precedes_source_outage() {
    let (env, proxy, source, asset) = setup();
    source.clear_price(&asset);
    assert!(matches!(
        proxy.refresh(&asset),
        RefreshStatus::SourceUnavailable
    ));

    proxy.set_manual_trip(&asset, &true, &None);
    assert!(matches!(proxy.refresh(&asset), RefreshStatus::Blocked(1)));
    proxy.set_manual_trip(&asset, &false, &None);

    source.set_price(&asset, &100_i128, &100_u64);
    proxy.refresh(&asset);
    proxy.configure_breakers(&asset, &0, &8);
    proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );
    source.set_price(&asset, &100_i128, &100_u64);
    assert!(matches!(proxy.refresh(&asset), RefreshStatus::Accepted(_)));
    set_ledger(&env, 101);
    source.set_price(&asset, &200_i128, &101_u64);
    assert!(matches!(proxy.refresh(&asset), RefreshStatus::Blocked(2)));

    source.clear_price(&asset);
    assert!(matches!(proxy.refresh(&asset), RefreshStatus::Blocked(2)));
}

#[test]
fn parity_manual_trip_blocks_reads_refresh_and_maps_event_fields() {
    let (env, proxy, source, asset) = setup();
    let metadata = Bytes::from_array(&env, &[1_u8, 2, 3]);

    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    assert!(legacy_lastprice(&proxy, &asset).is_some());

    proxy.set_manual_trip(&asset, &true, &Some(metadata.clone()));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![ManualTripSet {
            asset: asset.clone(),
            is_manually_tripped: true,
            metadata: Some(metadata),
        }
        .to_xdr(&env, &proxy.address)]
    );
    assert!(
        proxy
            .get_breaker_set_view(&asset)
            .unwrap()
            .is_manually_tripped
    );
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
    assert_eq!(
        legacy_prices(&env, &proxy, &asset, MAX_HISTORY_RECORDS),
        None
    );
    assert_eq!(legacy_price(&proxy, &asset, 100), None);

    source.set_price(&asset, &5_100_000_000_i128, &100_u64);
    let blocked = proxy.refresh(&asset.clone());
    assert!(matches!(blocked, RefreshStatus::Blocked(1)));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CacheBlocked {
            asset: asset.clone(),
            reason_code: 1,
        }
        .to_xdr(&env, &proxy.address)]
    );
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
}

#[test]
fn parity_breaker_trip_observed_history_rearm_and_events_match_near_matrix() {
    let (env, proxy, source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &8);
    let breaker_id = proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );

    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());

    set_ledger(&env, 101);
    source.set_price(&asset, &10_000_000_000_i128, &101_u64);
    let tripped = proxy.refresh(&asset.clone());
    assert!(matches!(tripped, RefreshStatus::Blocked(2)));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![
            CircuitBreakerTripped {
                asset: asset.clone(),
                breaker_id,
                tripped_at_secs: 101,
                price: 10_000_000_000,
                expo: -8,
                publish_timestamp_secs: 101,
                is_enforced: true,
            }
            .to_xdr(&env, &proxy.address),
            CacheBlocked {
                asset: asset.clone(),
                reason_code: 2,
            }
            .to_xdr(&env, &proxy.address),
        ]
    );
    assert_eq!(legacy_lastprice(&proxy, &asset), None);

    set_ledger(&env, 102);
    source.set_price(&asset, &10_500_000_000_i128, &102_u64);
    let still_blocked = proxy.refresh(&asset.clone());
    assert!(matches!(still_blocked, RefreshStatus::Blocked(2)));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CacheBlocked {
            asset: asset.clone(),
            reason_code: 2,
        }
        .to_xdr(&env, &proxy.address)]
    );

    let breakers_before_rearm = stored_breakers(&env, &proxy.address, &asset);
    assert_eq!(breakers_before_rearm.accepted_history().len(), 1);
    assert_eq!(breakers_before_rearm.observed_history().len(), 3);

    proxy.rearm(
        &asset,
        &breaker_id,
        &SorobanRearmConfig {
            arming_delay_secs: 3,
        },
    );
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CircuitBreakerRearmed {
            asset: asset.clone(),
            breaker_id,
            armed_at_secs: 105,
        }
        .to_xdr(&env, &proxy.address)]
    );
    let breakers_after_rearm = stored_breakers(&env, &proxy.address, &asset);
    assert_eq!(breakers_after_rearm.accepted_history().len(), 1);
    assert_eq!(breakers_after_rearm.observed_history().len(), 3);
    assert!(proxy.get_cached(&asset).is_none());
}

#[test]
fn hal_21_rearm_delay_arms_at_execution_time() {
    let (env, proxy, source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &2);
    let breaker_id = proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );
    source.set_price(&asset, &100_i128, &100_u64);
    proxy.refresh(&asset.clone());
    proxy.rearm(
        &asset,
        &breaker_id,
        &SorobanRearmConfig {
            arming_delay_secs: 3,
        },
    );

    set_ledger(&env, 102);
    source.set_price(&asset, &200_i128, &102_u64);
    assert!(matches!(proxy.refresh(&asset), RefreshStatus::Accepted(_)));

    set_ledger(&env, 103);
    source.set_price(&asset, &400_i128, &103_u64);
    assert!(matches!(proxy.refresh(&asset), RefreshStatus::Blocked(2)));
}
#[test]
fn parity_config_update_cache_invalidation_and_unauthorized_mutation() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    assert!(proxy.get_cached(&asset).is_some());

    let configured = proxy.get_proxy(&asset).unwrap();
    proxy.set_proxy(&asset, &configured);
    assert!(proxy.get_cached(&asset).is_some());
    assert!(legacy_lastprice(&proxy, &asset).is_some());

    let mut changed = configured;
    changed.max_age_secs = Some(31);
    proxy.set_proxy(&asset, &changed);
    assert!(proxy.get_cached(&asset).is_none());
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
    assert!(matches!(proxy.refresh(&asset), RefreshStatus::Accepted(_)));
    proxy.configure_breakers(&asset, &0, &2);
    assert!(matches!(proxy.refresh(&asset), RefreshStatus::Accepted(_)));
    proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::CumulativeChange(SorobanCumulativeChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );
    assert_eq!(
        stored_breakers(&env, &proxy.address, &asset).breaker_count(),
        1
    );

    let mut rotated = proxy.get_proxy(&asset).unwrap();
    let first_source = rotated.sources.get(0).unwrap();
    rotated.sources.set(
        0,
        SourceConfig {
            oracle: first_source.oracle,
            asset: Asset::Other(Symbol::new(&env, "BTC2")),
        },
    );
    proxy.set_proxy(&asset, &rotated);
    let breakers = stored_breakers(&env, &proxy.address, &asset);
    assert!(breakers.breakers().is_empty());
    assert_eq!(breakers.accepted_history().capacity(), 0);
    assert_eq!(breakers.observed_history().capacity(), 0);
    assert_eq!(
        legacy_prices(&env, &proxy, &asset, MAX_HISTORY_RECORDS),
        None
    );

    let unauth_env = Env::default();
    unauth_env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 100,
        ..Default::default()
    });
    let governance = Address::generate(&unauth_env);
    let base = Asset::Other(Symbol::new(&unauth_env, "USD"));
    let unauthorized_asset = Asset::Other(Symbol::new(&unauth_env, "BTC"));
    let proxy_id = unauth_env.register(SorobanProxyOracle, (&governance, &base));
    let unauth_proxy = SorobanProxyOracleClient::new(&unauth_env, &proxy_id);
    let mut sources = Vec::new(&unauth_env);
    sources.push_back(SourceConfig {
        oracle: Address::generate(&unauth_env),
        asset: unauthorized_asset.clone(),
    });

    assert!(unauth_proxy
        .try_set_proxy(
            &unauthorized_asset,
            &ProxyConfig {
                sources,
                min_sources: 1,
                max_age_secs: Some(30),
                max_clock_drift_secs: Some(5),
            },
        )
        .is_err());
}

#[test]
fn event_refresh_success_failure_and_cache_blocked_topics_payloads_are_exact() {
    let (env, proxy, source, asset) = setup();

    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![RefreshSuccess {
            asset: asset.clone(),
            mantissa: 5_000_000_000,
            expo: -8,
            timestamp: 100,
        }
        .to_xdr(&env, &proxy.address)]
    );

    source.clear_price(&asset);
    proxy.refresh(&asset.clone());
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![RefreshFailure {
            asset: asset.clone(),
            code: 5,
        }
        .to_xdr(&env, &proxy.address)]
    );

    proxy.set_manual_trip(&asset, &true, &None);
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![ManualTripSet {
            asset: asset.clone(),
            is_manually_tripped: true,
            metadata: None,
        }
        .to_xdr(&env, &proxy.address)]
    );

    source.set_price(&asset, &5_100_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CacheBlocked {
            asset: asset.clone(),
            reason_code: 1,
        }
        .to_xdr(&env, &proxy.address)]
    );
}

#[test]
fn event_proxy_set_topics_payload_are_exact() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 100,
        ..Default::default()
    });
    let governance = Address::generate(&env);
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let (_source, sources) = proxy_sources(&env, &base, &asset);
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);

    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources,
            min_sources: 3,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        },
    );

    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![ProxySet {
            asset,
            source_count: 3,
            min_sources: 3,
        }
        .to_xdr(&env, &proxy.address)]
    );
}

#[test]
fn event_circuit_breaker_tripped_topics_payload_are_exact() {
    let (env, proxy, source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &8);
    let breaker_id = proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );

    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());

    env.ledger().set(LedgerInfo {
        timestamp: 101,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });
    source.set_price(&asset, &10_000_000_000_i128, &101_u64);
    let result = proxy.refresh(&asset.clone());

    assert!(matches!(result, RefreshStatus::Blocked(2)));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![
            CircuitBreakerTripped {
                asset: asset.clone(),
                breaker_id,
                tripped_at_secs: 101,
                price: 10_000_000_000,
                expo: -8,
                publish_timestamp_secs: 101,
                is_enforced: true,
            }
            .to_xdr(&env, &proxy.address),
            CacheBlocked {
                asset,
                reason_code: 2,
            }
            .to_xdr(&env, &proxy.address),
        ]
    );
}

#[test]
fn event_proxy_breaker_governance_and_ttl_topics_payloads_are_exact() {
    let (env, proxy, source, asset) = setup();
    let old_governance = proxy.get_owner().unwrap();
    let new_governance = Address::generate(&env);

    proxy.configure_breakers(&asset, &2, &8);
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CircuitBreakerConfigSet {
            asset: asset.clone(),
            sample_interval_secs: 2,
            history_len: 8,
        }
        .to_xdr(&env, &proxy.address)]
    );

    let breaker_id = proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CircuitBreakerAdded {
            asset: asset.clone(),
            breaker_id,
            breaker_kind: 1,
        }
        .to_xdr(&env, &proxy.address)]
    );

    proxy.set_enforced(
        &asset,
        &breaker_id,
        &SorobanSetEnforcedConfig { is_enforced: false },
    );
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CircuitBreakerEnforcementSet {
            asset: asset.clone(),
            breaker_id,
            is_enforced: false,
        }
        .to_xdr(&env, &proxy.address)]
    );

    proxy.rearm(
        &asset,
        &breaker_id,
        &SorobanRearmConfig {
            arming_delay_secs: 0,
        },
    );
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CircuitBreakerRearmed {
            asset: asset.clone(),
            breaker_id,
            armed_at_secs: 100,
        }
        .to_xdr(&env, &proxy.address)]
    );

    proxy.remove_breaker(&asset, &breaker_id);
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CircuitBreakerRemoved {
            asset: asset.clone(),
            breaker_id,
        }
        .to_xdr(&env, &proxy.address)]
    );

    // Ownership transfer is delegated to `stellar_access::ownable`, which
    // emits its own events. We don't assert exact event payloads here —
    // those are the library's responsibility — but we verify the owner
    // field flips after the two-step transfer completes. `contract_events`
    // is filtered by sequence in subsequent steps, so the ownership events
    // don't leak into later assertions.
    let _ = old_governance;
    let live_until_ledger = env.ledger().max_live_until_ledger();
    proxy.transfer_ownership(&new_governance, &live_until_ledger);
    proxy.accept_ownership();
    assert_eq!(proxy.get_owner(), Some(new_governance.clone()));

    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    proxy.extend_ttl(&asset);
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![TtlExtended {
            asset: asset.clone()
        }
        .to_xdr(&env, &proxy.address)]
    );

    proxy.remove_proxy(&asset);
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![ProxyRemoved { asset }.to_xdr(&env, &proxy.address)]
    );
}

#[test]
fn refresh_updates_sep40_lastprice() {
    let (_env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);

    let result = proxy.refresh(&asset.clone());
    assert!(matches!(result, RefreshStatus::Accepted(_)));

    let price = legacy_lastprice(&proxy, &asset).unwrap();
    assert_eq!(price.price, 5_000_000_000);
    assert_eq!(price.timestamp, 100);
}

#[test]
fn lastprice_fails_closed_when_cache_is_stale() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());

    env.ledger().set(LedgerInfo {
        timestamp: 131,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });

    assert_eq!(legacy_lastprice(&proxy, &asset), None);
}

#[test]
fn manual_trip_blocks_refresh_and_cached_read() {
    let (_env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.set_manual_trip(&asset, &true, &None);

    let result = proxy.refresh(&asset.clone());
    assert!(matches!(result, RefreshStatus::Blocked(1)));
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
}

#[test]
fn hal_31_idempotent_breaker_mutations_preserve_cache_storage_and_events() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.configure_breakers(&asset, &0, &2);
    let breaker_id = proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );
    proxy.refresh(&asset.clone());

    let cached = legacy_lastprice(&proxy, &asset);
    let breakers = stored_breakers(&env, &proxy.address, &asset);

    proxy.configure_breakers(&asset, &0, &2);
    assert!(contract_events(&env, &proxy.address).is_empty());
    assert_eq!(legacy_lastprice(&proxy, &asset), cached);
    assert_eq!(stored_breakers(&env, &proxy.address, &asset), breakers);

    proxy.set_enforced(
        &asset,
        &breaker_id,
        &SorobanSetEnforcedConfig { is_enforced: true },
    );
    assert!(contract_events(&env, &proxy.address).is_empty());

    proxy.set_manual_trip(&asset, &false, &None);
    assert!(contract_events(&env, &proxy.address).is_empty());
    assert_eq!(legacy_lastprice(&proxy, &asset), cached);
    assert_eq!(stored_breakers(&env, &proxy.address, &asset), breakers);
}

#[test]
fn manual_trip_role_authorized_trip_and_untrip_are_separate() {
    let (_env, proxy, _source, asset) = setup();

    proxy.set_manual_trip(&asset, &true, &None);
    assert!(
        proxy
            .get_breaker_set_view(&asset)
            .unwrap()
            .is_manually_tripped
    );

    proxy.set_manual_trip(&asset, &false, &None);
    assert!(
        !proxy
            .get_breaker_set_view(&asset)
            .unwrap()
            .is_manually_tripped
    );
}

#[test]
fn manual_trip_metadata_accepts_1024_and_rejects_1025_bytes() {
    let (env, proxy, _source, asset) = setup();

    let metadata_1024 = Bytes::from_array(&env, &[7_u8; MAX_MANUAL_TRIP_METADATA_LEN]);
    proxy.set_manual_trip(&asset, &true, &Some(metadata_1024));
    assert!(
        proxy
            .get_breaker_set_view(&asset)
            .unwrap()
            .is_manually_tripped
    );

    let metadata_1025 = Bytes::from_array(&env, &[8_u8; MAX_MANUAL_TRIP_METADATA_LEN + 1]);
    assert_eq!(
        proxy.try_set_manual_trip(&asset, &false, &Some(metadata_1025)),
        Err(Ok(ContractError::InvalidInput))
    );
    assert!(
        proxy
            .get_breaker_set_view(&asset)
            .unwrap()
            .is_manually_tripped
    );
}

#[test]
fn manual_trip_role_metadata_event_payload_is_bounded_and_not_stored() {
    let (env, proxy, _source, asset) = setup();
    let metadata = Bytes::from_array(&env, &[1_u8, 2, 3]);

    proxy.set_manual_trip(&asset, &true, &Some(metadata.clone()));

    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![ManualTripSet {
            asset,
            is_manually_tripped: true,
            metadata: Some(metadata),
        }
        .to_xdr(&env, &proxy.address)]
    );
    assert!(proxy
        .get_breaker_set_view(&Asset::Other(Symbol::new(&env, "BTC")))
        .is_some());
}

#[test]
fn prices_returns_cached_history() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    env.ledger().set(LedgerInfo {
        timestamp: 101,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });
    source.set_price(&asset, &5_100_000_000_i128, &101_u64);
    proxy.refresh(&asset.clone());

    let prices = legacy_prices(&env, &proxy, &asset, 2).unwrap();
    assert_eq!(prices.len(), 2);
    assert_eq!(prices.get(0).unwrap().price, 5_000_000_000);
    assert_eq!(prices.get(1).unwrap().price, 5_100_000_000);
    assert_eq!(
        legacy_price(&proxy, &asset, 100).unwrap().price,
        5_000_000_000
    );
}

#[test]
fn one_manipulated_source_cannot_move_the_median() {
    let (_env, proxy, source, asset) = setup();
    source.clients[0].set_price(&asset, &5_000_000_000_i128, &100_u64);
    source.clients[1].set_price(&asset, &5_100_000_000_i128, &100_u64);
    source.clients[2].set_price(&asset, &50_000_000_000_i128, &100_u64);

    assert!(matches!(
        proxy.refresh(&asset.clone()),
        RefreshStatus::Accepted(price) if price.mantissa == 5_100_000_000
    ));
    assert_eq!(
        legacy_lastprice(&proxy, &asset).unwrap().price,
        5_100_000_000
    );
}

#[test]
fn same_timestamp_refresh_preserves_served_history_price() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    assert!(matches!(
        proxy.refresh(&asset.clone()),
        RefreshStatus::Accepted(_)
    ));
    source.set_price(&asset, &5_100_000_000_i128, &100_u64);
    assert!(matches!(
        proxy.refresh(&asset.clone()),
        RefreshStatus::Accepted(_)
    ));

    let prices = legacy_prices(&env, &proxy, &asset, 2).unwrap();
    assert_eq!(prices.len(), 1);
    assert_eq!(prices.get(0).unwrap().price, 5_000_000_000);
    assert_eq!(
        legacy_price(&proxy, &asset, 100).unwrap().price,
        5_000_000_000
    );
    assert_eq!(
        legacy_lastprice(&proxy, &asset).unwrap().price,
        5_000_000_000
    );
}

#[test]
fn equal_publish_timestamps_do_not_pad_accepted_breaker_history() {
    let (env, proxy, source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &4);
    proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::MonotonicRun(SorobanMonotonicRunConfig {
            max_streak: 3,
            min_relative_step_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset);

    for _ in 0..4 {
        proxy.refresh(&asset);
    }

    let breakers = stored_breakers(&env, &proxy.address, &asset);
    assert_eq!(breakers.accepted_history().len(), 1);
    assert_eq!(breakers.observed_history().len(), 1);
}

#[test]
fn regressing_median_timestamp_preserves_served_history_price() {
    let (env, proxy, source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &2);
    proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE),
        }),
    );
    set_ledger(&env, 102);
    source.clients[0].set_price(&asset, &4_000_000_000_i128, &101_u64);
    source.clients[1].set_price(&asset, &5_000_000_000_i128, &102_u64);
    source.clients[2].set_price(&asset, &6_000_000_000_i128, &100_u64);
    assert!(matches!(
        proxy.refresh(&asset.clone()),
        RefreshStatus::Accepted(_)
    ));
    let _ = contract_events(&env, &proxy.address);
    let breakers = stored_breakers(&env, &proxy.address, &asset);

    source.clients[0].set_price(&asset, &1_000_000_000_i128, &101_u64);
    source.clients[1].set_price(&asset, &2_000_000_000_i128, &99_u64);
    source.clients[2].set_price(&asset, &3_000_000_000_i128, &100_u64);
    assert!(matches!(
        proxy.refresh(&asset.clone()),
        RefreshStatus::Accepted(_)
    ));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![
            RefreshEvaluated {
                asset: asset.clone(),
                mantissa: 2_000_000_000,
                expo: -8,
                timestamp: 99,
            }
            .to_xdr(&env, &proxy.address),
            RefreshSuccess {
                asset: asset.clone(),
                mantissa: 5_000_000_000,
                expo: -8,
                timestamp: 102,
            }
            .to_xdr(&env, &proxy.address),
        ]
    );

    let prices = legacy_prices(&env, &proxy, &asset, 2).unwrap();
    assert_eq!(prices.len(), 1);
    assert_eq!(prices.get(0).unwrap().price, 5_000_000_000);
    assert!(legacy_price(&proxy, &asset, 99).is_none());
    let latest = legacy_lastprice(&proxy, &asset).unwrap();
    assert_eq!(latest.price, 5_000_000_000);
    assert_eq!(latest.timestamp, 102);
    assert_eq!(stored_breakers(&env, &proxy.address, &asset), breakers);
}

#[test]
fn regressing_median_timestamp_trips_breakers() {
    let (env, proxy, source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &2);
    proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );
    set_ledger(&env, 102);
    source.clients[0].set_price(&asset, &4_000_000_000_i128, &101_u64);
    source.clients[1].set_price(&asset, &5_000_000_000_i128, &102_u64);
    source.clients[2].set_price(&asset, &6_000_000_000_i128, &100_u64);
    assert!(matches!(
        proxy.refresh(&asset.clone()),
        RefreshStatus::Accepted(_)
    ));

    source.clients[0].set_price(&asset, &1_000_000_000_i128, &101_u64);
    source.clients[1].set_price(&asset, &2_000_000_000_i128, &99_u64);
    source.clients[2].set_price(&asset, &3_000_000_000_i128, &100_u64);
    assert!(matches!(
        proxy.refresh(&asset.clone()),
        RefreshStatus::Blocked(2)
    ));

    assert!(legacy_lastprice(&proxy, &asset).is_none());
    assert!(stored_breakers(&env, &proxy.address, &asset).is_blocking());
}

#[test]
fn source_outage_replaces_accepted_cache() {
    let (_env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    assert!(legacy_lastprice(&proxy, &asset).is_some());

    source.clear_price(&asset);
    let result = proxy.refresh(&asset.clone());

    assert!(matches!(result, RefreshStatus::SourceUnavailable));
    assert!(legacy_lastprice(&proxy, &asset).is_none());
    assert!(matches!(
        proxy.get_cached(&asset).unwrap().status,
        CachedStatus::ResolveFailed(SOURCE_UNAVAILABLE_CODE)
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
    let (source, sources) = proxy_sources(&env, &eur, &asset);
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &usd));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources,
            min_sources: 3,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        },
    );

    let result = proxy.refresh(&asset.clone());

    assert!(matches!(result, RefreshStatus::SourceUnavailable));
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
}

#[test]
fn refresh_rejects_future_source_beyond_clock_drift() {
    let (_env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &106_u64);

    let result = proxy.refresh(&asset.clone());

    assert!(matches!(result, RefreshStatus::ResolveFailed(1)));
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
}

#[test]
fn set_proxy_rejects_unreachable_min_sources() {
    let env = Env::default();
    env.mock_all_auths();
    let governance = Address::generate(&env);
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    for _ in 0..3 {
        sources.push_back(SourceConfig {
            oracle: Address::generate(&env),
            asset: asset.clone(),
        });
    }

    assert_eq!(
        proxy.try_set_proxy(
            &asset,
            &ProxyConfig {
                sources: sources.clone(),
                min_sources: 0,
                max_age_secs: Some(30),
                max_clock_drift_secs: Some(5),
            },
        ),
        Err(Ok(ContractError::InvalidInput))
    );
    assert_eq!(
        proxy.try_set_proxy(
            &asset,
            &ProxyConfig {
                sources,
                min_sources: 4,
                max_age_secs: Some(30),
                max_clock_drift_secs: Some(5),
            },
        ),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn prices_with_zero_records_returns_none() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());

    assert_eq!(legacy_prices(&env, &proxy, &asset, 0), None);
}

#[test]
fn invalid_config_duplicate_source_oracle_asset_pair() {
    let env = Env::default();
    env.mock_all_auths();
    let governance = Address::generate(&env);
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let (source_ids, _sources) = register_mock_sources(&env, &base);
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    for source_id in [
        source_ids.get(0).unwrap(),
        source_ids.get(0).unwrap(),
        source_ids.get(2).unwrap(),
    ] {
        sources.push_back(SourceConfig {
            oracle: source_id,
            asset: asset.clone(),
        });
    }

    assert_eq!(
        proxy.try_set_proxy(
            &asset,
            &ProxyConfig {
                sources,
                min_sources: 3,
                max_age_secs: Some(30),
                max_clock_drift_secs: Some(5),
            },
        ),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn invalid_config_duplicate_oracle_is_rejected_even_for_distinct_assets() {
    let env = Env::default();
    env.mock_all_auths();
    let governance = Address::generate(&env);
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let (source_ids, _sources) = register_mock_sources(&env, &base);
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    for (oracle, source_asset) in [
        (
            source_ids.get(0).unwrap(),
            Asset::Other(Symbol::new(&env, "BTC")),
        ),
        (
            source_ids.get(0).unwrap(),
            Asset::Other(Symbol::new(&env, "ETH")),
        ),
        (
            source_ids.get(1).unwrap(),
            Asset::Other(Symbol::new(&env, "BTC")),
        ),
    ] {
        sources.push_back(SourceConfig {
            oracle,
            asset: source_asset,
        });
    }

    assert_eq!(
        proxy.try_set_proxy(
            &asset,
            &ProxyConfig {
                sources,
                min_sources: 3,
                max_age_secs: Some(30),
                max_clock_drift_secs: Some(5),
            },
        ),
        Err(Ok(ContractError::InvalidInput))
    );
}

/// Register a bare proxy and assert `set_proxy` with `num_sources` sources and
/// `min_sources` quorum is rejected with `expected`.
fn assert_set_proxy_rejected(num_sources: u32, min_sources: u32, expected: ContractError) {
    let env = Env::default();
    env.mock_all_auths();
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let proxy_id = env.register(SorobanProxyOracle, (&Address::generate(&env), &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    for _ in 0..num_sources {
        sources.push_back(SourceConfig {
            oracle: Address::generate(&env),
            asset: asset.clone(),
        });
    }
    assert_eq!(
        proxy.try_set_proxy(
            &asset,
            &ProxyConfig {
                sources,
                min_sources,
                max_age_secs: Some(30),
                max_clock_drift_secs: Some(5),
            },
        ),
        Err(Ok(expected))
    );
}

#[test]
fn invalid_config_freshness_bounds_are_capped() {
    let (_env, proxy, _source, asset) = setup();
    let mut config = proxy.get_proxy(&asset).unwrap();
    config.max_age_secs = Some(MAX_SOURCE_AGE_SECS + 1);
    assert_eq!(
        proxy.try_set_proxy(&asset, &config),
        Err(Ok(ContractError::InvalidInput))
    );

    config.max_age_secs = Some(MAX_SOURCE_AGE_SECS);
    config.max_clock_drift_secs = Some(MAX_CLOCK_DRIFT_SECS + 1);
    assert_eq!(
        proxy.try_set_proxy(&asset, &config),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn invalid_config_too_few_sources() {
    for num_sources in 0..3 {
        assert_set_proxy_rejected(
            num_sources,
            num_sources.max(1),
            ContractError::TooFewSources,
        );
    }
}

#[test]
fn invalid_proxy_config_is_atomic_and_emits_nothing() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    let configured = proxy.get_proxy(&asset).unwrap();
    let cached = legacy_lastprice(&proxy, &asset);
    let events = contract_events(&env, &proxy.address);
    let mut invalid = configured.clone();
    invalid.max_clock_drift_secs = None;

    assert_eq!(
        proxy.try_set_proxy(&asset, &invalid),
        Err(Ok(ContractError::InvalidInput))
    );
    assert_eq!(proxy.get_proxy(&asset), Some(configured));
    assert_eq!(legacy_lastprice(&proxy, &asset), cached);
    assert_eq!(contract_events(&env, &proxy.address), events);
}

#[test]
fn proxy_configuration_rejects_sources_with_too_many_decimals() {
    let (_env, proxy, source, asset) = setup();
    source.set_decimals(&19);
    let config = proxy.get_proxy(&asset).unwrap();

    assert_eq!(
        proxy.try_set_proxy(&asset, &config),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn registry_cap_allows_reconfiguration_and_slot_reuse() {
    let (env, proxy, _source, initial_asset) = setup();
    let template = proxy.get_proxy(&initial_asset).unwrap();
    let additional_config = |asset: &Asset| {
        let mut sources = Vec::new(&env);
        for source in template.sources.iter() {
            sources.push_back(SourceConfig {
                oracle: source.oracle,
                asset: asset.clone(),
            });
        }
        ProxyConfig {
            sources,
            min_sources: template.min_sources,
            max_age_secs: template.max_age_secs,
            max_clock_drift_secs: template.max_clock_drift_secs,
        }
    };

    for _ in 1..MAX_REGISTERED_ASSETS {
        let asset = Asset::Stellar(Address::generate(&env));
        proxy.set_proxy(&asset, &additional_config(&asset));
    }
    assert_eq!(proxy.registered_assets().len(), MAX_REGISTERED_ASSETS);

    proxy.set_proxy(&initial_asset, &template);
    assert_eq!(proxy.registered_assets().len(), MAX_REGISTERED_ASSETS);

    let replacement = Asset::Stellar(Address::generate(&env));
    assert_eq!(
        proxy.try_set_proxy(&replacement, &additional_config(&replacement)),
        Err(Ok(ContractError::TooManyAssets))
    );
    proxy.remove_proxy(&initial_asset);
    proxy.set_proxy(&replacement, &additional_config(&replacement));
    assert_eq!(proxy.registered_assets().len(), MAX_REGISTERED_ASSETS);
}

#[test]
fn malformed_registry_and_missing_breakers_fail_closed() {
    let (env, proxy, _source, asset) = setup();
    env.as_contract(&proxy.address, || {
        env.storage().persistent().set(
            &DataKey::Assets,
            &Vec::from_array(&env, [asset.clone(), asset.clone()]),
        );
    });
    assert_eq!(
        proxy.try_registered_assets(),
        Err(Ok(ContractError::StorageError))
    );

    let (env, proxy, _source, _asset) = setup();
    let mut oversized = Vec::new(&env);
    for _ in 0..=MAX_REGISTERED_ASSETS {
        oversized.push_back(Asset::Stellar(Address::generate(&env)));
    }
    env.as_contract(&proxy.address, || {
        env.storage().persistent().set(&DataKey::Assets, &oversized);
    });
    assert_eq!(
        proxy.try_registered_assets(),
        Err(Ok(ContractError::StorageError))
    );

    let (env, proxy, _source, asset) = setup();
    env.as_contract(&proxy.address, || {
        env.storage()
            .persistent()
            .remove(&DataKey::Breakers(asset.clone()));
    });
    assert_eq!(
        proxy.try_add_breaker(
            &asset,
            &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
                max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
            }),
        ),
        Err(Ok(ContractError::StorageError))
    );

    let (env, proxy, _source, asset) = setup();
    env.as_contract(&proxy.address, || {
        env.storage().persistent().set(
            &DataKey::Breakers(asset.clone()),
            &Bytes::from_array(&env, &[0xff]),
        );
    });
    assert_eq!(
        proxy.try_add_breaker(
            &asset,
            &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
                max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
            }),
        ),
        Err(Ok(ContractError::StorageError))
    );
}

#[test]
fn hal_07_rejects_threshold_that_postcard_quantizes_to_zero() {
    let (env, proxy, _source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &2);
    let threshold =
        SorobanDecimal::from_decimal(&env, Decimal::from_repr([1, 0, 0, 0, 0, 0, 0, 0]));

    assert_eq!(
        proxy.try_add_breaker(
            &asset,
            &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
                max_relative_change: threshold,
            }),
        ),
        Err(Ok(ContractError::StorageError))
    );
    assert!(stored_breakers(&env, &proxy.address, &asset)
        .breakers()
        .is_empty());
}

#[test]
fn hal_13_oversized_registry_can_be_shrunk() {
    let (env, proxy, _source, asset) = setup();
    let mut assets = Vec::new(&env);
    assets.push_back(asset.clone());
    for _ in 0..MAX_REGISTERED_ASSETS {
        assets.push_back(Asset::Stellar(Address::generate(&env)));
    }
    env.as_contract(&proxy.address, || {
        env.storage().persistent().set(&DataKey::Assets, &assets);
    });

    proxy.remove_proxy(&asset);

    assert_eq!(proxy.registered_assets().len(), MAX_REGISTERED_ASSETS);
}

#[test]
fn invalid_config_quorum_zero() {
    assert_set_proxy_rejected(3, 0, ContractError::InvalidInput);
}

#[test]
fn invalid_config_quorum_above_source_count() {
    assert_set_proxy_rejected(3, 4, ContractError::InvalidInput);
}

#[test]
fn invalid_config_too_many_sources() {
    assert_set_proxy_rejected(17, 3, ContractError::TooManySources);
}

#[test]
fn invalid_config_max_history_above_limit() {
    let (_env, proxy, _source, asset) = setup();

    assert_eq!(
        proxy.try_configure_breakers(&asset, &0, &33),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn rearm_rejects_seconds_and_nanosecond_deadline_overflow() {
    let (env, proxy, _source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &2);
    let breaker_id = proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );
    set_ledger(&env, u64::MAX);

    assert_eq!(
        proxy.try_rearm(
            &asset,
            &breaker_id,
            &SorobanRearmConfig {
                arming_delay_secs: 1,
            },
        ),
        Err(Ok(ContractError::InvalidInput))
    );

    set_ledger(&env, 100);
    assert_eq!(
        proxy.try_rearm(
            &asset,
            &breaker_id,
            &SorobanRearmConfig {
                arming_delay_secs: u64::MAX / 1_000_000_000 + 1,
            },
        ),
        Err(Ok(ContractError::InvalidInput))
    );
}

/// Assert `add_breaker` rejects an inert breaker config with `InvalidInput`.
fn assert_breaker_inert(build: impl FnOnce(&Env) -> CircuitBreakerConfig) {
    let (env, proxy, _source, asset) = setup();
    let breaker = build(&env);
    assert_eq!(
        proxy.try_add_breaker(&asset, &breaker),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn inert_breaker_stepwise_max_change_zero() {
    assert_breaker_inert(|env| {
        CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(env, Decimal::ZERO),
        })
    });
}

#[test]
fn inert_breaker_monotonic_max_streak_zero() {
    assert_breaker_inert(|env| {
        CircuitBreakerConfig::MonotonicRun(SorobanMonotonicRunConfig {
            max_streak: 0,
            min_relative_step_change: SorobanDecimal::from_decimal(env, Decimal::ONE_HALF),
        })
    });
}

#[test]
fn inert_breaker_monotonic_min_step_zero() {
    assert_breaker_inert(|env| {
        CircuitBreakerConfig::MonotonicRun(SorobanMonotonicRunConfig {
            max_streak: 3,
            min_relative_step_change: SorobanDecimal::from_decimal(env, Decimal::ZERO),
        })
    });
}

#[test]
fn inert_breaker_windowed_window_len_below_2() {
    assert_breaker_inert(|env| {
        CircuitBreakerConfig::WindowedChangeDelta(SorobanWindowedChangeDeltaConfig {
            window_len: 1,
            lookback_windows: 1,
            max_relative_mean_change: SorobanDecimal::from_decimal(env, Decimal::ONE_HALF),
        })
    });
}

#[test]
fn inert_breaker_windowed_lookback_zero() {
    assert_breaker_inert(|env| {
        CircuitBreakerConfig::WindowedChangeDelta(SorobanWindowedChangeDeltaConfig {
            window_len: 2,

            lookback_windows: 0,
            max_relative_mean_change: SorobanDecimal::from_decimal(env, Decimal::ONE_HALF),
        })
    });
}

#[test]
fn inert_breaker_windowed_max_delta_zero() {
    assert_breaker_inert(|env| {
        CircuitBreakerConfig::WindowedChangeDelta(SorobanWindowedChangeDeltaConfig {
            window_len: 2,
            lookback_windows: 1,
            max_relative_mean_change: SorobanDecimal::from_decimal(env, Decimal::ZERO),
        })
    });
}

#[test]
fn cumulative_breaker_rejects_stale_cache_baseline() {
    let (env, proxy, source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &2);
    set_ledger(&env, 100);
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    set_ledger(&env, 131);

    assert_eq!(
        proxy.try_add_breaker(
            &asset,
            &CircuitBreakerConfig::CumulativeChange(SorobanCumulativeChangeConfig {
                max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
            }),
        ),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn legacy_proxy_without_freshness_bounds_adds_non_cumulative_breakers() {
    let (env, proxy, _source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &2);
    env.as_contract(&proxy.address, || {
        let mut config = env
            .storage()
            .persistent()
            .get::<_, ProxyConfig>(&DataKey::Proxy(asset.clone()))
            .unwrap();
        config.max_age_secs = None;
        env.storage()
            .persistent()
            .set(&DataKey::Proxy(asset.clone()), &config);
    });

    assert!(proxy
        .try_add_breaker(
            &asset,
            &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
                max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
            }),
        )
        .is_ok());
}

#[test]
fn inert_breaker_zero_history() {
    let (_env, proxy, _source, asset) = setup();

    assert_eq!(
        proxy.try_configure_breakers(&asset, &0, &0),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn invalid_breaker_configuration_is_never_persisted() {
    let (env, proxy, _source, asset) = setup();
    proxy.configure_breakers(&asset, &0, &4);
    proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::MonotonicRun(SorobanMonotonicRunConfig {
            max_streak: 1,
            min_relative_step_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );
    let before = stored_breakers(&env, &proxy.address, &asset);

    assert_eq!(
        proxy.try_configure_breakers(&asset, &1, &4),
        Err(Ok(ContractError::BreakerError))
    );
    assert_eq!(stored_breakers(&env, &proxy.address, &asset), before);
    assert!(before.validate().is_ok());
}
// ── TTL tests ────────────────────────────────────────────────────────────────

#[test]
fn ttl_extend_does_not_panic_before_any_refresh() {
    let (_env, proxy, _source, asset) = setup();
    proxy.extend_ttl(&asset);
}

fn assert_ttl_extension_survives_missing_key(key: impl FnOnce(&Asset) -> DataKey) {
    let (env, proxy, _source, asset) = setup();
    env.as_contract(&proxy.address, || {
        env.storage().persistent().remove(&key(&asset));
    });

    assert_eq!(proxy.try_extend_ttl(&asset), Ok(Ok(())));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![TtlExtended {
            asset: asset.clone()
        }
        .to_xdr(&env, &proxy.address)]
    );
}

#[test]
fn ttl_extend_survives_missing_assets_registry() {
    assert_ttl_extension_survives_missing_key(|_| DataKey::Assets);
}

#[test]
fn ttl_extend_survives_missing_breakers() {
    assert_ttl_extension_survives_missing_key(|asset| DataKey::Breakers(asset.clone()));
}

#[test]
fn ttl_extend_rejects_missing_proxy() {
    let (env, proxy, _source, asset) = setup();
    env.as_contract(&proxy.address, || {
        env.storage()
            .persistent()
            .remove(&DataKey::Proxy(asset.clone()));
    });

    assert_eq!(
        proxy.try_extend_ttl(&asset),
        Err(Ok(ContractError::InvalidInput))
    );
    assert!(contract_events(&env, &proxy.address).is_empty());
}

#[test]
fn ttl_extend_covers_cache_and_history_after_refresh() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    proxy.extend_ttl(&asset);
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![TtlExtended {
            asset: asset.clone()
        }
        .to_xdr(&env, &proxy.address)]
    );
}

// ── missing_config tests ─────────────────────────────────────────────────────

#[test]
fn missing_config_refresh_fails_closed_on_missing_base() {
    // If the Base instance key is absent (e.g. TTL expired), refresh must
    // return ResolveFailed rather than silently aggregating across sources
    // whose base assets we can no longer validate.
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);

    env.as_contract(&proxy.address, || {
        env.storage().instance().remove(&DataKey::Base);
    });

    let result = proxy.refresh(&asset.clone());
    assert!(matches!(
        result,
        RefreshStatus::ResolveFailed(STORAGE_FAILED_CODE)
    ));
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
}

#[test]
fn missing_config_lastprice_fails_closed_on_missing_proxy_config() {
    // If the ProxyConfig persistent key is absent, lastprice must return None
    // rather than treating missing max_age as u64::MAX (no freshness limit).
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&asset.clone());
    assert!(legacy_lastprice(&proxy, &asset).is_some());

    // Remove the Proxy config to simulate TTL expiry.
    env.as_contract(&proxy.address, || {
        env.storage()
            .persistent()
            .remove(&DataKey::Proxy(asset.clone()));
    });

    // Must return None, not treat missing Proxy as "no freshness limit".
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
}

#[test]
fn missing_freshness_bounds_are_rejected() {
    let (_env, proxy, _source, asset) = setup();
    let mut config = proxy.get_proxy(&asset).unwrap();
    config.max_age_secs = None;

    assert_eq!(
        proxy.try_set_proxy(&asset, &config),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn missing_config_source_base_returns_none() {
    let (env, proxy, _source, _asset) = setup();
    env.as_contract(&proxy.address, || {
        env.storage().instance().remove(&DataKey::Base);
    });
    // Post-refactor `source_base()` returns Option — adapter contracts
    // decide how to handle a missing parent base. No longer a hard panic.
    assert_eq!(proxy.source_base(), None);
}

#[test]
fn missing_registered_assets_fails_closed() {
    let (env, proxy, _source, _asset) = setup();
    env.as_contract(&proxy.address, || {
        env.storage().persistent().remove(&DataKey::Assets);
    });

    assert_eq!(
        proxy.try_registered_assets(),
        Err(Ok(ContractError::StorageError))
    );
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
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: Address::generate(&env),
        asset: asset.clone(),
    });

    let result = proxy.try_set_proxy(
        &asset,
        &ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        },
    );

    assert!(result.is_err());
}
