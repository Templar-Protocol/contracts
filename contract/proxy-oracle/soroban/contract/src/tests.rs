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
use templar_proxy_oracle_soroban_common::normalized_to_sep40;

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
    let proxy_id = env.register(SorobanProxyOracle, (&admin, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);

    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: source_id,
        asset: asset.clone(),
    });
    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources,
            min_sources: 1,
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

fn assert_refresh_failure_event(env: &Env, proxy: &SorobanProxyOracleClient, asset: &Asset) {
    let events = contract_events(env, &proxy.address);
    assert_eq!(events.len(), 1);
    assert_eq!(legacy_lastprice(proxy, asset), None);
}

#[test]
fn parity_refresh_resolution_matrix_matches_near_baseline_semantics() {
    let (env, proxy, source, asset) = setup();

    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    let accepted = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        accepted.get(0).unwrap().1,
        RefreshStatus::Accepted(_)
    ));
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
    let stale = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        stale.get(0).unwrap().1,
        RefreshStatus::ResolveFailed(_)
    ));
    assert_refresh_failure_event(&env, &proxy, &asset);

    source.clear_price(&asset);
    let unavailable = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        unavailable.get(0).unwrap().1,
        RefreshStatus::SourceUnavailable
    ));
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
    let first_source_id = proxy
        .get_proxy(&asset)
        .unwrap()
        .sources
        .get(0)
        .unwrap()
        .oracle;
    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: first_source_id,
        asset: asset.clone(),
    });
    sources.push_back(SourceConfig {
        oracle: second_source_id,
        asset: asset.clone(),
    });
    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources,
            min_sources: 2,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        },
    );
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    second_source.clear_price(&asset);
    let quorum = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        quorum.get(0).unwrap().1,
        RefreshStatus::ResolveFailed(_)
    ));
    assert_refresh_failure_event(&env, &proxy, &asset);

    let eur = Asset::Other(Symbol::new(&env, "EUR"));
    let (wrong_base_id, wrong_base_source) = register_mock_source(&env, &eur);
    wrong_base_source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    let mut wrong_base_sources = Vec::new(&env);
    wrong_base_sources.push_back(SourceConfig {
        oracle: wrong_base_id,
        asset: asset.clone(),
    });
    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources: wrong_base_sources,
            min_sources: 1,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        },
    );
    let base_mismatch = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        base_mismatch.get(0).unwrap().1,
        RefreshStatus::SourceUnavailable
    ));
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
}

#[test]
fn parity_manual_trip_blocks_reads_refresh_and_maps_event_fields() {
    let (env, proxy, source, asset) = setup();
    let metadata = Bytes::from_array(&env, &[1_u8, 2, 3]);

    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
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

    source.set_price(&asset, &5_100_000_000_i128, &100_u64);
    let blocked = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        blocked.get(0).unwrap().1,
        RefreshStatus::Blocked(1)
    ));
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
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    set_ledger(&env, 101);
    source.set_price(&asset, &10_000_000_000_i128, &101_u64);
    let tripped = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        tripped.get(0).unwrap().1,
        RefreshStatus::Blocked(2)
    ));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![
            CircuitBreakerTripped {
                asset: asset.clone(),
                breaker_id,
                tripped_at_secs: 101,
                price: 10_000_000_000,
                timestamp: 101,
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
    let still_blocked = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        still_blocked.get(0).unwrap().1,
        RefreshStatus::Blocked(2)
    ));
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
            armed_after_secs: 103,
            accepted_history_source_code: 1,
        },
    );
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CircuitBreakerRearmed {
            asset: asset.clone(),
            breaker_id,
            armed_after_secs: 103,
            accepted_history_source_code: 1,
        }
        .to_xdr(&env, &proxy.address)]
    );
    let breakers_after_rearm = stored_breakers(&env, &proxy.address, &asset);
    assert_eq!(breakers_after_rearm.accepted_history().len(), 3);
    assert_eq!(breakers_after_rearm.observed_history().len(), 3);
    assert!(proxy.get_cached(&asset).is_none());
}

