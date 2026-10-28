#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Group B — Healthy refresh paths.
//!
//! B1: single-asset propagates source → runtime cache → SEP-40 adapter
//! B2: two assets are isolated (trip on one doesn't touch the other)
//! B3: each registered asset refreshes in its own transaction

use soroban_sdk::testutils::Address as _;
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{contract, contractimpl, Address, Bytes, Env, Symbol};
use templar_proxy_oracle_soroban_common::{
    AggregatorConfig, Asset, CircuitBreakerConfig, MedianConfig, PriceData, PriceFeedTrait,
    ProxyConfig, SorobanDecimal, SourceConfig, StepwiseChangeConfig,
};
use templar_proxy_oracle_soroban_contract::RefreshStatus;
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_integration_tests::common::{ledger, Bootstrap, MockOracleClient};

#[test]
fn healthy_refresh_propagates_to_sep40_adapter() {
    let b = Bootstrap::new();
    b.configure_default_feed();

    // 50.00 USD at decimals=8.
    let price: i128 = 5_000_000_000;
    let ts: u64 = 100;
    b.push_upstream_price(&b.asset_btc, price, ts);

    let status = b.refresh_one(&b.asset_btc);
    let RefreshStatus::Accepted(np) = status else {
        panic!("expected Accepted refresh status");
    };
    assert_eq!(np.mantissa, i64::try_from(price).expect("mantissa fits"));
    assert_eq!(np.expo, -8);
    assert_eq!(np.timestamp, ts);

    let cached = b.runtime.get_cached(&b.asset_btc).unwrap();
    assert_eq!(cached.updated_at, b.env.ledger().timestamp());

    // Adapter scales NormalizedPrice → SEP-40 PriceData via its own decimals.
    // 8 + (-8) = 0, so adapter price equals the kernel mantissa.
    let sep40 = b.adapter.lastprice(&b.asset_btc).unwrap();
    assert_eq!(sep40.price, price);
    assert_eq!(sep40.timestamp, ts);
}

/// Registers `asset` against the fixture's three upstream oracles.
fn add_feed(b: &Bootstrap, asset: &Asset) {
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(
            asset.clone(),
            ProxyConfig {
                aggregator: templar_proxy_oracle_soroban_common::AggregatorConfig::MedianLow(
                    templar_proxy_oracle_soroban_common::MedianConfig {
                        sources: b.source_configs(asset),
                        min_sources: 3,
                    },
                ),
                max_age_secs: 300,
                max_clock_drift_secs: 60,
            },
        ),
    );
}

#[contract]
struct BudgetExhaustingOracle;

#[contractimpl]
impl PriceFeedTrait for BudgetExhaustingOracle {
    fn base(env: Env) -> Asset {
        let payload = Bytes::from_array(&env, &[0_u8; 32]);
        for _ in 0..50_000 {
            let _ = env.crypto().sha256(&payload);
        }
        Asset::Other(Symbol::new(&env, "USD"))
    }

    fn assets(env: Env) -> soroban_sdk::Vec<Asset> {
        soroban_sdk::Vec::new(&env)
    }

    fn decimals(env: Env) -> u32 {
        let _ = env;
        8
    }

    fn resolution(env: Env) -> u32 {
        let _ = env;
        1
    }

    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData> {
        let _ = (env, asset, timestamp);
        None
    }

    fn prices(env: Env, asset: Asset, records: u32) -> Option<soroban_sdk::Vec<PriceData>> {
        let _ = (env, asset, records);
        None
    }

    fn lastprice(env: Env, asset: Asset) -> Option<PriceData> {
        let _ = (env, asset);
        None
    }
}
#[test]
fn two_independent_assets_have_isolated_state() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let eth = Asset::Other(Symbol::new(&b.env, "ETH"));
    add_feed(&b, &eth);

    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    b.push_upstream_price(&eth, 2_000_000_000, 100);
    assert!(matches!(
        b.runtime.refresh(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));
    assert!(matches!(
        b.runtime.refresh(&eth),
        RefreshStatus::Accepted(_)
    ));

    let tripper = Address::generate(&b.env);
    b.grant_role(
        &tripper,
        templar_proxy_oracle_soroban_governance_common::Role::ManualTripper,
    );
    b.submit_and_execute(
        &tripper,
        GovernanceAction::SetManualTrip(b.asset_btc.clone(), true, None),
    );

    assert!(matches!(
        b.runtime.refresh(&b.asset_btc),
        RefreshStatus::Blocked(_)
    ));
    assert!(matches!(
        b.runtime.refresh(&eth),
        RefreshStatus::Accepted(_)
    ));
}

#[test]
fn refresh_returns_one_requested_asset_status() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);

    assert!(matches!(
        b.runtime.refresh(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));
}

