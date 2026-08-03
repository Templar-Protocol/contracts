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
    let market_id = spec.market_id()?;
    print_json(&crate::spec::check::Report {
        subject: CheckedSpec {
            oracle_id: spec.reads_oracle_id()?,
            governance_id: spec.governance_id()?,
            network: spec.network()?.to_string(),
            collateral_proxy: spec.collateral.clone().into_proxy(price_maximum_age),
            borrow_proxy: spec.borrow.clone().into_proxy(price_maximum_age),
            market_id: market_id.clone(),
        },
        checks: &checks,
    })?;

    crate::spec::check::gate(&checks, market_id.as_str(), "the spec is not deployable")
}

/// What `spec check` reports alongside its checks: everything the spec derives,
/// so a reviewer sees the accounts and proxies a deploy would create.
#[derive(serde::Serialize)]
struct CheckedSpec {
    market_id: AccountId,
    oracle_id: AccountId,
    governance_id: AccountId,
    network: String,
    collateral_proxy:
        templar_proxy_oracle_kernel::proxy::Proxy<templar_proxy_oracle_near_common::input::Source>,
    borrow_proxy:
        templar_proxy_oracle_kernel::proxy::Proxy<templar_proxy_oracle_near_common::input::Source>,
}

/// Every check, online then offline, writing resolved decimals back into the
/// spec. Shared with `market plan`, which would otherwise write plans for specs
/// `spec check` rejects.
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

/// Every check that needs the chain, in a stable order. Nothing propagates: a
/// failed read is a failed *check*, so one RPC error cannot hide the rest.
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
    checks.extend(direct_oracle(ctx, spec).await);
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
            "reference.price.all",
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
        exists_check(
            ctx,
            &contract_id,
            || Status::passed(spec.asset.to_string()),
            || Status::failed(format!("`{contract_id}` does not exist")),
        )
        .await,
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
    exists_check(
        ctx,
        oracle_id,
        || Status::passed(format!("{} on {oracle_id}", source.describe())),
        || Status::failed(format!("adapter `{oracle_id}` does not exist")),
    )
    .await
}

/// The NEP-141 account whose `ft_metadata` defines this asset's decimals. A
/// NEP-245 wrapper does not: `intents.near` holds many tokens, and the value
/// belongs to the underlying one its token id names.
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

/// What the chain says about this asset's decimals, or a failed check.
///
/// The only place the *underlying* account is touched — `asset.exists` covers
/// the NEP-245 wrapper — so every read failure here is a failure, not
/// "unverified". Only a real account with unusable metadata is `Unavailable`.
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

/// Every version key must already be registered, or the deploy fails partway.
///
/// Membership is all this can check: `remove_version` soft-deletes, and no
/// registry view distinguishes that from a live version, so a soft-deleted one
/// passes here and fails mid-deploy. Closing it needs an additive view.
async fn versions(ctx: &CliContext, spec: &MarketSpec) -> Vec<Check> {
    // A direct market deploys only itself, so the proxy versions it never uses
    // need not be registered.
    let mut labeled = vec![("market", Some(spec.versions.market.clone()))];
    if !spec.oracle.is_direct() {
        labeled.push(("oracle", spec.versions.proxy_oracle.clone()));
        labeled.push(("governance", spec.versions.proxy_governance.clone()));
    }

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
            return labeled
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

    labeled
        .into_iter()
        .map(|(label, key)| {
            Check::new(
                format!("registry.version.{label}"),
                match key {
                    // A proxy spec that names no version for a contract it
                    // deploys cannot be planned; reported here rather than
                    // aborting the whole report.
                    None => Status::failed(format!(
                        "this spec deploys its own proxy oracle but states no \
                         `versions.proxy_{label}`"
                    )),
                    Some(key) if registered.iter().any(|known| *known == key) => {
                        Status::passed(key)
                    }
                    Some(key) => Status::failed(format!(
                        "`{key}` is not registered in {}; the deploy would fail partway",
                        spec.registry
                    )),
                },
            )
        })
        .collect()
}

