//! On-chain preflight for a deployment spec: read-only checks that run before
//! any transaction.
//!
//! IO lives here rather than in `spec`, which stays offline so its checks and
//! `market export` can be unit-tested without a network.

use anyhow::Context as _;
use near_account_id::AccountId;
use templar_common::asset::{AssetClass, FungibleAsset};
use templar_gateway_core::GatewayError;
use templar_gateway_methods_spec::{account, contract, redstone, registry};
use templar_gateway_types::common::{ContractArgs, Pagination};

use crate::commands::spec::Check as CheckArgs;
use crate::context::{print_json, CliContext};
use crate::spec::{
    check::{reconcile_decimals, Check, OnChainDecimals, Status},
    oracle::{AssetSpec, SourceSpec},
    MarketSpec,
};

/// `spec check` — load a spec, run its checks, and report. Fails when any check
/// failed, so it is usable as a gate in CI or a pre-deploy script.
pub(super) async fn check(ctx: CliContext, args: CheckArgs) -> anyhow::Result<()> {
    let mut spec = crate::spec::extends::load(&args.path)?;
    let mut checks = Vec::new();

    if args.offline {
        checks.push(Check::new(
            "preflight.online",
            Status::Skipped {
                reason: "--offline".to_owned(),
            },
        ));
    } else {
        // Online first, writing resolved decimals back into the spec. Otherwise
        // `config.validate` reports itself skipped for want of decimals this
        // very run just read off the chain.
        checks.extend(run(&ctx, &mut spec, args.accept_decimals_mismatch).await?);
    }
    // Offline checks run last so they see those decimals.
    checks.extend(crate::spec::check::run_offline(&spec));

    let price_maximum_age = spec.market.price_maximum_age;
    print_json(&serde_json::json!({
        "market_id": spec.market_id()?,
        "oracle_id": spec.oracle_id()?,
        "governance_id": spec.governance_id()?,
        "network": spec.network()?.to_string(),
        "collateral_proxy": spec.collateral.clone().into_proxy(price_maximum_age),
        "borrow_proxy": spec.borrow.clone().into_proxy(price_maximum_age),
        "checks": checks,
    }))?;

    let failed = checks
        .iter()
        .filter(|check| check.status.is_failure())
        .count();
    anyhow::ensure!(failed == 0, "{failed} check(s) failed");
    Ok(())
}

/// Every check that needs the chain, in a stable order so two runs of the same
/// spec produce comparable reports. Takes the spec mutably to write back the
/// decimals it resolves.
async fn run(
    ctx: &CliContext,
    spec: &mut MarketSpec,
    accept_mismatch: bool,
) -> anyhow::Result<Vec<Check>> {
    let mut checks = Vec::new();
    checks.extend(asset_checks(ctx, "collateral", &mut spec.collateral, accept_mismatch).await);
    checks.extend(asset_checks(ctx, "borrow", &mut spec.borrow, accept_mismatch).await);
    checks.extend(versions(ctx, spec).await?);
    checks.extend(accounts(ctx, spec).await);
    Ok(checks)
}

/// Existence, decimals, and source checks for one side of the pair.
///
/// Generic over the asset class rather than erasing it: the typed accessors on
/// [`FungibleAsset`] identify the underlying token, and going through `Display`
/// instead would tie this to that format staying fixed.
async fn asset_checks<A: AssetClass>(
    ctx: &CliContext,
    side: &str,
    spec: &mut AssetSpec<A>,
    accept_mismatch: bool,
) -> Vec<Check> {
    let contract_id = spec.asset.contract_id().to_owned();
    let mut checks = vec![Check::new(
        format!("asset.exists.{side}"),
        match exists(ctx, &contract_id).await {
            Ok(true) => Status::passed(spec.asset.to_string()),
            Ok(false) => Status::failed(format!("`{contract_id}` does not exist")),
            Err(error) => Status::failed(format!("{error:#}")),
        },
    )];

    let on_chain = match underlying_ft(&spec.asset) {
        Some(account_id) => match ft_decimals(ctx, &account_id).await {
            Ok(Some(decimals)) => OnChainDecimals::Known(decimals),
            Ok(None) | Err(_) => OnChainDecimals::Unavailable,
        },
        None => OnChainDecimals::Unavailable,
    };
    let (status, resolved) = reconcile_decimals(side, spec.decimals, on_chain, accept_mismatch);
    spec.decimals = resolved;
    checks.push(Check::new(format!("asset.decimals.{side}"), status));

    for (index, source) in spec.sources.iter().enumerate() {
        checks.push(Check::new(
            format!("oracle.source.{side}.{index}"),
            source_status(ctx, source).await,
        ));
    }

    checks
}

