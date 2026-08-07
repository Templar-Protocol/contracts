#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Groups N + O + P — Scaling math, freshness gate, history windowing.

use rstest::rstest;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Symbol};
use templar_proxy_oracle_soroban_common::{Asset, ProxyConfig};
use templar_proxy_oracle_soroban_contract::RefreshStatus;
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_integration_tests::common::{ledger, Bootstrap};
use templar_proxy_oracle_soroban_sep40_adapter_contract::{Sep40Adapter, Sep40AdapterClient};

fn deploy_adapter_with_decimals(b: &Bootstrap, decimals: u32) -> Sep40AdapterClient<'static> {
    let id = b.env.register(
        Sep40Adapter,
        (
            &b.admin,
            &b.runtime_id,
            &b.asset_btc,
            &decimals,
            &1_u32,
            &b.base_usd,
        ),
    );
    Sep40AdapterClient::new(&b.env, &id)
}

/// Source publishes at 8 decimals (mantissa = "human_value * 10^8"), so the
/// kernel's `NormalizedPrice` ends up with `expo = -8`. The adapter then
/// scales by `adapter_decimals + expo = adapter_decimals - 8`.
#[rstest]
#[case::downscale_to_2(2, 5_000_000_000, 50_00)]
#[case::identity_at_8(8, 5_000_000_000, 5_000_000_000)]
#[case::upscale_to_18(18, 1_000_000_000, 10_000_000_000_000_000_000)]
fn adapter_scales_for_decimals(
    #[case] adapter_decimals: u32,
    #[case] source_price: i128,
    #[case] expected: i128,
) {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let adapter = deploy_adapter_with_decimals(&b, adapter_decimals);
    b.push_upstream_price(&b.asset_btc, source_price, 100);
    let _ = b.refresh_one(&b.asset_btc);
    let sep40 = adapter.lastprice(&b.asset_btc).unwrap();
    assert_eq!(sep40.price, expected);
}

// `normalized_to_sep40` has an overflow guard, but with the in-contract
// invariants — adapter decimals ≤ 18, source decimals capped, mantissa fitted
// to i64 by `source_price_to_kernel` — the multiplication never actually
// overflows i128 through the public API. The guard remains as defense in
// depth; integration testing it would require bypassing those invariants.

#[test]
fn adapter_returns_none_for_unknown_asset() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let _ = b.refresh_one(&b.asset_btc);
    let other = Asset::Other(Symbol::new(&b.env, "ETH"));
    assert!(b.adapter.lastprice(&other).is_none());
}

#[test]
fn freshness_gate_blocks_stale_cache_for_adapter() {
    let b = Bootstrap::new();
    b.configure_default_feed(); // max_age_secs = 300
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let _ = b.refresh_one(&b.asset_btc);
    assert!(b.adapter.lastprice(&b.asset_btc).is_some());

    // Past the freshness window.
    ledger::advance_secs(&b.env, 400);
    assert!(b.adapter.lastprice(&b.asset_btc).is_none());
}

#[test]
fn refresh_restores_freshness() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let _ = b.refresh_one(&b.asset_btc);
    ledger::advance_secs(&b.env, 400);
    assert!(b.adapter.lastprice(&b.asset_btc).is_none());

    let now = b.env.ledger().timestamp();
    b.push_upstream_price(&b.asset_btc, 5_100_000_000, now);
    let _ = b.refresh_one(&b.asset_btc);
    assert!(b.adapter.lastprice(&b.asset_btc).is_some());
}

#[test]
fn one_manipulated_source_cannot_move_the_median() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.push_upstream_prices(
        &b.asset_btc,
        [5_000_000_000, 5_100_000_000, 50_000_000_000],
        100,
    );

    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Accepted(price) if price.mantissa == 5_100_000_000
    ));
    assert_eq!(
        b.adapter.lastprice(&b.asset_btc).unwrap().price,
        5_100_000_000
    );
}

#[test]
fn freshness_without_max_age_is_rejected() {
    let b = Bootstrap::new();
    let action = GovernanceAction::SetProxy(
        b.asset_btc.clone(),
        ProxyConfig {
            sources: b.source_configs(&b.asset_btc),
            min_sources: 3,
            max_age_secs: None,
            max_clock_drift_secs: Some(60),
        },
    );

    let id = b.governance.next_proposal_id();
    assert!(b
        .governance
        .try_create_proposal(&b.admin, &id, &action, &0)
        .is_err());
    assert!(b.runtime.get_proxy(&b.asset_btc).is_none());
}

#[test]
fn future_dated_source_is_not_published() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let future = b.env.ledger().timestamp() + 365 * 24 * 3600;
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, future);

    let _ = b.refresh_one(&b.asset_btc);
    assert!(b.adapter.lastprice(&b.asset_btc).is_none());
}

#[test]
fn prices_returns_history_oldest_first() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 16),
    );

    // 5 accepted refreshes at distinct ledger times.
    let prices: [i128; 5] = [
        1_000_000_000,
        1_100_000_000,
        1_200_000_000,
        1_300_000_000,
        1_400_000_000,
    ];
    for (i, p) in prices.iter().enumerate() {
        let ts = 100 + i as u64;
        b.push_upstream_price(&b.asset_btc, *p, ts);
        let _ = b.refresh_one(&b.asset_btc);
        ledger::advance_secs(&b.env, 1);
    }
    let series = b.adapter.prices(&b.asset_btc, &3_u32).unwrap();
    assert_eq!(series.len(), 3);
    // Last 3 oldest-first: prices[2], prices[3], prices[4].
    assert_eq!(series.get(0).unwrap().price, prices[2]);
    assert_eq!(series.get(2).unwrap().price, prices[4]);
}

#[test]
fn price_lookup_by_exact_timestamp() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 16),
    );

    for (i, p) in [1_000_000_000_i128, 1_100_000_000, 1_200_000_000]
        .iter()
        .enumerate()
    {
        let ts = 100 + i as u64;
        b.push_upstream_price(&b.asset_btc, *p, ts);
        let _ = b.refresh_one(&b.asset_btc);
        ledger::advance_secs(&b.env, 1);
    }

    assert!(b.adapter.price(&b.asset_btc, &101).is_some());
    assert!(b.adapter.price(&b.asset_btc, &999).is_none());
}

#[test]
fn prices_with_zero_records_returns_none() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let _ = b.refresh_one(&b.asset_btc);
    assert!(b.adapter.prices(&b.asset_btc, &0_u32).is_none());
}

#[test]
fn aggregated_history_is_capped_at_max_history_records() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 32),
    );

    // 40 refreshes — runtime caps history at MAX_HISTORY_RECORDS=32.
    for i in 0_u64..40 {
        let ts = 100 + i;
        b.push_upstream_price(&b.asset_btc, 1_000_000_000 + i128::from(i), ts);
        let _ = b.refresh_one(&b.asset_btc);
        ledger::advance_secs(&b.env, 1);
    }
    let series = b.adapter.prices(&b.asset_btc, &100_u32).unwrap();
    assert!(series.len() <= 32);

    // Suppress unused-import warning.
    let _ = Address::generate;
}
