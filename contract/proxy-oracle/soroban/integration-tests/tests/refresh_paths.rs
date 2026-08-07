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
use soroban_sdk::{contract, contractimpl, Address, Bytes, Env, Symbol};
use templar_proxy_oracle_soroban_common::{
    Asset, PriceData, PriceFeedTrait, ProxyConfig, SourceConfig,
};
use templar_proxy_oracle_soroban_contract::RefreshStatus;
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

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
                sources: b.source_configs(asset),
                min_sources: 3,
                max_age_secs: Some(300),
                max_clock_drift_secs: Some(60),
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
        },
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(
            eth.clone(),
            ProxyConfig {
                sources,
                min_sources: 3,
                max_age_secs: Some(300),
                max_clock_drift_secs: Some(60),
            },
        ),
    );
    b.push_upstream_price(&eth, 2_000_000_000, 100);
    b.runtime.refresh(&eth);
}