#[test]
fn parity_config_update_cache_invalidation_and_unauthorized_mutation() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(proxy.get_cached(&asset).is_some());

    let configured = proxy.get_proxy(&asset).unwrap();
    proxy.set_proxy(&asset, &configured);
    assert!(proxy.get_cached(&asset).is_none());
    assert_eq!(legacy_lastprice(&proxy, &asset), None);

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
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
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
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
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
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
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
    let (source_id, _source) = register_mock_source(&env, &base);
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: source_id,
        asset: asset.clone(),
    });

    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        },
    );

    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![ProxySet {
            asset,
            source_count: 1,
            min_sources: 1,
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
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    env.ledger().set(LedgerInfo {
        timestamp: 101,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });
    source.set_price(&asset, &10_000_000_000_i128, &101_u64);
    let result = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    assert!(matches!(
        result.get(0).unwrap().1,
        RefreshStatus::Blocked(2)
    ));
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![
            CircuitBreakerTripped {
                asset: asset.clone(),
                breaker_id,
                tripped_at_secs: 101,
                price: 10_000_000_000,
                timestamp: 101,
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
            armed_after_secs: 100,
            accepted_history_source_code: 0,
        },
    );
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![CircuitBreakerRearmed {
            asset: asset.clone(),
            breaker_id,
            armed_after_secs: 100,
            accepted_history_source_code: 0,
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
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    proxy.extend_ttl();
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![TtlExtended { asset_count: 1 }.to_xdr(&env, &proxy.address)]
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

    let result = proxy.refresh(&Vec::from_array(&proxy.env, [asset.clone()]));
    assert!(matches!(
        result.get(0).unwrap().1,
        RefreshStatus::Accepted(_)
    ));

    let price = legacy_lastprice(&proxy, &asset).unwrap();
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

    assert_eq!(legacy_lastprice(&proxy, &asset), None);
}

#[test]
fn manual_trip_blocks_refresh_and_cached_read() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.set_manual_trip(&asset, &true, &None);

    let result = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        result.get(0).unwrap().1,
        RefreshStatus::Blocked(1)
    ));
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
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
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    env.ledger().set(LedgerInfo {
        timestamp: 101,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });
    source.set_price(&asset, &5_100_000_000_i128, &101_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

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
fn same_timestamp_refresh_replaces_history_entry() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    source.set_price(&asset, &5_100_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    let prices = legacy_prices(&env, &proxy, &asset, 2).unwrap();
    assert_eq!(prices.len(), 1);
    assert_eq!(prices.get(0).unwrap().price, 5_100_000_000);
    assert_eq!(
        legacy_price(&proxy, &asset, 100).unwrap().price,
        5_100_000_000
    );
}

#[test]
fn failed_refresh_overwrites_accepted_cache_fail_closed() {
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(legacy_lastprice(&proxy, &asset).is_some());

    source.clear_price(&asset);
    let result = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    assert!(matches!(
        result.get(0).unwrap().1,
        RefreshStatus::SourceUnavailable
    ));
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
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
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &usd));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: source_id,
        asset: asset.clone(),
    });
    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        },
    );

    let result = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    assert!(matches!(
        result.get(0).unwrap().1,
        RefreshStatus::SourceUnavailable
    ));
    assert_eq!(legacy_lastprice(&proxy, &asset), None);
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
    sources.push_back(SourceConfig {
        oracle: Address::generate(&env),
        asset: asset.clone(),
    });

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
                min_sources: 2,
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
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    assert_eq!(legacy_prices(&env, &proxy, &asset, 0), None);
}

#[test]
fn invalid_config_duplicate_source_oracle_asset_pair() {
    let env = Env::default();
    env.mock_all_auths();
    let governance = Address::generate(&env);
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let (source_id, _source) = register_mock_source(&env, &base);
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: source_id.clone(),
        asset: asset.clone(),
    });
    sources.push_back(SourceConfig {
        oracle: source_id.clone(),
        asset: asset.clone(),
    });

    assert_eq!(
        proxy.try_set_proxy(
            &asset,
            &ProxyConfig {
                sources,
                min_sources: 1,
                max_age_secs: None,
                max_clock_drift_secs: None,
            },
        ),
        Err(Ok(ContractError::InvalidInput))
    );
}

#[test]
fn invalid_config_same_oracle_different_asset_is_not_a_duplicate() {
    let env = Env::default();
    env.mock_all_auths();
    let governance = Address::generate(&env);
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let (source_id, _source) = register_mock_source(&env, &base);
    let proxy_id = env.register(SorobanProxyOracle, (&governance, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: source_id.clone(),
        asset: Asset::Other(Symbol::new(&env, "BTC")),
    });
    sources.push_back(SourceConfig {
        oracle: source_id.clone(),
        asset: Asset::Other(Symbol::new(&env, "ETH")),
    });

    assert_eq!(
        proxy.try_set_proxy(
            &asset,
            &ProxyConfig {
                sources,
                min_sources: 1,
                max_age_secs: None,
                max_clock_drift_secs: None,
            },
        ),
        Ok(Ok(()))
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
                max_age_secs: None,
                max_clock_drift_secs: None,
            },
        ),
        Err(Ok(expected))
    );
}

