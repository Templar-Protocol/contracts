use clap::Parser as _;
use rstest::rstest;

use super::*;
use crate::cli::{Cli, Command};
use crate::commands::spec::SpecNs;
use crate::spec::oracle::AggregatorSpec;
use crate::spec::plan::testing::alpha_market;

const NOW: Nanoseconds = Nanoseconds::from_secs(1_000);

fn price(timestamp: u64, value: i64) -> Price {
    Price {
        price: value,
        conf: 1,
        expo: -2,
        publish_time_ns: Nanoseconds::from_secs(timestamp),
    }
}

fn redstone() -> SourceSpec {
    SourceSpec::RedStone {
        oracle: "redstone.near".parse().unwrap(),
        price_id: "ETH".to_owned(),
        weight: Some(1),
    }
}

fn lazer() -> SourceSpec {
    SourceSpec::Lazer {
        oracle: "lazer.near".parse().unwrap(),
        feed_id: 1,
        weight: Some(1),
    }
}

fn pyth() -> SourceSpec {
    SourceSpec::Pyth {
        oracle: "pyth.near".parse().unwrap(),
        price_id: COLLATERAL_PRICE_ID,
        weight: Some(1),
    }
}

fn lst_source() -> SourceSpec {
    SourceSpec::Lst {
        oracle: "pyth.near".parse().unwrap(),
        price_id: COLLATERAL_PRICE_ID,
        contract: "lst.near".parse().unwrap(),
        method: "rate".to_owned(),
        decimals: 24,
        weight: Some(1),
    }
}

fn spec(sources: Vec<SourceSpec>) -> MarketSpec {
    let mut spec = alpha_market();
    spec.collateral.sources = sources;
    spec.borrow.sources.clear();
    spec.collateral.aggregator = Some(AggregatorSpec::MedianLow);
    spec.collateral.min_sources = 1;
    spec.collateral.max_age = None;
    spec.collateral.max_clock_drift = None;
    spec.market.price_maximum_age = Nanoseconds::from_secs(100);
    spec
}

fn verdict<'a>(checks: &'a [Check], id: &str) -> &'a Status {
    &checks.iter().find(|check| check.id == id).unwrap().status
}

#[test]
fn provider_failure_never_uses_healthy_storage() {
    let spec = spec(vec![redstone()]);
    let (resolved, checks) = leg(
        "collateral",
        &spec.collateral,
        &spec,
        vec![SourcePrices::Provider {
            selected: Err(anyhow::anyhow!("signature rejected")),
            stored: Ok(Some(price(1_000, 200))),
        }],
        NOW,
        Ok(CircuitBreakerSet::empty()),
        PricesFrom::Provider,
    );
    assert_eq!(resolved, None);
    assert!(verdict(&checks, "oracle.price.collateral.0").is_failure());
    assert!(verdict(&checks, "oracle.price.collateral.0")
        .detail()
        .contains("provider"));
    assert!(verdict(&checks, "oracle.aggregate.collateral").is_failure());
    assert!(matches!(
        verdict(&checks, "oracle.stored.collateral.0"),
        Status::Passed { .. }
    ));
}

#[rstest]
#[case::stale(Ok(Some(price(899, 100))))]
#[case::missing(Ok(None))]
#[case::unreadable(Err(anyhow::anyhow!("RPC unavailable")))]
fn provider_success_is_independent_of_storage(#[case] stored: PriceResult) {
    let spec = spec(vec![redstone()]);
    let selected = price(1_000, 200);
    let (resolved, checks) = leg(
        "collateral",
        &spec.collateral,
        &spec,
        vec![SourcePrices::Provider {
            selected: Ok(Some(selected)),
            stored,
        }],
        NOW,
        Ok(CircuitBreakerSet::empty()),
        PricesFrom::Provider,
    );
    // MedianLow consumes the lower confidence bound, not the raw spot value.
    assert_eq!(
        resolved,
        Some(Price {
            price: 199,
            conf: 0,
            ..selected
        })
    );
    assert!(matches!(
        verdict(&checks, "oracle.aggregate.collateral"),
        Status::Passed { .. }
    ));
    assert!(matches!(
        verdict(&checks, "oracle.stored.collateral.0"),
        Status::Warned { .. }
    ));
    assert_eq!(crate::spec::check::failures(&checks), 0);
}

