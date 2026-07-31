//! Spec-level tests. All offline: a spec must be loadable, convertible, and
//! checkable with no network, which is what lets `market export` (ENG-540)
//! round-trip specs in unit tests.

use std::path::{Path, PathBuf};

use serde_json::Value;
use templar_common::market::MarketConfiguration;

use crate::spec::{
    check::{self, OnChainDecimals, Status},
    extends,
    oracle::SourceSpec,
    MarketSpec, BORROW_PRICE_ID, COLLATERAL_PRICE_ID,
};

fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/spec")
        .join(relative)
}

fn alpha_market() -> MarketSpec {
    extends::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deployments/alpha/iethfxrp-ixlmusdc.toml"),
    )
    .expect("fixture spec should load")
}

/// The load-bearing test: the spec must reproduce a market we already run
/// before it is allowed to create new ones. Compared against the very
/// `market-args.json` each market was deployed with.
///
/// Both sides are compared as parsed `MarketConfiguration`s, not as JSON text.
/// `Decimal` does not round-trip its own decimal representation — `"1.2"`
/// re-serializes as `"1.199…9"` — so comparing text would assert on formatting
/// rather than on meaning, and would fail for a spec that is in fact identical.
#[test]
fn reproduces_the_live_alpha_market_configuration() {
    let deployed: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/deployed/iethfxrp-ixlmusdc.templar-alpha.near.json"),
        )
        .expect("checked-in market args should be readable"),
    )
    .expect("market args should be JSON");
    let deployed: MarketConfiguration = serde_json::from_value(deployed["configuration"].clone())
        .expect("deployed market args should parse");

    let derived = alpha_market()
        .into_market_configuration(6, 7)
        .expect("spec should convert");

    assert_eq!(
        derived, deployed,
        "spec-derived configuration differs from the deployed market"
    );
}

/// Three ids that were string-interpolated per market, now derived once.
#[test]
fn derives_account_ids_from_the_registry_and_name() {
    let spec = alpha_market();

    assert_eq!(
        spec.market_id().expect("market id").as_str(),
        "iethfxrp-ixlmusdc.templar-alpha.near"
    );
    assert_eq!(
        spec.oracle_id().expect("oracle id").as_str(),
        "proxy-oracle-iethfxrp-ixlmusdc.templar-alpha.near"
    );
    assert_eq!(
        spec.governance_id().expect("governance id").as_str(),
        "proxy-gov-iethfxrp-ixlmusdc.templar-alpha.near"
    );
}

/// A `.near` registry can only be mainnet, so the spec never states it.
#[test]
fn derives_the_network_from_the_registry() {
    use templar_gateway_client::Network;

    assert_eq!(alpha_market().network().expect("network"), Network::Mainnet);
}

/// The price ids were typed out as `cccc…`/`bbbb…` in two files each, with
/// nothing checking they matched. They are constants now.
#[test]
fn price_identifiers_are_constant() {
    let configuration = alpha_market()
        .into_market_configuration(6, 7)
        .expect("spec should convert");

    assert_eq!(
        configuration
            .price_oracle_configuration
            .collateral_asset_price_id,
        COLLATERAL_PRICE_ID
    );
    assert_eq!(
        configuration
            .price_oracle_configuration
            .borrow_asset_price_id,
        BORROW_PRICE_ID
    );
}

/// `SourceSpec` is a deliberate parallel model of the three-level
/// externally-tagged on-chain enum. This is what keeps it honest.
///
/// Compared against the checked-in `proxy-*.json` that the retired deploy
/// actually configures, rather than an inline literal — an inline expectation
/// would only assert that this module agrees with itself, which is how the
/// `max_clock_drift` default was wrong here in the first place.
#[test]
fn derived_proxies_match_the_deployed_proxy_files() {
    let spec = alpha_market();
    let price_maximum_age = spec.market.price_maximum_age;

    for (side, derived) in [
        (
            "proxy-collateral.json",
            serde_json::to_value(spec.collateral.clone().into_proxy(price_maximum_age)),
        ),
        (
            "proxy-borrow.json",
            serde_json::to_value(spec.borrow.clone().into_proxy(price_maximum_age)),
        ),
    ] {
        let deployed: Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("fixtures/deployed/iethfxrp-ixlmusdc")
                    .join(side),
            )
            .unwrap_or_else(|error| panic!("read {side}: {error}")),
        )
        .unwrap_or_else(|error| panic!("parse {side}: {error}"));

        assert_eq!(
            derived.expect("proxy should serialize"),
            deployed,
            "derived {side} differs from what is deployed"
        );
    }
}

