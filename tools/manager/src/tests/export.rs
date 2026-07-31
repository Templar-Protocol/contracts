//! The validation gate: a spec must reproduce markets we already run.
//!
//! Offline — these read the checked-in deployment configs rather than the chain,
//! so the gate runs in the fast test partition and cannot be skipped for want of
//! a network.
//!
//! **Coverage is 2 of 18 alpha markets, by decision** (see ENG-540). The other
//! sixteen use oracle topologies a spec does not express; [`refuses_a_market_it_cannot_express`]
//! is what keeps that a refusal rather than a wrong answer.

use std::path::{Path, PathBuf};

use templar_common::market::MarketConfiguration;

use crate::spec::{export::Deployed, GovernanceSpec, MarketSpec, Versions};

/// The two markets that deploy their own proxy oracle.
const IN_SCOPE: [&str; 2] = ["iethfxrp-ixlmusdc", "iethwbtc-ixlmusdc"];

fn alpha(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/deployed/alpha")
        .join(relative)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// Versions and governance are not recoverable from market state; the CLI reads
/// them from the registry and the governance contract. They are supplied here so
/// the round-trip can exercise everything that *is* recoverable.
fn placeholder_versions() -> Versions {
    Versions {
        market: "v1.3.0".to_owned(),
        proxy_oracle: "oracle@0.3.0".to_owned(),
        proxy_governance: "governance@0.1.0".to_owned(),
    }
}

fn placeholder_governance() -> GovernanceSpec {
    GovernanceSpec {
        admin: "templar-alpha.near".parse().expect("valid account"),
        ttl_default: templar_common::Nanoseconds::from_ns(0),
    }
}

fn deployed(market: &str) -> Deployed {
    let configuration: serde_json::Value = read_json(&alpha(&format!("{market}/market-args.json")));
    Deployed {
        market_id: format!("{market}.templar-alpha.near")
            .parse()
            .expect("valid market id"),
        configuration: serde_json::from_value(configuration["configuration"].clone())
            .unwrap_or_else(|error| panic!("parse {market} configuration: {error}")),
        collateral_proxy: read_json(&alpha(&format!("{market}/proxy-collateral.json"))),
        borrow_proxy: read_json(&alpha(&format!("{market}/proxy-borrow.json"))),
        versions: placeholder_versions(),
        governance: placeholder_governance(),
    }
}

/// Export then re-derive: the spec must land back on exactly the deployed
/// configuration and both proxies. This is what licenses the tool to create
/// markets.
#[test]
fn round_trips_every_in_scope_alpha_market() {
    for market in IN_SCOPE {
        let input = deployed(market);
        let (configuration, collateral_proxy, borrow_proxy) = (
            input.configuration.clone(),
            input.collateral_proxy.clone(),
            input.borrow_proxy.clone(),
        );
        let oracle = &configuration.price_oracle_configuration;
        let (collateral_decimals, borrow_decimals) = (
            oracle.collateral_asset_decimals,
            oracle.borrow_asset_decimals,
        );

        let spec = MarketSpec::from_deployed(input)
            .unwrap_or_else(|error| panic!("{market} should export: {error:#}"));

        let price_maximum_age = spec.market.price_maximum_age;
        assert_eq!(
            spec.collateral.clone().into_proxy(price_maximum_age),
            collateral_proxy,
            "{market}: collateral proxy did not round-trip"
        );
        assert_eq!(
            spec.borrow.clone().into_proxy(price_maximum_age),
            borrow_proxy,
            "{market}: borrow proxy did not round-trip"
        );
        assert_eq!(
            spec.into_market_configuration(collateral_decimals, borrow_decimals)
                .unwrap_or_else(|error| panic!("{market} should convert: {error:#}")),
            configuration,
            "{market}: configuration did not round-trip"
        );
    }
}

/// Decimals survive the round trip, which is what lets an exported spec be
/// checked offline without an on-chain metadata lookup.
#[test]
fn export_recovers_decimals() {
    let input = deployed("iethfxrp-ixlmusdc");
    let configuration = input.configuration.clone();
    let spec = MarketSpec::from_deployed(input).expect("should export");

    assert_eq!(
        spec.collateral.decimals.map(i32::from),
        Some(
            configuration
                .price_oracle_configuration
                .collateral_asset_decimals
        )
    );
}

/// `symbol` never reaches the chain, so an export must leave it unset rather
/// than invent a plausible ticker.
#[test]
fn export_leaves_symbol_unset() {
    let input = deployed("iethfxrp-ixlmusdc");
    let spec = MarketSpec::from_deployed(input).expect("should export");

    assert!(spec.collateral.symbol.is_none());
    assert!(spec.borrow.symbol.is_none());
}

/// An exported spec has to be a *usable* spec, not just a valid value: TOML
/// serialization is order-sensitive about tables, so this exercises
/// export → render → reload and asserts the reloaded spec is identical.
#[test]
fn exported_spec_renders_to_loadable_toml() {
    let input = deployed("iethfxrp-ixlmusdc");
    let exported = MarketSpec::from_deployed(input).expect("should export");

    let rendered = toml::to_string_pretty(&exported).expect("spec should render as TOML");
    let reloaded: MarketSpec = toml::from_str(&rendered)
        .unwrap_or_else(|error| panic!("reload failed: {error}\n{rendered}"));

    assert_eq!(reloaded, exported);
}

/// `None` freshness bounds mean *unbounded* on chain, but *unspecified* in a
/// spec — where `into_proxy` fills them from `price_maximum_age` and
/// `DEFAULT_MAX_CLOCK_DRIFT`. Copying one through would turn "accept a price of
/// any age" into "enforce the market's bound" on the next deploy, silently.
/// Neither in-scope alpha market exercises this, so it is constructed.
#[test]
fn refuses_a_proxy_with_an_unbounded_freshness_filter() {
    let mut input = deployed("iethfxrp-ixlmusdc");
    input.collateral_proxy.freshness_filter.max_age_ns = None;

    let error = MarketSpec::from_deployed(input)
        .expect_err("an unbounded freshness filter is not expressible as a spec");

    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("collateral") && rendered.contains("unbounded"),
        "the error must name the side and why it cannot round-trip: {rendered}"
    );
}

/// The other sixteen alpha markets read an oracle a spec does not derive.
/// Refusing is the whole point: emitting a spec that re-derives to a *different*
/// oracle account would be a silent wrong answer.
#[test]
fn refuses_a_market_it_cannot_express() {
    let configuration: MarketConfiguration =
        read_json(&alpha("ixlm-ixlmusdc.templar-alpha.near.json"));
    let in_scope = deployed("iethfxrp-ixlmusdc");

    let error = MarketSpec::from_deployed(Deployed {
        market_id: "ixlm-ixlmusdc.templar-alpha.near"
            .parse()
            .expect("valid market id"),
        configuration,
        collateral_proxy: in_scope.collateral_proxy,
        borrow_proxy: in_scope.borrow_proxy,
        versions: placeholder_versions(),
        governance: placeholder_governance(),
    })
    .expect_err("a pyth-direct market is not expressible as a spec");

    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("pyth-oracle.near"),
        "the error must name the oracle actually deployed: {rendered}"
    );
    assert!(
        rendered.contains("proxy-oracle-ixlm-ixlmusdc.templar-alpha.near"),
        "the error must name what a spec would derive instead: {rendered}"
    );
}
