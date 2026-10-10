//! Real signed RedStone provider reads against deployed sandbox contracts.
//! The bridge replaces transport only; the production gateway verifies both packages.

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use clap::Parser as _;
use near_account_id::AccountId;
use templar_common::{asset::FungibleAsset, Nanoseconds};
use templar_contract_artifacts::{format_version_key, ArtifactId};
use templar_gateway_client::Client;
use templar_gateway_methods_spec::{account, redstone};
use templar_gateway_testing::{wasm, SandboxHarness};
use templar_gateway_types::{Base64Bytes, OperationStatus};

use crate::{
    cli::{Cli, Command},
    commands::{market::MarketNs, spec::SpecNs},
    report::Reporter,
    spec::{
        check::{self, Check, Status},
        oracle::{AggregatorSpec, ReferenceAsset, SourceSpec},
        plan::PlanFile,
        GovernanceSpec, MarketSpec, OracleMode,
    },
};

mod bridge;
use bridge::{BridgeFixture, NEW_TIMESTAMP_MS, OLD_TIMESTAMP_MS};

/// Accounts and files belong to this fixture, not to any production network.
struct Fixture {
    harness: SandboxHarness,
    spec: MarketSpec,
    spec_path: PathBuf,
    plan_path: PathBuf,
    bridge: BridgeFixture,
    client: Client,
    oracle_id: AccountId,
    registry_id: AccountId,
    directory: PathBuf,
}

impl Fixture {
    #[allow(clippy::too_many_lines, reason = "one sandbox deployment fixture")]
    async fn new() -> Result<Self> {
        let harness = SandboxHarness::start().await?;
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let registry_id: AccountId =
            format!("pf-{}-{unique:x}.near", std::process::id()).parse()?;
        let client =
            super::registry_upgrade::live_registry(&harness, &registry_id, "2.0.0").await?;
        let mut versions = Vec::new();
        for (artifact, bytes) in [
            (ArtifactId::Market, wasm::market().await),
            (ArtifactId::ProxyOracle, wasm::proxy_oracle().await),
            (ArtifactId::ProxyGovernance, wasm::proxy_governance().await),
        ] {
            let metadata = artifact.metadata();
            let key = format_version_key(
                metadata.package_name,
                metadata
                    .version()
                    .context("catalog artifact has a version")?,
                bytes,
            );
            super::registry_upgrade::add_version(&client, &registry_id, &key, bytes.to_vec())
                .await?;
            versions.push(key);
        }
        let oracle_id = harness
            .deploy_redstone_adapter("preflight-redstone")
            .await?;
        let eth = harness
            .deploy_ft("preflight-eth", "Fixture Ether", "ETH")
            .await?;
        let btc = harness
            .deploy_ft("preflight-btc", "Fixture Bitcoin", "BTC")
            .await?;
        let mut spec = crate::spec::plan::testing::alpha_market();
        spec.registry = registry_id.clone();
        spec.name = "eth-btc".to_owned();
        spec.market_version = versions[0].clone();
        spec.oracle = OracleMode::Proxy {
            governance: GovernanceSpec {
                admin: registry_id.clone(),
                ttl_default: Nanoseconds::from_ns(0),
            },
            oracle_version: versions[1].clone(),
            governance_version: versions[2].clone(),
        };
        spec.collateral.asset = FungibleAsset::nep141(eth);
        spec.borrow.asset = FungibleAsset::nep141(btc);
        spec.collateral.symbol = Some("ETH".to_owned());
        spec.borrow.symbol = Some("BTC".to_owned());
        spec.collateral.reference = Some(ReferenceAsset::Unlisted {
            reason: "sandbox fixture asset".to_owned(),
        });
        spec.borrow.reference = spec.collateral.reference.clone();
        spec.collateral.decimals = Some(24);
        spec.borrow.decimals = Some(24);
        spec.collateral.aggregator = Some(AggregatorSpec::MedianLow);
        spec.borrow.aggregator = Some(AggregatorSpec::MedianLow);
        spec.collateral.min_sources = 1;
        spec.borrow.min_sources = 1;
        spec.collateral.sources = vec![SourceSpec::RedStone {
            oracle: oracle_id.clone(),
            price_id: "ETH".to_owned(),
            weight: Some(1),
        }];
        spec.borrow.sources = vec![SourceSpec::RedStone {
            oracle: oracle_id.clone(),
            price_id: "BTC".to_owned(),
            weight: Some(1),
        }];
        spec.collateral.max_age = None;
        spec.borrow.max_age = None;
        spec.market.protocol_account_id = registry_id.clone();
        spec.market.yield_weights = templar_common::market::YieldWeights::new_with_supply_weight(9)
            .with_static(registry_id.clone(), 1);
        let now_ms = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
        let midpoint_ms = OLD_TIMESTAMP_MS + (NEW_TIMESTAMP_MS - OLD_TIMESTAMP_MS) / 2;
        // MarketConfiguration stores whole seconds; rounding down is conservative.
        let maximum_age = Nanoseconds::from_secs(
            now_ms
                .checked_sub(midpoint_ms)
                .context("host time precedes fixture packages")?
                / 1_000,
        );
        assert!(now_ms - OLD_TIMESTAMP_MS > maximum_age.as_ms());
        assert!(now_ms - NEW_TIMESTAMP_MS < maximum_age.as_ms());
        // Both sides have almost two days of slack, so block production and CLI
        // startup cannot turn this into a boundary-timing test.
        assert!(NEW_TIMESTAMP_MS - midpoint_ms > 24 * 60 * 60 * 1_000);
        spec.market.price_maximum_age = maximum_age;
        let directory = std::env::temp_dir().join(format!("tmplrmgr-preflight-{unique:x}"));
        std::fs::create_dir(&directory)?;
        let spec_path = directory.join("market.toml");
        let plan_path = directory.join("plan.json");
        std::fs::write(&spec_path, toml::to_string_pretty(&spec)?)?;
        let bridge = BridgeFixture::new()?;
        Ok(Self {
            harness,
            spec,
            spec_path,
            plan_path,
            bridge,
            client,
            oracle_id,
            registry_id,
            directory,
        })
    }