/// A source's weight and oracle are readable without matching on the variant —
/// ENG-541 and ENG-543 both need this.
#[test]
fn source_accessors_cover_both_variants() {
    let sources = alpha_market().borrow.sources;
    let kinds: Vec<_> = sources
        .iter()
        .map(|source| (source.oracle_id().as_str().to_owned(), source.weight()))
        .collect();

    assert_eq!(
        kinds,
        vec![
            ("pyth-lazer.templar-alpha.near".to_owned(), Some(8)),
            ("redstone-adapter.v1.tmplr.near".to_owned(), Some(2)),
        ]
    );
    assert!(matches!(sources[0], SourceSpec::Lazer { feed_id: 7, .. }));
}

/// Profiles merge left to right, then the declaring file wins over all of them.
#[test]
fn extends_applies_profiles_then_lets_the_market_win() {
    let spec = alpha_market();

    // From alpha-mainnet.toml.
    assert_eq!(spec.registry.as_str(), "templar-alpha.near");
    assert_eq!(spec.versions.market, "v1.3.0");
    // From irs-stable.toml.
    assert!(matches!(
        spec.market.interest_rate_strategy,
        templar_common::interest_rate_strategy::InterestRateStrategy::Piecewise(_)
    ));
    // From the market file itself.
    assert_eq!(spec.name, "iethfxrp-ixlmusdc");
    // Fully applied chains are emptied, so a re-serialized spec is not a lie.
    assert!(spec.extends.is_empty());
}

#[test]
fn unknown_fields_are_rejected() {
    let error = extends::load(&fixture("invalid/unknown-field.toml"))
        .expect_err("deny_unknown_fields should reject a typo");

    assert!(
        format!("{error:#}").contains("no_such_field"),
        "error should name the offending key: {error:#}"
    );
}

#[test]
fn extends_cycles_are_rejected() {
    let error =
        extends::load(&fixture("invalid/cycle-a.toml")).expect_err("a cycle must not loop forever");

    assert!(
        format!("{error:#}").contains("cycle"),
        "error should name the cycle: {error:#}"
    );
}

#[test]
fn offline_checks_pass_for_a_real_market() {
    let checks = check::run_offline(&alpha_market());
    let failures: Vec<_> = checks
        .iter()
        .filter(|check| check.status.is_failure())
        .collect();

    assert!(failures.is_empty(), "unexpected failures: {failures:#?}");
    assert!(
        checks
            .iter()
            .any(|check| check.id == "config.validate"
                && matches!(check.status, Status::Passed { .. })),
        "config.validate should run offline when decimals are stated: {checks:#?}"
    );
}

/// Without stated decimals the configuration cannot be built offline. That must
/// read as "not run", never as "fine".
#[test]
fn config_validate_is_skipped_not_passed_without_decimals() {
    let mut spec = alpha_market();
    spec.collateral.decimals = None;

    let checks = check::run_offline(&spec);
    let validate = checks
        .iter()
        .find(|check| check.id == "config.validate")
        .expect("config.validate should always be reported");

    assert!(matches!(validate.status, Status::Skipped { .. }));
}

/// The whole decimals matrix, pure and offline. The override exists because a
/// bridged asset shipped without `ft_metadata`, so "declared, nothing on chain"
/// must pass — while still reading as unverified, not as confirmed.
#[rstest::rstest]
#[case::derived(None, OnChainDecimals::Known(6), false, Some(6), false)]
#[case::agrees(Some(6), OnChainDecimals::Known(6), false, Some(6), false)]
#[case::disagrees(Some(8), OnChainDecimals::Known(6), false, None, true)]
#[case::disagrees_accepted(Some(8), OnChainDecimals::Known(6), true, Some(8), false)]
#[case::override_unverified(Some(8), OnChainDecimals::Unavailable, false, Some(8), false)]
#[case::nothing_to_go_on(None, OnChainDecimals::Unavailable, false, None, true)]
fn decimals_reconciliation(
    #[case] declared: Option<u8>,
    #[case] on_chain: OnChainDecimals,
    #[case] accept_mismatch: bool,
    #[case] expected: Option<u8>,
    #[case] should_fail: bool,
) {
    let (status, resolved) =
        check::reconcile_decimals("collateral", declared, on_chain, accept_mismatch);

    assert_eq!(resolved, expected);
    assert_eq!(status.is_failure(), should_fail, "{status:?}");
}