#[rstest]
#[case::chain(redstone(), PricesFrom::Chain)]
#[case::pyth(pyth(), PricesFrom::Provider)]
#[case::lst(lst_source(), PricesFrom::Provider)]
fn absent_chain_input_is_skipped(#[case] source: SourceSpec, #[case] mode: PricesFrom) {
    let spec = spec(vec![source]);
    let (resolved, checks) = leg(
        "collateral",
        &spec.collateral,
        &spec,
        vec![SourcePrices::Chain(Ok(None))],
        NOW,
        Ok(CircuitBreakerSet::empty()),
        mode,
    );
    assert_eq!(resolved, None);
    for id in ["oracle.price.collateral.0", "oracle.aggregate.collateral"] {
        assert!(matches!(verdict(&checks, id), Status::Skipped { .. }));
        assert!(verdict(&checks, id).detail().contains("chain"));
    }
    assert!(matches!(
        verdict(&checks, "oracle.stored.collateral.0"),
        Status::Warned { .. }
    ));
}

#[rstest]
#[case::age_boundary(900, None, true)]
#[case::too_old(899, None, false)]
#[case::override_boundary(950, Some(50), true)]
#[case::override_stale(949, Some(50), false)]
#[case::drift_boundary(1_010, None, true)]
#[case::drift_exceeded(1_011, None, false)]
fn stored_and_selected_freshness_boundaries(
    #[case] timestamp: u64,
    #[case] max_age: Option<u64>,
    #[case] accepted: bool,
) {
    let mut spec = spec(vec![redstone()]);
    spec.collateral.max_age = max_age.map(Nanoseconds::from_secs);
    let (resolved, checks) = leg(
        "collateral",
        &spec.collateral,
        &spec,
        vec![SourcePrices::Chain(Ok(Some(price(timestamp, 200))))],
        NOW,
        Ok(CircuitBreakerSet::empty()),
        PricesFrom::Chain,
    );
    assert_eq!(resolved.is_some(), accepted);
    assert_eq!(
        matches!(
            verdict(&checks, "oracle.stored.collateral.0"),
            Status::Passed { .. }
        ),
        accepted
    );
    assert_eq!(
        matches!(
            verdict(&checks, "oracle.stored.collateral.0"),
            Status::Warned { .. }
        ),
        !accepted
    );
    assert_eq!(crate::spec::check::failures(&checks) > 0, !accepted);
}

#[test]
fn source_order_and_mixed_provenance_are_preserved() {
    let mut spec = spec(vec![pyth(), lazer(), lst_source(), redstone()]);
    spec.collateral.aggregator = Some(AggregatorSpec::Priority);
    for source in &mut spec.collateral.sources {
        match source {
            SourceSpec::Lazer { weight, .. }
            | SourceSpec::Pyth { weight, .. }
            | SourceSpec::Lst { weight, .. }
            | SourceSpec::RedStone { weight, .. } => *weight = None,
        }
    }
    let selected = price(1_000, 200);
    let (resolved, checks) = leg(
        "collateral",
        &spec.collateral,
        &spec,
        vec![
            SourcePrices::Chain(Ok(None)),
            SourcePrices::Provider {
                selected: Ok(Some(selected)),
                stored: Ok(None),
            },
            SourcePrices::Chain(Ok(Some(price(1_000, 300)))),
            SourcePrices::Provider {
                selected: Ok(Some(price(1_000, 400))),
                stored: Ok(None),
            },
        ],
        NOW,
        Ok(CircuitBreakerSet::empty()),
        PricesFrom::Provider,
    );
    assert_eq!(resolved, Some(selected));
    for (index, origin) in ["chain", "provider", "chain", "provider"]
        .iter()
        .enumerate()
    {
        assert!(
            verdict(&checks, &format!("oracle.price.collateral.{index}"))
                .detail()
                .starts_with(origin)
        );
    }
    assert!(verdict(&checks, "oracle.aggregate.collateral")
        .detail()
        .contains("provider and chain"));
}

#[rstest]
#[case::unreadable(Err(anyhow::anyhow!("breaker RPC failed")))]
#[case::tripped({
    let mut breakers = CircuitBreakerSet::empty();
    breakers.set_manual_trip(true, templar_proxy_oracle_kernel::primitive::AccountId::from_bytes([0; 64]), None);
    Ok(breakers)
})]
fn breaker_refusal_survives_provider_success(
    #[case] breakers: anyhow::Result<CircuitBreakerSet<CircuitBreaker>>,
) {
    let spec = spec(vec![redstone()]);
    let (resolved, checks) = leg(
        "collateral",
        &spec.collateral,
        &spec,
        vec![SourcePrices::Provider {
            selected: Ok(Some(price(1_000, 200))),
            stored: Ok(None),
        }],
        NOW,
        breakers,
        PricesFrom::Provider,
    );
    assert_eq!(resolved, None);
    assert!(verdict(&checks, "oracle.aggregate.collateral").is_failure());
}