    fn args(&self, command: &[&str]) -> Vec<String> {
        // The standard sandbox catalog prebuild uses --no-abi. This opt-out
        // applies only to those known test WASMs, never to oracle verification.
        let abi_bypass =
            matches!(command, ["market", "plan" | "apply", ..]).then_some("--skip-abi-check");
        [
            "tmplrmgr",
            "--network",
            "mainnet",
            "--rpc-url",
            self.harness.network.rpc_endpoints[0].url.as_str(),
            "-q",
        ]
        .into_iter()
        .chain(command.iter().copied())
        .chain(abi_bypass)
        .chain([
            "--redstone-node-path",
            self.bridge
                .node_path()
                .to_str()
                .expect("UTF-8 fixture path"),
        ])
        .map(str::to_owned)
        .collect()
    }

    fn cli(&self, command: &[&str]) -> Result<Cli> {
        let mut cli = Cli::try_parse_from(self.args(command))?;
        // Do not mutate the process environment: parallel clap tests may read
        // it. Clearing the unused argument proves this RedStone-only fixture
        // needs no Lazer credential even on a developer's configured machine.
        let prices = match &mut cli.command {
            Command::Spec {
                command: SpecNs::Check(args),
            } => &mut args.prices,
            Command::Market {
                command: MarketNs::Plan(args),
            } => &mut args.prices,
            Command::Market {
                command: MarketNs::Apply(args),
            } => &mut args.prices,
            Command::Market {
                command: MarketNs::Verify(args),
            } => &mut args.prices,
            _ => anyhow::bail!("fixture only parses the four preflight commands"),
        };
        prices.lazer.pyth_lazer_api_key = None;
        Ok(cli)
    }

    async fn run(&self, command: &[&str]) -> Result<()> {
        let cli = self.cli(command)?;
        let ctx = crate::context::build_context(&cli)?;
        crate::dispatch::dispatch(ctx, cli.command).await
    }

    async fn checks(&self, chain: bool, deployed: bool) -> Result<Vec<Check>> {
        let path = self.spec_path.to_str().context("UTF-8 spec path")?;
        let command = if chain {
            vec!["spec", "check", path, "--prices-from", "chain"]
        } else {
            vec!["spec", "check", path]
        };
        let cli = self.cli(&command)?;
        let ctx = crate::context::build_context(&cli)?;
        let Command::Spec {
            command: SpecNs::Check(args),
        } = cli.command
        else {
            unreachable!("parsed spec check")
        };
        let mut spec = self.spec.clone();
        let deployed_oracle = if deployed {
            Some(spec.oracle_id()?)
        } else {
            None
        };
        let mut reporter = Reporter::capturing(&[]).quieted();
        crate::dispatch::preflight::run_all(
            &ctx,
            &mut spec,
            false,
            false,
            deployed_oracle.as_ref(),
            &args.prices,
            &mut reporter,
        )
        .await?;
        Ok(reporter.into_checks())
    }

    async fn push(&self, payload: Vec<u8>) -> Result<()> {
        let result = self
            .client
            .execute_as(
                self.registry_id.clone(),
                redstone::WritePrices {
                    oracle_id: self.oracle_id.clone(),
                    feed_ids: vec!["ETH".into(), "BTC".into()],
                    payload: Base64Bytes(payload),
                },
            )
            .await?;
        anyhow::ensure!(
            result.operation.status == OperationStatus::Succeeded,
            "{result:?}"
        );
        Ok(())
    }

