//! On-chain preflight for a deployment spec: read-only checks that run before
//! any transaction.
//!
//! IO lives here rather than in `spec`, which stays offline so its checks and
//! `market export` can be unit-tested without a network.

use anyhow::Context as _;
use near_account_id::AccountId;
use templar_common::asset::{AssetClass, FungibleAsset};
use templar_gateway_core::GatewayError;
use templar_gateway_methods_spec::{account, contract, registry};
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
    let checks = run_all(
        &ctx,
        &mut spec,
        args.offline,
        args.accept_decimals_mismatch,
        None,
    )
    .await?;

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

    let failed = crate::spec::check::failures(&checks);
    anyhow::ensure!(failed == 0, "{failed} check(s) failed");
    Ok(())
}

/// Every check, online then offline, writing resolved decimals back into the
/// spec as it goes.
///
/// Shared with `market plan` (ENG-544), which embeds the same results in its
/// artifact — running a *different* set of checks there would let a plan be
/// written for a spec `spec check` rejects.
pub(super) async fn run_all(
    ctx: &CliContext,
    spec: &mut MarketSpec,
    offline: bool,
    accept_decimals_mismatch: bool,
    deployed_oracle: Option<&AccountId>,
) -> anyhow::Result<Vec<Check>> {
    let mut checks = Vec::new();

    if offline {
        checks.push(Check::new(
            "preflight.online",
            Status::Skipped {
                reason: "--offline".to_owned(),
            },
        ));
    } else {
        // The spec names its own chain, and the CLI defaults to testnet. Reading
        // a mainnet spec against testnet would report every account and version
        // as missing — a page of confident, entirely wrong failures.
        let declared = spec.network()?;
        anyhow::ensure!(
            declared == ctx.network(),
            "this spec is for {declared} (its registry is `{}`), but the CLI is \
             pointed at {}. Re-run with `--network {declared}`.",
            spec.registry,
            ctx.network(),
        );

        // Online first, writing resolved decimals back into the spec. Otherwise
        // `config.validate` reports itself skipped for want of decimals this
        // very run just read off the chain.
        checks.extend(run(ctx, spec, accept_decimals_mismatch, deployed_oracle).await);
    }
    // Offline checks run last so they see those decimals.
    checks.extend(crate::spec::check::run_offline(spec));
    Ok(checks)
}