fn invocation() -> (CliContext, PreflightPriceArgs) {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "--network",
        "mainnet",
        "spec",
        "check",
        "unused.toml",
        "--pyth-lazer-api-key",
        "test-token",
        "--pyth-lazer-ws-url",
        "wss://example.com/v1/stream",
        "--pyth-lazer-channel",
        "fixed_rate@200ms",
        "--pyth-lazer-max-payload-age-ms",
        "5000",
        "--redstone-node-path",
        "/nonexistent-eng777-node",
    ])
    .unwrap();
    let ctx = crate::context::build_context(&cli).unwrap();
    let Command::Spec {
        command: SpecNs::Check(args),
    } = cli.command
    else {
        panic!("spec check")
    };
    (ctx, args.prices)
}

#[tokio::test]
async fn only_used_provider_kinds_are_constructed_independently() {
    let (ctx, mut args) = invocation();
    args.lazer.pyth_lazer_api_key = None;
    args.redstone.redstone_node_path = "/bin/true".into();
    let providers = Providers::new(&ctx, &spec(vec![redstone()]), &args);
    assert!(providers.lazer.is_none());
    assert!(matches!(providers.redstone, Some(Ok(_))));
    drop(providers);
    let providers = Providers::new(&ctx, &spec(vec![lazer(), redstone()]), &args);
    assert!(providers.lazer.as_ref().unwrap().is_err());
    assert!(matches!(providers.redstone, Some(Ok(_))));
    drop(providers);

    args.lazer.pyth_lazer_api_key = Some("test-token".to_owned().into());
    args.redstone.redstone_node_path = "/nonexistent-eng777-node".into();
    let providers = Providers::new(&ctx, &spec(vec![lazer()]), &args);
    assert!(matches!(providers.lazer, Some(Ok(_))));
    assert!(providers.redstone.is_none());
    drop(providers);

    args.lazer.pyth_lazer_api_key = None;
    let providers = Providers::new(&ctx, &spec(vec![lazer(), redstone()]), &args);
    assert!(providers
        .lazer
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap_err()
        .to_string()
        .contains("pyth-lazer-api-key"));
    assert!(providers.redstone.as_ref().unwrap().is_err());
    assert!(providers
        .fetch(&lazer())
        .await
        .unwrap_err()
        .to_string()
        .contains("pyth-lazer-api-key"));
    assert!(providers.fetch(&redstone()).await.is_err());
}

#[tokio::test]
async fn unused_invalid_provider_configuration_is_not_built() {
    let (ctx, mut args) = invocation();
    args.lazer.pyth_lazer_api_key = None;
    args.lazer.pyth_lazer_channel = "invalid".to_owned();
    let chain_only = spec(vec![pyth(), lst_source()]);
    let providers = Providers::new(&ctx, &chain_only, &args);
    assert!(providers.lazer.is_none() && providers.redstone.is_none());
    let mut all = spec(vec![lazer(), redstone()]);
    args.prices_from = PricesFrom::Chain;
    let providers = Providers::new(&ctx, &all, &args);
    assert!(providers.lazer.is_none() && providers.redstone.is_none());
    args.prices_from = PricesFrom::Provider;
    let mut reporter = crate::report::Reporter::capturing(&[]);
    crate::dispatch::preflight::run_all(&ctx, &mut all, true, false, None, &args, &mut reporter)
        .await
        .unwrap();
    assert!(matches!(
        verdict(reporter.checks(), "preflight.online"),
        Status::Skipped { .. }
    ));
    all.oracle = crate::spec::OracleMode::Direct {
        account_id: "pyth.near".parse().unwrap(),
    };
    let providers = Providers::new(&ctx, &all, &args);
    assert!(providers.lazer.is_none() && providers.redstone.is_none());
}

#[test]
fn verified_provider_without_usable_ema_stays_skipped() {
    let spec = spec(vec![lazer()]);
    let (resolved, checks) = leg(
        "collateral",
        &spec.collateral,
        &spec,
        vec![SourcePrices::Provider {
            selected: Ok(None),
            stored: Ok(Some(price(1_000, 200))),
        }],
        NOW,
        Ok(CircuitBreakerSet::empty()),
        PricesFrom::Provider,
    );
    assert_eq!(resolved, None);
    assert!(matches!(
        verdict(&checks, "oracle.price.collateral.0"),
        Status::Skipped { .. }
    ));
    assert!(matches!(
        verdict(&checks, "oracle.aggregate.collateral"),
        Status::Skipped { .. }
    ));
    assert_eq!(crate::spec::check::failures(&checks), 0);
}