#[test]
fn registered_assets_refresh_in_separate_calls() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let eth = Asset::Other(Symbol::new(&b.env, "ETH"));
    add_feed(&b, &eth);
    b.push_upstream_price(&eth, 2_000_000_000, 100);
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);

    assert!(matches!(
        b.runtime.refresh(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));
    assert!(matches!(
        b.runtime.refresh(&eth),
        RefreshStatus::Accepted(_)
    ));
}

#[test]
#[should_panic(expected = "Error(Budget, ExceededLimit)")]
fn hostile_source_cannot_abort_a_separate_asset_refresh() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    assert!(matches!(
        b.runtime.refresh(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));

    let eth = Asset::Other(Symbol::new(&b.env, "ETH"));
    let hostile_oracle = b.env.register(BudgetExhaustingOracle, ());
    let mut sources = b.source_configs(&eth);
    sources.set(
        0,
        SourceConfig {
            oracle: hostile_oracle,
            asset: eth.clone(),
            expected_decimals: 8,
        },
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(
            eth.clone(),
            ProxyConfig {
                aggregator: templar_proxy_oracle_soroban_common::AggregatorConfig::MedianLow(
                    templar_proxy_oracle_soroban_common::MedianConfig {
                        sources,
                        min_sources: 3,
                    },
                ),
                max_age_secs: 300,
                max_clock_drift_secs: 60,
            },
        ),
    );
    b.push_upstream_price(&eth, 2_000_000_000, 100);
    b.runtime.refresh(&eth);
}

fn two_source_config(b: &Bootstrap, method: u32) -> ProxyConfig {
    let mut sources = b.source_configs(&b.asset_btc);
    let _ = sources.pop_back();
    let aggregator = match method {
        0 => AggregatorConfig::MedianLow(MedianConfig {
            sources,
            min_sources: 2,
        }),
        1 => AggregatorConfig::MedianHigh(MedianConfig {
            sources,
            min_sources: 2,
        }),
        2 => {
            // Ordered fallback: the second fixture carries the larger price.
            let first = sources.pop_front_unchecked();
            sources.push_back(first);
            AggregatorConfig::Priority(sources)
        }
        _ => unreachable!(),
    };
    ProxyConfig {
        aggregator,
        max_age_secs: 300,
        max_clock_drift_secs: 60,
    }
}

#[test]
fn governed_equal_weight_methods_reach_sep40_and_fail_closed() {
    for (method, expected) in [
        (0, 10_000_000_000_i128),
        (1, 20_000_000_000),
        (2, 20_000_000_000),
    ] {
        let b = Bootstrap::new();
        let config = two_source_config(&b, method);
        let id = b.governance.next_proposal_id();
        b.governance.create_proposal(
            &b.admin,
            &id,
            &GovernanceAction::SetProxy(b.asset_btc.clone(), config),
            &0,
        );
        b.governance.execute_proposal(&b.admin, &id);
        b.push_upstream_prices(&b.asset_btc, [10_000_000_000, 20_000_000_000, 0], 100);
        assert!(matches!(
            b.refresh_one(&b.asset_btc),
            RefreshStatus::Accepted(_)
        ));
        assert_eq!(b.adapter.lastprice(&b.asset_btc).unwrap().price, expected);

        // Only the first fixture remains fresh. Both medians lose quorum,
        // whereas Priority skips its stale first choice and serves the second.
        ledger::advance_secs(&b.env, 301);
        b.upstream.set_price(&b.asset_btc, &10_000_000_000, &401);
        let status = b.refresh_one(&b.asset_btc);
        if method == 2 {
            assert!(matches!(status, RefreshStatus::Accepted(_)));
            assert_eq!(
                b.adapter.lastprice(&b.asset_btc).unwrap().price,
                10_000_000_000
            );
        } else {
            assert!(matches!(status, RefreshStatus::ResolveFailed(_)));
            assert!(b.adapter.lastprice(&b.asset_btc).is_none());
        }
        ledger::advance_secs(&b.env, 301);
        assert!(!matches!(
            b.refresh_one(&b.asset_btc),
            RefreshStatus::Accepted(_)
        ));
        assert!(b.adapter.lastprice(&b.asset_btc).is_none());
    }
}