/// The adapter must exist. Whether it currently *holds* a price for the feed is
/// reported, not required.
///
/// An adapter carries a feed once someone pushes one — Pyth Lazer will verify
/// any feed it signs, and RedStone any feed written to it. A market being
/// deployed for the first time may name a feed that has never appeared on this
/// adapter on this chain, which says nothing about whether the adapter supports
/// it. Failing on absence would block exactly the new-market case this pipeline
/// exists for, so absence is reported as unverified instead.
async fn source_status(ctx: &CliContext, source: &SourceSpec) -> Status {
    let oracle_id = source.oracle_id();
    match exists(ctx, oracle_id).await {
        Ok(false) => return Status::failed(format!("adapter `{oracle_id}` does not exist")),
        Err(error) => return Status::failed(format!("{error:#}")),
        Ok(true) => {}
    }

    match holds_price(ctx, source).await {
        Ok(true) => Status::passed(format!(
            "{} on {oracle_id}, currently carrying a price",
            describe(source)
        )),
        Ok(false) => Status::Skipped {
            reason: format!(
                "`{oracle_id}` holds no price for {} yet, which is expected for a feed \
                 not previously used here and does not mean it is unsupported. The \
                 aggregation dry-run cannot verify this feed until a price is pushed.",
                describe(source)
            ),
        },
        Err(error) => Status::failed(format!("{error:#}")),
    }
}

/// Whether the adapter currently holds a price for this feed.
async fn holds_price(ctx: &CliContext, source: &SourceSpec) -> anyhow::Result<bool> {
    match source {
        SourceSpec::Lazer {
            oracle, feed_id, ..
        } => {
            // No typed gateway method covers the Lazer adapter, so this uses the
            // documented generic escape hatch. `get_feed_data` answers `null`
            // for a feed it is not currently carrying.
            let result = ctx
                .client
                .read(contract::ViewFunction {
                    contract_id: oracle.clone(),
                    method_name: "get_feed_data".to_owned().into(),
                    args: ContractArgs::Json(serde_json::json!({ "feed_id": feed_id })),
                })
                .await
                .with_context(|| format!("read lazer feed {feed_id} from {oracle}"))?;
            Ok(!result.value.is_null())
        }
        SourceSpec::RedStone {
            oracle, price_id, ..
        } => {
            let result = ctx
                .client
                .read(redstone::ReadPriceData {
                    oracle_id: oracle.clone(),
                    feed_ids: vec![price_id.clone().into()],
                })
                .await
                .with_context(|| format!("read redstone `{price_id}` from {oracle}"))?;
            Ok(!result.entries.is_empty())
        }
    }
}

fn describe(source: &SourceSpec) -> String {
    match source {
        SourceSpec::Lazer { feed_id, .. } => format!("lazer feed {feed_id}"),
        SourceSpec::RedStone { price_id, .. } => format!("redstone `{price_id}`"),
    }
}

/// The NEP-141 account whose `ft_metadata` defines this asset's decimals.
///
/// A NEP-245 wrapper does not define them: `intents.near` holds many tokens, and
/// the value that matters belongs to the *underlying* one its token id names.
/// Where that id does not name a NEP-141 account, nothing on chain answers — and
/// that is precisely the case the spec's `decimals` override exists for.
fn underlying_ft<A: AssetClass>(asset: &FungibleAsset<A>) -> Option<AccountId> {
    if let Some(contract_id) = asset.clone().into_nep141() {
        return Some(contract_id);
    }
    asset
        .nep245_token_id()?
        .strip_prefix("nep141:")?
        .parse()
        .ok()
}

