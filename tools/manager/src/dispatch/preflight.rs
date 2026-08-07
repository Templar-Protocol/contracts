//! On-chain preflight for a deployment spec: read-only checks that run before
//! any transaction.
//!
//! IO lives here rather than in `spec`, which stays offline so its checks and
//! `market export` can be unit-tested without a network.

use anyhow::Context as _;
use near_account_id::AccountId;
use templar_common::asset::{AssetClass, FungibleAsset};
use templar_common::oracle::pyth::PriceIdentifier;
use templar_common::price::PricePair;
use templar_gateway_core::GatewayError;
use templar_gateway_methods_spec::{account, contract, registry};
use templar_gateway_types::common::{ContractArgs, Pagination};
use templar_proxy_oracle_kernel::Price;
use templar_proxy_oracle_near_common::convert::{
    pyth_price_try_from_kernel, pyth_price_try_to_kernel,
};

use crate::commands::spec::Check as CheckArgs;
use crate::context::{print_json, CliContext};
use crate::report::Reporter;
use crate::spec::{
    check::{reconcile_decimals, Check, OnChainDecimals, Status},
    oracle::{AssetSpec, SourceSpec},
    MarketSpec,
};

/// `spec check` — load a spec, run its checks, and report. Fails when any check
/// failed, so it is usable as a gate in CI or a pre-deploy script.
pub(super) async fn check(ctx: CliContext, args: CheckArgs) -> anyhow::Result<()> {
    let mut spec = crate::spec::extends::load(&args.path)?;
    let mut reporter = ctx.reporter(&args.skip_check);
    run_all(
        &ctx,
        &mut spec,
        args.offline,
        args.accept_decimals_mismatch,
        None,
        &mut reporter,
    )
    .await?;
    reporter.ensure_every_skip_matched()?;
    reporter.digest();
    let checks = reporter.into_checks();

    let price_maximum_age = spec.market.price_maximum_age;
    let market_id = spec.market_id()?;
    print_json(&crate::spec::check::Report {
        subject: CheckedSpec {
            oracle_id: spec.reads_oracle_id()?,
            governance_id: spec.own_governance_id()?,
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
    governance_id: Option<AccountId>,
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
    reporter: &mut Reporter,
) -> anyhow::Result<()> {
    if offline {
        reporter.phase("offline only");
        reporter.record(Check::new(
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
        run(
            ctx,
            spec,
            accept_decimals_mismatch,
            deployed_oracle,
            reporter,
        )
        .await;
    }
    // Offline checks run last so they see those decimals.
    reporter.phase("the spec itself");
    reporter.extend(crate::spec::check::run_offline(spec));
    Ok(())
}

/// Every check that needs the chain, in a stable order. Nothing propagates: a
/// failed read is a failed *check*, so one RPC error cannot hide the rest.
async fn run(
    ctx: &CliContext,
    spec: &mut MarketSpec,
    accept_mismatch: bool,
    deployed_oracle: Option<&AccountId>,
    reporter: &mut Reporter,
) {
    reporter.phase("assets and their sources");
    asset_checks(
        ctx,
        "collateral",
        &mut spec.collateral,
        accept_mismatch,
        reporter,
    )
    .await;
    asset_checks(ctx, "borrow", &mut spec.borrow, accept_mismatch, reporter).await;

    reporter.phase("registry versions");
    reporter.extend(versions(ctx, spec).await);

    let (direct_checks, direct_collateral, direct_borrow) = direct_oracle(ctx, spec).await;
    if !direct_checks.is_empty() {
        reporter.phase("the oracle this market reads");
        reporter.extend(direct_checks);
    }

    reporter.phase("yield recipients");
    accounts(ctx, spec, reporter).await;

    // Aggregation before the cross-check: it produces the prices the reference
    // source is compared against.
    reporter.phase("price aggregation");
    let (collateral, borrow) = super::aggregate::checks(ctx, spec, deployed_oracle, reporter).await;

    // Exactly one mode produces prices: a proxy market from the aggregation
    // dry-run, a direct one from the call `oracle.serves_pair` already made. The
    // cross-check is the only thing that judges the *value* rather than its
    // presence, so a direct market needs it just as much.
    let (collateral, borrow) = (collateral.or(direct_collateral), borrow.or(direct_borrow));
    reporter.record(Check::new(
        "oracle.prices_are_usable",
        prices_are_usable(
            Leg {
                price: collateral.as_ref(),
                decimals: spec.collateral.decimals,
            },
            Leg {
                price: borrow.as_ref(),
                decimals: spec.borrow.decimals,
            },
        ),
    ));

    reporter.phase("reference prices");
    match super::reference::CoinGecko::from_env() {
        Ok(source) => {
            reporter.extend(super::reference::checks(&source, spec, collateral, borrow).await);
        }
        // Failing to build a client is "could not check", like every other
        // reference-source problem.
        Err(error) => reporter.record(Check::new(
            "reference.price.all",
            Status::Skipped {
                reason: format!("no reference price source: {error:#}"),
            },
        )),
    }
}

/// One side of the pair as preflight resolved it. Either half can be absent:
/// the checks that report those failures have already run.
#[derive(Clone, Copy)]
struct Leg<'a> {
    price: Option<&'a Price>,
    decimals: Option<u8>,
}

/// Whether the market could build a `PricePair` out of what the oracle said.
///
/// The kernel represents prices the market rejects: negative or zero, a
/// confidence interval at least as wide as the price, an exponent that
/// underflows the token's decimals. Both modes otherwise report their feeds
/// healthy for a market on which every price-dependent operation fails.
fn prices_are_usable(collateral: Leg, borrow: Leg) -> Status {
    let skipped = |reason: &str| Status::Skipped {
        reason: reason.to_owned(),
    };

    let (Some(collateral_price), Some(borrow_price)) = (collateral.price, borrow.price) else {
        return skipped("no pair to build: a leg produced no price, which its own check reports");
    };
    let (Some(collateral_decimals), Some(borrow_decimals)) = (collateral.decimals, borrow.decimals)
    else {
        return skipped("decimals are unresolved, and they set the exponent this would check");
    };
    let (Some(collateral_price), Some(borrow_price)) = (
        pyth_price_try_from_kernel(collateral_price),
        pyth_price_try_from_kernel(borrow_price),
    ) else {
        return Status::failed(
            "a resolved price carries a publish time outside Pyth's range".to_owned(),
        );
    };

    match PricePair::new(
        &collateral_price,
        i32::from(collateral_decimals),
        &borrow_price,
        i32::from(borrow_decimals),
    ) {
        Ok(_) => Status::passed("both prices convert to the market's `PricePair`".to_owned()),
        Err(error) => Status::failed(format!(
            "`{error}` — the oracle answered, but the market would reject what it \
             said, so every price-dependent operation would fail."
        )),
    }
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
    reporter: &mut Reporter,
) {
    let contract_id = spec.asset.contract_id().to_owned();
    reporter.record(Check::new(
        format!("asset.exists.{side}"),
        exists_check(
            ctx,
            &contract_id,
            || Status::passed(spec.asset.to_string()),
            || Status::failed(format!("`{contract_id}` does not exist")),
        )
        .await,
    ));

    let (status, resolved) = match underlying_decimals(ctx, side, &spec.asset).await {
        Ok(on_chain) => reconcile_decimals(side, spec.decimals, on_chain, accept_mismatch),
        Err(status) => (status, None),
    };
    spec.decimals = resolved;
    reporter.record(Check::new(format!("asset.decimals.{side}"), status));

    for (index, source) in spec.sources.iter().enumerate() {
        reporter.record(Check::new(
            format!("oracle.source.{side}.{index}"),
            source_status(ctx, source).await,
        ));
    }
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

/// Every version key must already be registered *and still deployable*, or the deploy fails
/// partway.
///
/// Against a registry too old to serve `get_version` this falls back to membership, which cannot
/// see that `remove_version` cleared a version's code — such a key passes here and fails
/// mid-deploy, after the governance and oracle steps have already run.
async fn versions(ctx: &CliContext, spec: &MarketSpec) -> Vec<Check> {
    // A direct market deploys only itself, so the proxy versions it never uses
    // need not be registered.
    let mut labeled = vec![("market", &spec.market_version)];
    if let Some((_, oracle_version, governance_version)) = spec.proxy() {
        labeled.push(("oracle", oracle_version));
        labeled.push(("governance", governance_version));
    }

    if serves_entry_and_version_views(ctx, &spec.registry).await {
        let mut checks = Vec::with_capacity(labeled.len());
        for (label, key) in labeled {
            let status = match ctx
                .client
                .read(registry::GetVersion {
                    registry_id: spec.registry.clone(),
                    version_key: key.clone(),
                })
                .await
            {
                Ok(result) => match result.version {
                    Some(info) if info.availability.is_deployable() => Status::passed(key.clone()),
                    Some(_) => Status::failed(format!(
                        "`{key}`'s code was removed from {}; the deploy would fail partway",
                        spec.registry,
                    )),
                    None => Status::failed(format!(
                        "`{key}` is not registered in {}; the deploy would fail partway",
                        spec.registry,
                    )),
                },
                Err(error) => Status::failed(format!(
                    "could not read `{key}` in {}: {error}",
                    spec.registry,
                )),
            };
            checks.push(Check::new(format!("registry.version.{label}"), status));
        }
        return checks;
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

/// A direct market skips the three checks a proxy gets, which left the oracle
/// it does read checked by nothing. `MarketConfiguration` is immutable after
/// init, so a mistyped `price_id` would be permanent.
async fn direct_oracle(
    ctx: &CliContext,
    spec: &MarketSpec,
) -> (Vec<Check>, Option<Price>, Option<Price>) {
    let crate::spec::OracleMode::Direct { account_id } = &spec.oracle else {
        return (Vec::new(), None, None);
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
        return (checks, None, None);
    };
    let (status, collateral_price, borrow_price) =
        serves_pair(ctx, spec, account_id, collateral, borrow).await;
    checks.push(Check::new("oracle.serves_pair", status));
    (checks, collateral_price, borrow_price)
}

/// Ask the configured oracle for the pair, through the gateway's own resolution.
///
/// `oracle.getPrices` resolves the *configuration* — an LST wrapper's
/// exchange-rate transform, a proxy's sources and aggregator, a plain oracle's
/// read — rather than calling the contract's price view. Deliberately: a proxy
/// serves a cached price gated on breakers and freshness, and neither is a
/// misconfiguration. Requiring them here would make writing a plan depend on
/// someone having called `update_prices` first.
///
/// The prices come back too: the reference cross-check is the only thing that
/// judges what a feed *says* rather than that it answered.
async fn serves_pair(
    ctx: &CliContext,
    spec: &MarketSpec,
    oracle_id: &AccountId,
    collateral: PriceIdentifier,
    borrow: PriceIdentifier,
) -> (Status, Option<Price>, Option<Price>) {
    let age = spec.market.price_maximum_age.as_secs();

    let resolved = match ctx
        .client
        .read(templar_gateway_methods_spec::oracle::GetPrices {
            oracle_id: oracle_id.clone(),
            price_ids: vec![collateral, borrow],
            age,
        })
        .await
    {
        Ok(result) => result.prices,
        Err(error) => {
            return (
                Status::failed(format!(
                    "`{oracle_id}` did not answer the market's price call: {error}. \
                     A mistyped `price_id` cannot be corrected after init."
                )),
                None,
                None,
            )
        }
    };

    let (mut unpriced, mut unrepresentable, mut found) = (Vec::new(), Vec::new(), Vec::new());
    for (side, id) in [("collateral", collateral), ("borrow", borrow)] {
        let named = format!("{side} ({id})");
        match resolved
            .iter()
            .find(|entry| entry.price_id == id)
            .and_then(|entry| entry.price.as_ref())
        {
            None => unpriced.push(named),
            // A price the kernel cannot represent is not a price. Flattening it
            // away would report the feed healthy and drop it from the
            // cross-check in the same breath.
            Some(price) => match pyth_price_try_to_kernel(price) {
                Some(price) => found.push(price),
                None => unrepresentable.push(named),
            },
        }
    }

    if !unpriced.is_empty() {
        return (
            Status::failed(format!(
                "`{oracle_id}` resolves no price within {age}s for {}. Either the \
                 identifier is wrong — which cannot be corrected after init — or \
                 nothing has published to it recently.",
                unpriced.join(" and "),
            )),
            None,
            None,
        );
    }
    if !unrepresentable.is_empty() {
        return (
            Status::failed(format!(
                "`{oracle_id}` returned a price for {} that does not fit the \
                 kernel's representation, so the market could not consume it.",
                unrepresentable.join(" and "),
            )),
            None,
            None,
        );
    }

    let mut found = found.into_iter();
    (
        Status::passed(format!("both feeds priced within {age}s by `{oracle_id}`")),
        found.next(),
        found.next(),
    )
}

/// Yield recipients must exist, or that share of yield is unclaimable.
async fn accounts(ctx: &CliContext, spec: &MarketSpec, reporter: &mut Reporter) {
    let protocol = account_check(ctx, "protocol", &spec.market.protocol_account_id).await;
    reporter.record(protocol);

    // Check ids are a contract — `--skip-check` and the plan artifact key on
    // them — so each recipient gets its own, in a stable order rather than
    // `HashMap`'s.
    let mut recipients: Vec<_> = spec.market.yield_weights.r#static.keys().collect();
    recipients.sort();
    for account_id in recipients {
        let check = account_check(ctx, &format!("yield_static.{account_id}"), account_id).await;
        reporter.record(check);
    }
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

/// Whether `registry_id` serves the views that make the target and version checks sound.
///
/// Unreadable or unparseable counts as "no". Every registry deployed so far predates these
/// views, so the checks that want them have to degrade to what any registry can answer — failing
/// closed would refuse to plan against all of them.
pub(super) async fn serves_entry_and_version_views(
    ctx: &CliContext,
    registry_id: &AccountId,
) -> bool {
    ctx.client
        .read(contract::GetVersion {
            contract_id: registry_id.clone(),
        })
        .await
        .ok()
        .and_then(|result| result.parsed)
        .is_some_and(|version| {
            version
                .cast::<templar_gateway_types::Registry>()
                .supports_entry_and_version_views()
        })
}

#[cfg(test)]
mod tests {
    use super::{prices_are_usable, underlying_ft, Leg, Price, Status};
    use templar_common::asset::{CollateralAsset, FungibleAsset};

    fn price(value: i64, conf: u64) -> Price {
        Price {
            price: value,
            conf,
            expo: -8,
            publish_time_ns: templar_common::Nanoseconds::from_ns(1_700_000_000_000_000_000),
        }
    }

    fn leg(price: &Price) -> Leg<'_> {
        Leg {
            price: Some(price),
            decimals: Some(6),
        }
    }

    /// The kernel's `Price` is laxer than the market's `PricePair`, so a feed can
    /// answer, satisfy `oracle.serves_pair`, and still leave a market on which
    /// every price-dependent operation fails.
    #[test]
    fn a_price_the_market_would_reject_fails_the_check() {
        let healthy = price(3_000_000_000, 1_000_000);
        assert!(
            matches!(
                prices_are_usable(leg(&healthy), leg(&healthy)),
                Status::Passed { .. }
            ),
            "a well-formed pair must pass, or the rejections below prove nothing"
        );

        // Zero satisfies `conf >= price` on its own, so an unpublished feed
        // reporting nothing looks identical to one reporting a real price.
        for (label, bad) in [
            ("negative", price(-1, 0)),
            ("zero", price(0, 0)),
            ("confidence wider than the price", price(100, 100)),
        ] {
            assert!(
                matches!(
                    prices_are_usable(leg(&bad), leg(&healthy)),
                    Status::Failed { .. }
                ),
                "a {label} price must fail"
            );
        }
    }

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