#[test]
fn mutable_source_scale_is_pinned_and_deliberate_repin_resets_without_reusing_ids() {
    let b = Bootstrap::new();
    let mut config = two_source_config(&b, 0);
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(b.asset_btc.clone(), config.clone()),
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 8),
    );
    let rule = CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
        max_relative_change: SorobanDecimal::from_decimal(
            &b.env,
            templar_primitives::Decimal::ONE_HALF,
        ),
    });
    for _ in 0..2 {
        b.submit_and_execute(
            &b.admin,
            GovernanceAction::AddBreaker(b.asset_btc.clone(), rule.clone()),
        );
    }
    b.push_upstream_prices(&b.asset_btc, [10_000_000_000, 20_000_000_000, 0], 100);
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));
    assert!(b.runtime.aggregated_history(&b.asset_btc, &8).is_some());

    // Only the declared scale changes: the source mantissa is untouched.
    b.upstream.set_decimals(&9);
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::ResolveFailed(_)
    ));
    assert!(b.adapter.lastprice(&b.asset_btc).is_none());
    assert_eq!(b.runtime.get_proxy(&b.asset_btc), Some(config.clone()));
    assert_eq!(
        b.runtime
            .get_breaker_set_view(&b.asset_btc)
            .unwrap()
            .next_id,
        2
    );

    let AggregatorConfig::MedianLow(median) = &mut config.aggregator else {
        unreachable!()
    };
    let mut source = median.sources.get(0).unwrap();
    source.expected_decimals = 9;
    median.sources.set(0, source);
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(b.asset_btc.clone(), config.clone()),
    );
    assert_eq!(b.runtime.get_proxy(&b.asset_btc), Some(config));
    assert!(b.runtime.get_cached(&b.asset_btc).is_none());
    assert!(b.runtime.aggregated_history(&b.asset_btc, &8).is_none());
    let reset = b.runtime.get_breaker_set_view(&b.asset_btc).unwrap();
    assert_eq!(reset.breaker_count, 0);
    assert_eq!(reset.next_id, 2);
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 8),
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::AddBreaker(b.asset_btc.clone(), rule),
    );
    let replacement = b.runtime.get_breaker_set_view(&b.asset_btc).unwrap();
    assert_eq!(replacement.breaker_count, 1);
    assert_eq!(replacement.next_id, 3);
    let id = b.governance.next_proposal_id();
    b.governance.create_proposal(
        &b.admin,
        &id,
        &GovernanceAction::RemoveBreaker(b.asset_btc.clone(), 0),
        &0,
    );
    assert!(b.governance.try_execute_proposal(&b.admin, &id).is_err());
    assert_eq!(
        b.runtime
            .get_breaker_set_view(&b.asset_btc)
            .unwrap()
            .to_xdr(&b.env),
        replacement.to_xdr(&b.env),
    );
}

#[test]
fn pin_admission_failure_preserves_existing_proxy_cache_and_breakers() {
    let b = Bootstrap::new();
    let original = two_source_config(&b, 0);
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(b.asset_btc.clone(), original.clone()),
    );
    b.push_upstream_prices(&b.asset_btc, [10_000_000_000, 20_000_000_000, 0], 100);
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));
    let cached = b.runtime.get_cached(&b.asset_btc).unwrap().to_xdr(&b.env);
    let breakers = b
        .runtime
        .get_breaker_set_view(&b.asset_btc)
        .unwrap()
        .to_xdr(&b.env);
    for pin in [9, 19] {
        let mut invalid = original.clone();
        let AggregatorConfig::MedianLow(median) = &mut invalid.aggregator else {
            unreachable!()
        };
        let mut source = median.sources.get(0).unwrap();
        source.expected_decimals = pin;
        median.sources.set(0, source);
        let id = b.governance.next_proposal_id();
        let action = GovernanceAction::SetProxy(b.asset_btc.clone(), invalid);
        if pin == 19 {
            assert!(b
                .governance
                .try_create_proposal(&b.admin, &id, &action, &0)
                .is_err());
            assert_eq!(b.governance.next_proposal_id(), id);
            assert!(b.governance.get_proposal(&id).is_none());
        } else {
            let proposal = b.governance.create_proposal(&b.admin, &id, &action, &0);
            assert!(b.governance.try_execute_proposal(&b.admin, &id).is_err());
            assert_eq!(b.governance.get_proposal(&id), Some(proposal));
        }
        assert_eq!(b.runtime.get_proxy(&b.asset_btc), Some(original.clone()));
        assert_eq!(
            b.runtime.get_cached(&b.asset_btc).unwrap().to_xdr(&b.env),
            cached
        );
        assert_eq!(
            b.runtime
                .get_breaker_set_view(&b.asset_btc)
                .unwrap()
                .to_xdr(&b.env),
            breakers
        );
    }
    // A source declaring an unsupported scale cannot be admitted even when
    // governance supplied an otherwise valid pin.
    let secondary = b.source_configs(&b.asset_btc).get(1).unwrap();
    MockOracleClient::new(&b.env, &secondary.oracle).set_decimals(&19);
    let id = b.governance.next_proposal_id();
    b.governance.create_proposal(
        &b.admin,
        &id,
        &GovernanceAction::SetProxy(b.asset_btc.clone(), two_source_config(&b, 1)),
        &0,
    );
    assert!(b.governance.try_execute_proposal(&b.admin, &id).is_err());
    assert_eq!(b.runtime.get_proxy(&b.asset_btc), Some(original));
    assert_eq!(
        b.runtime.get_cached(&b.asset_btc).unwrap().to_xdr(&b.env),
        cached
    );
    assert_eq!(
        b.runtime
            .get_breaker_set_view(&b.asset_btc)
            .unwrap()
            .to_xdr(&b.env),
        breakers
    );
}