/// Whether the contract simply has no such method, as opposed to rejecting the
/// call. Matched on the runtime's own wording, since `GatewayError` carries
/// contract failures as text.
fn is_missing_method(error: &templar_gateway_core::GatewayError) -> bool {
    let rendered = error.to_string();
    rendered.contains("MethodNotFound") || rendered.contains("doesn't exist")
}

/// A direct market skips the three checks a proxy gets, which left the oracle
/// it does read checked by nothing. `MarketConfiguration` is immutable after
/// init, so a mistyped `price_id` would be permanent.
async fn direct_oracle(ctx: &CliContext, spec: &MarketSpec) -> Vec<Check> {
    use templar_gateway_methods_spec::contract;
    use templar_gateway_types::common::ContractArgs;

    let crate::spec::OracleMode::Direct { account_id } = &spec.oracle else {
        return Vec::new();
    };

    let mut checks = vec![Check::new(
        "oracle.exists",
        exists_check(
            ctx,
            account_id,
            || Status::passed(account_id.to_string()),
            || {
                Status::failed(format!(
                    "`{account_id}` does not exist, so this market would read an \
                     oracle that is not there"
                ))
            },
        )
        .await,
    )];

    let Ok((collateral, borrow)) = spec.price_identifiers() else {
        return checks;
    };
    for (side, id) in [("collateral", collateral), ("borrow", borrow)] {
        let hex = hex::encode(id.0);
        checks.push(Check::new(
            format!("oracle.serves.{side}"),
            match ctx
                .client
                .read(contract::ViewFunction {
                    contract_id: account_id.clone(),
                    method_name: "get_price".to_owned().into(),
                    args: ContractArgs::Json(serde_json::json!({ "price_identifier": hex })),
                })
                .await
            {
                // A failure, not the Skipped a proxy source gets: this oracle
                // is not ours to configure, so the spec asserting an identifier
                // that is absent means the identifier is wrong.
                Ok(result) if result.value.is_null() => Status::failed(format!(
                    "`{account_id}` serves no price for {hex}. For an oracle this \
                     deployment does not configure, an unknown identifier is a \
                     wrong one — and it cannot be corrected after init."
                )),
                Ok(_) => Status::passed(format!("{hex} on {account_id}")),
                // Not every oracle answers `get_price`; a proxy oracle exposes
                // `price_feed_exists`. Falling through rather than giving up,
                // since three shipped specs read oracles of that kind.
                Err(error) if is_missing_method(&error) => {
                    match ctx
                        .client
                        .read(
                            templar_gateway_methods_spec::proxy_oracle::PriceFeedExists {
                                oracle_id: account_id.clone(),
                                price_identifier: id,
                            },
                        )
                        .await
                    {
                        Ok(result) if result.exists => {
                            Status::passed(format!("{hex} on {account_id} (proxy)"))
                        }
                        Ok(_) => Status::failed(format!(
                            "`{account_id}` serves no feed for {hex}. It cannot be \
                             corrected after init."
                        )),
                        Err(inner) if is_missing_method(&inner) => Status::Skipped {
                            reason: format!(
                                "`{account_id}` answers neither `get_price` nor \
                                 `price_feed_exists`, so this build cannot confirm \
                                 it serves {hex}. Check it by hand before \
                                 deploying — this is not a pass."
                            ),
                        },
                        Err(inner) => Status::failed(format!(
                            "`{account_id}` did not answer for {hex}: {inner}"
                        )),
                    }
                }
                Err(error) => Status::failed(format!(
                    "`{account_id}` did not answer for {hex}: {error}. A mistyped \
                     `price_id` cannot be corrected after init."
                )),
            },
        ));
    }
    checks
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

/// An existence check as a verdict, with the caller naming both outcomes.
///
/// Shared so the `Err` policy is decided once: a read that failed is a failed
/// *check*, never a pass. Four call sites had it written out separately, and
/// two of them read the polarity the other way round.
pub(super) async fn exists_check(
    ctx: &CliContext,
    account_id: &AccountId,
    present: impl FnOnce() -> Status,
    absent: impl FnOnce() -> Status,
) -> Status {
    match exists(ctx, account_id).await {
        Ok(true) => present(),
        Ok(false) => absent(),
        Err(error) => Status::failed(format!("{error:#}")),
    }
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