/// An unverified override must not read as confirmed — the report is what an
/// operator trusts before spending real NEAR.
#[test]
fn an_unverified_override_says_so() {
    let (status, _) =
        check::reconcile_decimals("borrow", Some(8), OnChainDecimals::Unavailable, false);

    assert!(
        matches!(&status, Status::Passed { detail } if detail.contains("unverified")),
        "{status:?}"
    );
}

/// The chain stores `price_maximum_age` in whole seconds and `time_chunk` in
/// whole milliseconds. Dividing silently would make the market enforce a
/// different bound than the proxy, or collapse a chunk to zero length.
#[rstest::rstest]
#[case::sub_second_age("price_maximum_age", "1500ms", "whole number of seconds")]
#[case::sub_milli_chunk("time_chunk", "500us", "whole number of milliseconds")]
#[case::zero_chunk("time_chunk", "0s", "at least 1ms")]
fn durations_that_would_not_survive_the_chain_are_rejected(
    #[case] field: &str,
    #[case] value: &str,
    #[case] expected: &str,
) {
    let mut spec = alpha_market();
    let parsed = crate::commands::duration::parse_duration(value).expect("valid duration");
    if field == "price_maximum_age" {
        spec.market.price_maximum_age = parsed;
    } else {
        spec.market.time_chunk = parsed;
    }

    let error = spec
        .into_market_configuration(6, 7)
        .expect_err("a lossy duration must not reach the chain");

    assert!(
        format!("{error:#}").contains(expected),
        "error should explain the loss: {error:#}"
    );
}

#[test]
fn sources_check_catches_an_unsatisfiable_minimum() {
    let mut spec = alpha_market();
    spec.borrow.min_sources = 99;

    let checks = check::run_offline(&spec);
    let sources = checks
        .iter()
        .find(|check| check.id == "config.sources")
        .expect("config.sources should always be reported");

    assert!(
        matches!(&sources.status, Status::Failed { detail } if detail.contains("99 of 2")),
        "{sources:#?}"
    );
}

/// Every migrated alpha market must reproduce the configuration it was
/// generated from (ENG-548).
///
/// The load-bearing test for the migration: these specs replace checked-in
/// `market-args.json` files that are the record of what is actually deployed.
/// Compared as parsed `MarketConfiguration`s rather than as JSON text, because
/// `Decimal` does not round-trip its own representation — comparing text would
/// assert on formatting and fail for specs that are in fact identical.
#[test]
fn migrated_specs_reproduce_their_deployed_configurations() {
    let specs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deployments/alpha");
    let configs = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/deployed");

    let mut checked = 0;
    for entry in std::fs::read_dir(&specs).expect("specs/alpha should exist") {
        let path = entry.expect("readable entry").path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let name = path
            .file_stem()
            .expect("spec has a stem")
            .to_string_lossy()
            .to_string();

        let spec = extends::load(&path).unwrap_or_else(|e| panic!("load {name}: {e:#}"));
        let flat = configs.join(format!("{name}.templar-alpha.near.json"));
        let source = flat;
        let deployed: Value = serde_json::from_str(
            &std::fs::read_to_string(&source)
                .unwrap_or_else(|e| panic!("read config for {name}: {e}")),
        )
        .unwrap_or_else(|e| panic!("parse config for {name}: {e}"));
        let deployed: MarketConfiguration =
            serde_json::from_value(deployed.get("configuration").cloned().unwrap_or(deployed))
                .unwrap_or_else(|e| panic!("parse configuration for {name}: {e}"));

        let (collateral, borrow) = (
            spec.collateral.decimals.expect("collateral decimals"),
            spec.borrow.decimals.expect("borrow decimals"),
        );
        let derived = spec
            .into_market_configuration(i32::from(collateral), i32::from(borrow))
            .unwrap_or_else(|e| panic!("convert {name}: {e:#}"));

        assert_eq!(
            derived, deployed,
            "spec `{name}` differs from what is deployed"
        );
        checked += 1;
    }

    assert_eq!(checked, 18, "every alpha market must be covered");
}