    async fn stored(&self) -> Result<Vec<redstone::PriceDataEntry>> {
        let mut data = self
            .client
            .read(redstone::ReadPriceData {
                oracle_id: self.oracle_id.clone(),
                feed_ids: vec!["ETH".into(), "BTC".into()],
            })
            .await?;
        data.sort_by(|a, b| a.feed_id.as_ref().cmp(b.feed_id.as_ref()));
        Ok(data)
    }

    async fn assert_targets(&self, deployed: bool) -> Result<()> {
        for account_id in [
            self.spec.market_id()?,
            self.spec.oracle_id()?,
            self.spec.governance_id()?,
        ] {
            let result = self.client.read(account::Get { account_id }).await;
            if deployed {
                assert_ne!(
                    result?.code_hash,
                    near_primitives::hash::CryptoHash::default().to_string()
                );
            } else {
                assert!(
                    matches!(
                        result,
                        Err(templar_gateway_core::GatewayError::AccountNotFound(_))
                    ),
                    "{result:?}"
                );
            }
        }
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn status<'a>(checks: &'a [Check], id: &str) -> &'a Status {
    &checks
        .iter()
        .find(|check| check.id == id)
        .unwrap_or_else(|| panic!("missing {id}"))
        .status
}

fn assert_selected(checks: &[Check], provenance: &str) {
    for id in [
        "oracle.price.collateral.0",
        "oracle.price.borrow.0",
        "oracle.aggregate.collateral",
        "oracle.aggregate.borrow",
        "oracle.aggregate.pair",
    ] {
        let selected = status(checks, id);
        assert!(
            matches!(selected, Status::Passed { .. }),
            "{id}: {selected:?}"
        );
        assert!(selected.detail().contains(provenance), "{id}: {selected:?}");
    }
}