/// Every check that needs the chain, in a stable order so two runs of the same
/// spec produce comparable reports. Takes the spec mutably to write back the
/// decimals it resolves.
/// Nothing here propagates: a read that fails is a *check* that failed, and the
/// operator still gets the rest of the report. Aborting on the first RPC error
/// would hide every other problem in the spec.
async fn run(
    ctx: &CliContext,
    spec: &mut MarketSpec,
    accept_mismatch: bool,
    deployed_oracle: Option<&AccountId>,
) -> Vec<Check> {
    let mut checks = Vec::new();
    checks.extend(asset_checks(ctx, "collateral", &mut spec.collateral, accept_mismatch).await);
    checks.extend(asset_checks(ctx, "borrow", &mut spec.borrow, accept_mismatch).await);
    checks.extend(versions(ctx, spec).await);
    checks.extend(accounts(ctx, spec).await);
    // Aggregation before the cross-check: it produces the prices the reference
    // source is compared against.
    let (aggregate_checks, collateral, borrow) =
        super::aggregate::checks(ctx, spec, deployed_oracle).await;
    checks.extend(aggregate_checks);

    match super::reference::CoinGecko::from_env() {
        Ok(source) => {
            checks.extend(super::reference::checks(&source, spec, collateral, borrow).await);
        }
        // Failing to build a client is "could not check", like every other
        // reference-source problem.
        Err(error) => checks.push(Check::new(
            "reference.price",
            Status::Skipped {
                reason: format!("no reference price source: {error:#}"),
            },
        )),
    }
    checks
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

    let (status, resolved) = match underlying_decimals(ctx, side, &spec.asset).await {
        Ok(on_chain) => reconcile_decimals(side, spec.decimals, on_chain, accept_mismatch),
        Err(status) => (status, None),
    };
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

/// The adapter must exist. Whether it currently carries a price is reported by
/// `oracle.price.*` in the aggregation dry-run, which fetches it anyway — asking
/// here as well would round-trip every source twice.
async fn source_status(ctx: &CliContext, source: &SourceSpec) -> Status {
    let oracle_id = source.oracle_id();
    match exists(ctx, oracle_id).await {
        Ok(true) => Status::passed(format!("{} on {oracle_id}", source.describe())),
        Ok(false) => Status::failed(format!("adapter `{oracle_id}` does not exist")),
        Err(error) => Status::failed(format!("{error:#}")),
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

/// What the chain says about the decimals of the token that defines them, or a
/// failed check.
///
/// `asset.exists` only covers `contract_id`, which for a NEP-245 asset is the
/// *wrapper* — `intents.near` — not the token named inside its token id. So this
/// is the only place the underlying account is ever touched, and swallowing its
/// errors here means a mistyped bridge address produces an all-green report.
/// That is the worst outcome the preflight can have, so:
///
/// - a token id naming an account that does not exist is a **failure**, not
///   "unverified";
/// - any other read failure is a **failure**, not "this token publishes no
///   metadata";
/// - only a real account with absent or unparseable metadata is `Unavailable`,
///   which is the case the spec's `decimals` override exists for.
async fn underlying_decimals<A: AssetClass>(
    ctx: &CliContext,
    side: &str,
    asset: &FungibleAsset<A>,
) -> Result<OnChainDecimals, Status> {
    // An opaque NEP-245 token id names no NEP-141 account; nothing on chain can
    // answer, and the override is the only source.
    let Some(account_id) = underlying_ft(asset) else {
        return Ok(OnChainDecimals::Unavailable);
    };

    match exists(ctx, &account_id).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(Status::failed(format!(
                "the {side} token id names `{account_id}`, which does not exist. \
                 Check the asset string — `asset.exists.{side}` only covers the \
                 NEP-245 wrapper, not the token inside it."
            )))
        }
        Err(error) => return Err(Status::failed(format!("{error:#}"))),
    }

    match ft_decimals(ctx, &account_id).await {
        Ok(Some(decimals)) => Ok(OnChainDecimals::Known(decimals)),
        Ok(None) => Ok(OnChainDecimals::Unavailable),
        Err(error) => Err(Status::failed(format!(
            "could not read decimals from `{account_id}`: {error:#}. This is \
             inconclusive, not evidence the token publishes no metadata."
        ))),
    }
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
///
/// Membership is all this can check. `remove_version` soft-deletes: it sets
/// `VersionEntry::Code.code = None` but keeps the key, and `code_hash()` still
/// answers with the stored hash — so `list_versions` and `get_version_code_hash`
/// both report a removed version as present, while `deploy` aborts with
/// "Version code has been deleted". No registry view distinguishes the two, so
/// closing this needs an additive view reporting code availability (the same
/// contract change ENG-463/464 wants for ABI validation). Until then a
/// soft-deleted version passes here and fails mid-deploy.
async fn versions(ctx: &CliContext, spec: &MarketSpec) -> Vec<Check> {
    // A direct market deploys only itself, so the proxy versions it never uses
    // need not be registered.
    let labelled: Vec<_> = if spec.oracle.is_direct() {
        vec![("market", &spec.versions.market)]
    } else {
        vec![
            ("market", &spec.versions.market),
            ("oracle", &spec.versions.proxy_oracle),
            ("governance", &spec.versions.proxy_governance),
        ]
    };

    let registered = match ctx
        .client
        .read(registry::ListVersions {
            registry_id: spec.registry.clone(),
            args: Pagination::default(),
        })
        .await
    {
        Ok(registered) => registered.values,
        // One failed read must not swallow the rest of the report.
        Err(error) => {
            return labelled
                .into_iter()
                .map(|(label, _)| {
                    Check::new(
                        format!("registry.version.{label}"),
                        Status::failed(format!(
                            "could not list versions in {}: {error}",
                            spec.registry
                        )),
                    )
                })
                .collect()
        }
    };

    labelled
        .into_iter()
        .map(|(label, key)| {
            Check::new(
                format!("registry.version.{label}"),
                if registered.iter().any(|known| known == key) {
                    Status::passed(key.clone())
                } else {
                    Status::failed(format!(
                        "`{key}` is not registered in {}; the deploy would fail partway",
                        spec.registry
                    ))
                },
            )
        })
        .collect()
}

/// Yield recipients must exist, or that share of yield is unclaimable.
async fn accounts(ctx: &CliContext, spec: &MarketSpec) -> Vec<Check> {
    let mut checks = vec![account_check(ctx, "protocol", &spec.market.protocol_account_id).await];

    // Check ids are a contract — `--skip-check` and the plan artifact key on
    // them — so each recipient gets its own, and they are emitted in a stable
    // order rather than `HashMap`'s.
    let mut recipients: Vec<_> = spec.market.yield_weights.r#static.keys().collect();
    recipients.sort();
    for account_id in recipients {
        checks.push(account_check(ctx, &format!("yield_static.{account_id}"), account_id).await);
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
pub(super) async fn exists(ctx: &CliContext, account_id: &AccountId) -> anyhow::Result<bool> {
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