async fn ft_decimals(ctx: &CliContext, account_id: &AccountId) -> anyhow::Result<Option<u8>> {
    let result = ctx
        .client
        .read(contract::ViewFunction {
            contract_id: account_id.clone(),
            method_name: "ft_metadata".to_owned().into(),
            args: ContractArgs::Json(serde_json::json!({})),
        })
        .await
        .with_context(|| format!("read ft_metadata from {account_id}"))?;

    Ok(result
        .value
        .get("decimals")
        .and_then(serde_json::Value::as_u64)
        .and_then(|decimals| u8::try_from(decimals).ok()))
}

/// Every version key must already be registered, or the deploy fails partway —
/// which is how `deploy.sh` leaves an orphaned governance contract today.
async fn versions(ctx: &CliContext, spec: &MarketSpec) -> anyhow::Result<Vec<Check>> {
    let registered = ctx
        .client
        .read(registry::ListVersions {
            registry_id: spec.registry.clone(),
            args: Pagination::default(),
        })
        .await
        .with_context(|| format!("list versions in {}", spec.registry))?;

    Ok([
        ("market", &spec.versions.market),
        ("oracle", &spec.versions.proxy_oracle),
        ("governance", &spec.versions.proxy_governance),
    ]
    .into_iter()
    .map(|(label, key)| {
        Check::new(
            format!("registry.version.{label}"),
            if registered.values.iter().any(|known| known == key) {
                Status::passed(key.clone())
            } else {
                Status::failed(format!(
                    "`{key}` is not registered in {}; the deploy would fail partway",
                    spec.registry
                ))
            },
        )
    })
    .collect())
}

/// Yield recipients must exist, or that share of yield is unclaimable.
async fn accounts(ctx: &CliContext, spec: &MarketSpec) -> Vec<Check> {
    let mut checks = vec![account_check(ctx, "protocol", &spec.market.protocol_account_id).await];
    for account_id in spec.market.yield_weights.r#static.keys() {
        checks.push(account_check(ctx, "yield_static", account_id).await);
    }
    checks
}

async fn account_check(ctx: &CliContext, label: &str, account_id: &AccountId) -> Check {
    Check::new(
        format!("account.exists.{label}"),
        match exists(ctx, account_id).await {
            Ok(true) => Status::passed(account_id.to_string()),
            Ok(false) => Status::failed(format!("`{account_id}` does not exist")),
            Err(error) => Status::failed(format!("{error:#}")),
        },
    )
}

/// Whether the account exists.
///
/// Only `AccountNotFound` means "no"; every other failure propagates. A timed-out
/// RPC reported as "this account does not exist" would send an operator off to
/// create an account that is already there.
async fn exists(ctx: &CliContext, account_id: &AccountId) -> anyhow::Result<bool> {
    match ctx
        .client
        .read(account::Get {
            account_id: account_id.clone(),
        })
        .await
    {
        Ok(_) => Ok(true),
        Err(GatewayError::AccountNotFound(_)) => Ok(false),
        Err(error) => {
            Err(anyhow::Error::new(error).context(format!("look up account {account_id}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::underlying_ft;
    use templar_common::asset::{CollateralAsset, FungibleAsset};

    /// Which token defines an asset's decimals is not obvious, and getting it
    /// wrong silently mis-scales every price the market sees.
    #[test]
    fn resolves_the_token_that_defines_decimals() {
        let nep141: FungibleAsset<CollateralAsset> =
            "nep141:usdc.near".parse().expect("valid asset");
        assert_eq!(
            underlying_ft(&nep141).map(|id| id.to_string()),
            Some("usdc.near".to_owned())
        );

        // A NEP-245 wrapper does not define decimals; the token it names does.
        let wrapped: FungibleAsset<CollateralAsset> =
            "nep245:intents.near:nep141:eth-0xce6170.omft.near"
                .parse()
                .expect("valid asset");
        assert_eq!(
            underlying_ft(&wrapped).map(|id| id.to_string()),
            Some("eth-0xce6170.omft.near".to_owned())
        );

        // A NEP-245 token id that names no NEP-141 account: nothing on chain
        // answers, so the spec's `decimals` override is the only source.
        let opaque: FungibleAsset<CollateralAsset> =
            "nep245:intents.near:nep245:v2_1.omni.hot.tg:1100_abc"
                .parse()
                .expect("valid asset");
        assert_eq!(underlying_ft(&opaque), None);
    }
}