fn assert_stored(checks: &[Check], warned: bool) {
    for id in ["oracle.stored.collateral.0", "oracle.stored.borrow.0"] {
        let stored = status(checks, id);
        if warned {
            assert!(matches!(stored, Status::Warned { .. }), "{id}: {stored:?}");
        } else {
            assert!(matches!(stored, Status::Passed { .. }), "{id}: {stored:?}");
        }
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one ordered empty/stale/fresh storage scenario"
)]
async fn requires_sandbox_manager_provider_preflight_redstone_only() -> Result<()> {
    let fixture = Fixture::new().await?;
    let spec_path = fixture.spec_path.to_str().context("UTF-8 spec path")?;
    let plan_path = fixture.plan_path.to_str().context("UTF-8 plan path")?;
    let market_id = fixture.spec.market_id()?;
    let registry_id = fixture.registry_id.as_str();
    let public_key = super::TEST_SECRET_KEY
        .parse::<near_api::SecretKey>()?
        .public_key()
        .to_string();
    let verify = [
        "market",
        "verify",
        market_id.as_str(),
        "--governance-admin",
        registry_id,
        "--against",
        spec_path,
    ];

    // Never-pushed feeds can be checked and planned, without secretly writing
    // either adapter storage or the accounts the plan would deploy.
    assert!(fixture.stored().await?.is_empty());
    let checks = fixture.checks(false, false).await?;
    assert_selected(&checks, "provider");
    assert_stored(&checks, true);
    check::gate(
        &checks,
        "fixture",
        "empty storage does not block provider preflight",
    )
    .with_context(|| format!("preflight checks: {checks:#?}"))?;
    fixture.run(&["spec", "check", spec_path]).await?;
    fixture
        .run(&[
            "market",
            "plan",
            spec_path,
            "--out",
            plan_path,
            "--signer-id",
            registry_id,
            "--public-key",
            &public_key,
        ])
        .await?;
    let plan: PlanFile = serde_json::from_slice(&std::fs::read(&fixture.plan_path)?)?;
    assert_selected(&plan.checks, "provider");
    assert_stored(&plan.checks, true);
    for step in &plan.steps {
        assert_ne!(
            step.receiver_id, fixture.oracle_id,
            "deployment must not push adapter prices"
        );
    }
    assert!(fixture.stored().await?.is_empty());
    fixture.assert_targets(false).await?;

    fixture
        .run(&[
            "market",
            "apply",
            "--plan",
            plan_path,
            "--yes",
            "--signer-id",
            registry_id,
            "--secret-key",
            super::TEST_SECRET_KEY,
        ])
        .await?;
    fixture.assert_targets(true).await?;
    assert!(fixture.stored().await?.is_empty());
    fixture.run(&verify).await?;
    let checks = fixture.checks(false, true).await?;
    assert_selected(&checks, "provider");
    assert_stored(&checks, true);
    assert!(fixture.stored().await?.is_empty());

    // A live market without pushed feeds is still "nothing to judge" in chain
    // mode, not the removed eager-push-only live_market failure.
    let startups = fixture.bridge.startup_count()?;
    let mut empty_chain_verify = verify.to_vec();
    empty_chain_verify.extend(["--prices-from", "chain"]);
    fixture.run(&empty_chain_verify).await?;
    let empty_chain = fixture.checks(true, true).await?;
    for id in [
        "oracle.price.collateral.0",
        "oracle.price.borrow.0",
        "oracle.aggregate.collateral",
        "oracle.aggregate.borrow",
        "oracle.aggregate.pair",
    ] {
        assert!(
            matches!(status(&empty_chain, id), Status::Skipped { .. }),
            "{empty_chain:?}"
        );
    }
    check::gate(&empty_chain, "fixture", "missing chain prices are skipped")?;
    assert_stored(&empty_chain, true);
    assert_eq!(fixture.bridge.startup_count()?, startups);
    assert!(fixture.stored().await?.is_empty());

    // A recent write does not refresh the older signed package timestamp.
    fixture.push(bridge::old_payload()).await?;
    let old = fixture.stored().await?;
    assert_eq!(old.len(), 2);
    for entry in &old {
        assert_eq!(
            entry.data.package_timestamp,
            Nanoseconds::from_ms(OLD_TIMESTAMP_MS)
        );
    }
    fixture.run(&verify).await?;
    fixture.run(&["spec", "check", spec_path]).await?;
    let checks = fixture.checks(false, true).await?;
    assert_selected(&checks, "provider");
    assert_stored(&checks, true);
    check::gate(&checks, "fixture", "stale storage remains diagnostic")?;
    assert_eq!(fixture.stored().await?, old);

    let count = fixture.bridge.request_count()?;
    let startups = fixture.bridge.startup_count()?;
    let chain_checks = fixture.checks(true, true).await?;
    assert_stored(&chain_checks, true);
    for id in ["oracle.aggregate.collateral", "oracle.aggregate.borrow"] {
        assert!(
            matches!(status(&chain_checks, id), Status::Failed { .. }),
            "{chain_checks:?}"
        );
    }
    assert!(check::gate(&chain_checks, "fixture", "chain prices are stale").is_err());
    assert!(fixture
        .run(&["spec", "check", spec_path, "--prices-from", "chain"])
        .await
        .is_err());
    let mut chain_verify = verify.to_vec();
    chain_verify.extend(["--prices-from", "chain"]);
    assert!(fixture.run(&chain_verify).await.is_err());
    assert_eq!(
        fixture.bridge.request_count()?,
        count,
        "chain mode must not request provider input"
    );
    assert_eq!(
        fixture.bridge.startup_count()?,
        startups,
        "chain mode must not start the bridge"
    );
    assert_eq!(fixture.stored().await?, old);

    // Healthy storage cannot rescue failed provider acquisition. Conversely,
    // chain mode remains usable and never starts the failing provider bridge.
    fixture.push(bridge::new_payload()).await?;
    let fresh = fixture.stored().await?;
    assert_eq!(fresh.len(), 2);
    for entry in &fresh {
        assert_eq!(
            entry.data.package_timestamp,
            Nanoseconds::from_ms(NEW_TIMESTAMP_MS)
        );
    }
    fixture.bridge.set_failure(true)?;
    let failed = fixture.checks(false, true).await?;
    assert_stored(&failed, false);
    for id in ["oracle.price.collateral.0", "oracle.price.borrow.0"] {
        assert!(
            matches!(status(&failed, id), Status::Failed { .. }),
            "{failed:?}"
        );
        assert!(status(&failed, id).detail().contains("provider"));
    }
    assert!(check::gate(&failed, "fixture", "provider has no stored fallback").is_err());
    assert!(fixture.run(&["spec", "check", spec_path]).await.is_err());
    assert!(fixture.run(&verify).await.is_err());
    assert_eq!(fixture.stored().await?, fresh);
    let count = fixture.bridge.request_count()?;
    let startups = fixture.bridge.startup_count()?;
    let checks = fixture.checks(true, true).await?;
    assert_selected(&checks, "chain");
    assert_stored(&checks, false);
    check::gate(&checks, "fixture", "fresh chain inputs pass")?;
    fixture
        .run(&["spec", "check", spec_path, "--prices-from", "chain"])
        .await?;
    fixture.run(&chain_verify).await?;
    assert_eq!(fixture.bridge.request_count()?, count);
    assert_eq!(
        fixture.bridge.startup_count()?,
        startups,
        "chain mode must not start the bridge"
    );
    assert_eq!(fixture.stored().await?, fresh);
    Ok(())
}