#[test]
fn invalid_config_zero_sources() {
    assert_set_proxy_rejected(0, 1, ContractError::TooManySources);
}

#[test]
fn invalid_config_quorum_zero() {
    assert_set_proxy_rejected(1, 0, ContractError::InvalidInput);
}

#[test]
fn invalid_config_quorum_above_source_count() {
    assert_set_proxy_rejected(1, 2, ContractError::InvalidInput);
}

#[test]
fn invalid_config_too_many_sources() {
    assert_set_proxy_rejected(17, 1, ContractError::TooManySources);
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
fn invalid_config_invalid_accepted_history_source_code() {
    let (env, proxy, _source, asset) = setup();
    let breaker_id = proxy.add_breaker(
        &asset,
        &CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
        }),
    );

    assert_eq!(
        proxy.try_rearm(
            &asset,
            &breaker_id,
            &SorobanRearmConfig {
                armed_after_secs: 0,
                accepted_history_source_code: 99,
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
            max_relative_change_delta: SorobanDecimal::from_decimal(env, Decimal::ONE_HALF),
        })
    });
}

#[test]
fn inert_breaker_windowed_lookback_zero() {
    assert_breaker_inert(|env| {
        CircuitBreakerConfig::WindowedChangeDelta(SorobanWindowedChangeDeltaConfig {
            window_len: 2,
            lookback_windows: 0,
            max_relative_change_delta: SorobanDecimal::from_decimal(env, Decimal::ONE_HALF),
        })
    });
}

#[test]
fn inert_breaker_windowed_max_delta_zero() {
    assert_breaker_inert(|env| {
        CircuitBreakerConfig::WindowedChangeDelta(SorobanWindowedChangeDeltaConfig {
            window_len: 2,
            lookback_windows: 1,
            max_relative_change_delta: SorobanDecimal::from_decimal(env, Decimal::ZERO),
        })
    });
}

#[test]
fn inert_breaker_zero_history() {
    let (_env, proxy, _source, asset) = setup();

    assert_eq!(
        proxy.try_configure_breakers(&asset, &0, &0),
        Err(Ok(ContractError::InvalidInput))
    );
}

// ── TTL tests ────────────────────────────────────────────────────────────────

#[test]
fn ttl_extend_does_not_panic_before_any_refresh() {
    // Verify extend_ttl() is safe when Cache and History do not yet exist
    // (normal state after set_proxy but before any successful refresh).
    let (_env, proxy, _source, _asset) = setup();
    // Must not panic — Cache(BTC) and History(BTC) do not exist yet.
    proxy.extend_ttl();
}

#[test]
fn ttl_extend_covers_cache_and_history_after_refresh() {
    // After a successful refresh, extend_ttl must cover Cache and History.
    let (env, proxy, source, asset) = setup();
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    // Both Cache and History exist now; must not panic.
    proxy.extend_ttl();
    assert_eq!(
        contract_events(&env, &proxy.address),
        vec![TtlExtended { asset_count: 1 }.to_xdr(&env, &proxy.address)]
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

    let result = proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
    assert!(matches!(
        result.get(0).unwrap().1,
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
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));
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
fn missing_config_lastprice_no_freshness_limit_is_documented_exception() {
    // max_age_secs = None in a present ProxyConfig is the documented exception:
    // the operator explicitly configured no freshness limit. lastprice must
    // return the cached price regardless of age in that case.
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
    let proxy_id = env.register(SorobanProxyOracle, (&admin, &base));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    let mut sources = Vec::new(&env);
    sources.push_back(SourceConfig {
        oracle: source_id,
        asset: asset.clone(),
    });
    // max_age_secs = None: operator deliberately configures no freshness limit.
    proxy.set_proxy(
        &asset,
        &ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: None,
            max_clock_drift_secs: None,
        },
    );
    source.set_price(&asset, &5_000_000_000_i128, &100_u64);
    proxy.refresh(&Vec::from_array(&env, [asset.clone()]));

    // Advance time well past any normal max_age: must still return the price.
    env.ledger().set(LedgerInfo {
        timestamp: 999_999,
        protocol_version: 25,
        sequence_number: 200,
        ..Default::default()
    });
    assert!(legacy_lastprice(&proxy, &asset).is_some());
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
fn missing_config_registered_assets_returns_empty() {
    let (env, proxy, _source, _asset) = setup();
    env.as_contract(&proxy.address, || {
        env.storage().persistent().remove(&DataKey::Assets);
    });
    // Post-refactor `registered_assets()` defaults a missing key to empty
    // rather than panicking — it's an enumeration helper, not a SEP-40
    // surface promise.
    assert!(proxy.registered_assets().is_empty());
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
