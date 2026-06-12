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
//! B3: refresh deduplicates repeated asset entries in its input
//! B4: empty input refreshes every registered asset

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Symbol, Vec as SVec};
use templar_proxy_oracle_soroban_common::{Asset, ProxyConfig, SourceConfig};
use templar_proxy_oracle_soroban_contract::RefreshStatus;
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_integration_tests::common::{
    Bootstrap, MockOracle, MockOracleClient, ADAPTER_DECIMALS, ADAPTER_RESOLUTION,
};

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

/// Helper: deploy a second mock upstream and register `asset` against it.
fn add_feed(b: &Bootstrap, asset: &Asset) -> Address {
    let upstream_id = b.env.register(
        MockOracle,
        (&b.base_usd, &ADAPTER_DECIMALS, &ADAPTER_RESOLUTION),
    );
    let mut sources = SVec::new(&b.env);
    sources.push_back(SourceConfig {
        oracle: upstream_id.clone(),
        asset: asset.clone(),
    });
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(
            asset.clone(),
            ProxyConfig {
                sources,
                min_sources: 1,
                max_age_secs: Some(300),
                max_clock_drift_secs: Some(60),
            },
        ),
    );
    upstream_id
}

#[test]
fn two_independent_assets_have_isolated_state() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let eth = Asset::Other(Symbol::new(&b.env, "ETH"));
    let eth_upstream_id = add_feed(&b, &eth);
    let eth_upstream = MockOracleClient::new(&b.env, &eth_upstream_id);

    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    eth_upstream.set_price(&eth, &2_000_000_000_i128, &100_u64);

    // Both refresh cleanly.
    let assets = SVec::from_array(&b.env, [b.asset_btc.clone(), eth.clone()]);
    let results = b.runtime.refresh(&assets);
    assert_eq!(results.len(), 2);
    assert!(matches!(
        results.get(0).unwrap().1,
        RefreshStatus::Accepted(_)
    ));
    assert!(matches!(
        results.get(1).unwrap().1,
        RefreshStatus::Accepted(_)
    ));

    // Manually trip BTC via governance.
    let tripper = Address::generate(&b.env);
    b.grant_role(
        &tripper,
        templar_proxy_oracle_soroban_governance_common::Role::ManualTripper,
    );
    b.submit_and_execute(
        &tripper,
        GovernanceAction::SetManualTrip(b.asset_btc.clone(), true, None),
    );

    let results = b.runtime.refresh(&assets);
    // Match by asset rather than position — refresh result ordering is an
    // implementation detail (and inputs are deduplicated).
    let btc_status = results
        .iter()
        .find(|(asset, _)| asset == &b.asset_btc)
        .unwrap()
        .1;
    let eth_status = results.iter().find(|(asset, _)| asset == &eth).unwrap().1;
    assert!(matches!(btc_status, RefreshStatus::Blocked(_)));
    assert!(matches!(eth_status, RefreshStatus::Accepted(_)));
}

#[test]
fn refresh_deduplicates_repeated_assets_in_input() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);

    let assets = SVec::from_array(
        &b.env,
        [
            b.asset_btc.clone(),
            b.asset_btc.clone(),
            b.asset_btc.clone(),
        ],
    );
    let results = b.runtime.refresh(&assets);
    assert_eq!(results.len(), 1);
}

#[test]
fn refresh_with_empty_input_refreshes_every_registered_asset() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let eth = Asset::Other(Symbol::new(&b.env, "ETH"));
    let eth_upstream_id = add_feed(&b, &eth);
    MockOracleClient::new(&b.env, &eth_upstream_id).set_price(&eth, &2_000_000_000_i128, &100_u64);
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);

    let results = b.runtime.refresh(&SVec::new(&b.env));
    assert_eq!(results.len(), 2);
    // Order is whatever `registered_assets()` produces — assert by set
    // membership rather than position.
    let mut seen_assets: std::vec::Vec<Asset> = std::vec::Vec::new();
    for entry in results.iter() {
        seen_assets.push(entry.0);
    }
    assert!(seen_assets.contains(&b.asset_btc));
    assert!(seen_assets.contains(&eth));
}
