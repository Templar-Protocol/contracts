//! `market export` — read a deployed market and reconstruct its spec.
//!
//! A multi-step read chain (the configuration, both proxies, three registry
//! deployment records), so it lives here rather than in `commands`, which stays
//! free of IO.

use anyhow::Context as _;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_common::Nanoseconds;
use templar_gateway_methods_spec::{
    market, proxy_oracle, proxy_oracle_governance as governance, registry,
};
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_governance_common::OperationKind;

use crate::commands::market::Export;
use crate::context::CliContext;
use crate::spec::{
    export::{split_market_id, Deployed},
    governance_account_id, GovernanceSpec, MarketSpec, Versions, BORROW_PRICE_ID,
    COLLATERAL_PRICE_ID,
};

pub(super) async fn market(ctx: CliContext, args: Export) -> anyhow::Result<()> {
    let spec = reconstruct(&ctx, &args.market_id, &args.governance_admin).await?;

    let rendered = toml::to_string_pretty(&spec).context("render spec as TOML")?;
    match &args.out {
        Some(path) => {
            std::fs::write(path, &rendered).with_context(|| format!("write {}", path.display()))?;
        }
        None => print!("{rendered}"),
    }
    Ok(())
}

/// The spec equivalent to what is deployed at `market_id`.
///
/// Shared with `market verify` (ENG-547), which re-runs the preflight against
/// it: verifying a *differently* reconstructed spec than `export` emits would
/// mean the two disagree about what the deployment is.
///
/// Reads the oracle's actual proxies, so a market whose feeds were never
/// configured fails here rather than verifying clean. That matters because
/// `admin_set_proxy` is dispatched detached: the proposal that should have set
/// them reports success even when the oracle rejected it, so on-chain state is
/// the only witness.
pub(super) async fn reconstruct(
    ctx: &CliContext,
    market_id: &AccountId,
    governance_admin: &AccountId,
) -> anyhow::Result<MarketSpec> {
    let (name, registry_id) = split_market_id(market_id)?;

    let configuration = ctx
        .client
        .read(market::GetConfiguration {
            market_id: market_id.clone(),
        })
        .await
        .context("read market configuration")?;

    let oracle = &configuration.price_oracle_configuration;
    let oracle_id = oracle.account_id.clone();
    let governance_id = governance_account_id(&name, &registry_id)?;

    // Which mode this market is in, decided from chain state before anything
    // proxy-shaped is read. A direct market's oracle is not ours: it has no
    // proxy to fetch, and the governance account beside it was never deployed,
    // so reading either fails on a market that is perfectly healthy.
    let proxy_mode = oracle_id == crate::spec::oracle_account_id(&name, &registry_id)?
        && oracle.collateral_asset_price_id == COLLATERAL_PRICE_ID
        && oracle.borrow_asset_price_id == BORROW_PRICE_ID;

    if !proxy_mode {
        return MarketSpec::from_deployed(Deployed {
            versions: Versions {
                market: version_key(ctx, &registry_id, market_id).await?,
                proxy_oracle: None,
                proxy_governance: None,
            },
            governance: None,
            collateral_proxy: None,
            borrow_proxy: None,
            market_id: market_id.clone(),
            configuration,
        });
    }

    MarketSpec::from_deployed(Deployed {
        versions: versions(ctx, &name, &registry_id, &oracle_id, market_id).await?,
        governance: Some(GovernanceSpec {
            admin: governance_admin.clone(),
            ttl_default: governance_ttl(ctx, &governance_id).await?,
        }),
        collateral_proxy: Some(proxy(ctx, &oracle_id, COLLATERAL_PRICE_ID).await?),
        borrow_proxy: Some(proxy(ctx, &oracle_id, BORROW_PRICE_ID).await?),
        market_id: market_id.clone(),
        configuration,
    })
}

/// The governance contract's default proposal TTL, read rather than assumed.
///
/// `GovernanceSpec` carries one TTL, but the contract stores one *per operation
/// kind*. Defaulting to `0s` would be silently destructive: re-deploying an
/// exported spec for a governance contract with a real timelock would remove
/// that timelock. So every kind is queried, a uniform value is recovered, and a
/// non-uniform set is refused rather than flattened.
async fn governance_ttl(
    ctx: &CliContext,
    governance_id: &AccountId,
) -> anyhow::Result<Nanoseconds> {
    use clap::ValueEnum as _;

    let mut uniform: Option<(OperationKind, Nanoseconds)> = None;
    for kind in OperationKind::value_variants() {
        let ttl = match ctx
            .client
            .read(governance::GetOperationTtl {
                governance_id: governance_id.clone(),
                kind: *kind,
            })
            .await
        {
            Ok(result) => result.ttl_ns,
            // A deployed contract older than this build does not know every
            // operation kind — `SelfUpgrade` postdates the governance running
            // on the alpha markets, and asking for its TTL panics the contract.
            // A kind the deployment does not have has no TTL to recover, which
            // is not the same as a failed read: matched on the contract's own
            // "unknown variant" so a genuine RPC failure still propagates.
            Err(error) if is_unknown_kind(&error, *kind) => continue,
            Err(error) => {
                return Err(anyhow::Error::new(error)
                    .context(format!("read {kind:?} TTL from {governance_id}")))
            }
        };

        match uniform {
            None => uniform = Some((*kind, ttl)),
            Some((first_kind, first)) => anyhow::ensure!(
                first == ttl,
                "`{governance_id}` uses per-operation TTLs ({first_kind:?} is {}ns, \
                 {kind:?} is {}ns). A spec carries a single `ttl_default` and cannot \
                 express that, so this market cannot be exported.",
                first.as_ns(),
                ttl.as_ns(),
            ),
        }
    }

    uniform
        .map(|(_, ttl)| ttl)
        .context("governance exposes no operation kinds to read a TTL from")
}

/// Operation kinds this build knows may postdate a deployed governance
/// contract, and whose absence is therefore expected rather than suspicious.
///
/// An allowlist, because skipping a kind removes it from the uniformity check
/// — and a *wrongly* skipped kind is how a per-operation timelock gets
/// flattened into a single `ttl_default` and then removed by the next deploy.
const POSTDATED_KINDS: [OperationKind; 1] = [OperationKind::SelfUpgrade];

/// Whether the contract rejected `kind` as a variant it does not know.
///
/// Narrow on both axes: only kinds known to postdate a deployment are
/// skippable, and the error must name the kind that was asked for. A bare
/// "unknown variant" match would also swallow a genuine query failure whose
/// message happened to contain that phrase, and every swallowed kind silently
/// widens what `ttl_default` claims to cover.
fn is_unknown_kind(error: &templar_gateway_core::GatewayError, kind: OperationKind) -> bool {
    if !POSTDATED_KINDS.contains(&kind) {
        return false;
    }
    let rendered = error.to_string();
    rendered.contains("unknown variant") && rendered.contains(&format!("{kind:?}"))
}

/// A configured proxy, or a legible error. An oracle serving neither constant is
/// not a proxy-oracle deployment, and the spec cannot express it.
async fn proxy(
    ctx: &CliContext,
    oracle_id: &AccountId,
    id: PriceIdentifier,
) -> anyhow::Result<Proxy<Source>> {
    ctx.client
        .read(proxy_oracle::GetProxy {
            oracle_id: oracle_id.clone(),
            id,
        })
        .await
        .with_context(|| format!("read proxy {} from {oracle_id}", hex::encode(id.0)))?
        .proxy
        .with_context(|| {
            format!(
                "`{oracle_id}` serves no proxy for {}; this market is not a \
                 proxy-oracle deployment and cannot be exported",
                hex::encode(id.0)
            )
        })
}

/// Version keys for the three contracts, from their registry deployment records.
async fn versions(
    ctx: &CliContext,
    name: &str,
    registry_id: &AccountId,
    oracle_id: &AccountId,
    market_id: &AccountId,
) -> anyhow::Result<Versions> {
    let governance_id = governance_account_id(name, registry_id)?;
    let versions = Versions {
        market: version_key(ctx, registry_id, market_id).await?,
        proxy_oracle: Some(version_key(ctx, registry_id, oracle_id).await?),
        proxy_governance: Some(version_key(ctx, registry_id, &governance_id).await?),
    };

    // A deployment record outlives its version, so a recovered key can name a
    // version the registry no longer offers at all. That case is caught here.
    //
    // What this does NOT catch is soft-deletion: `remove_version` only sets
    // `VersionEntry::Code.code = None`, leaving the key in the map and
    // `get_version_code_hash` still answering with the hash. No registry view
    // distinguishes that, so an exported spec can still name a version whose
    // redeployment fails with "Version code has been deleted" — after earlier
    // contracts in the deploy have already been created. Closing that needs a
    // registry view reporting code availability, which is a contract change.
    let live = ctx
        .client
        .read(registry::ListVersions {
            registry_id: registry_id.clone(),
            args: templar_gateway_types::common::Pagination::default(),
        })
        .await
        .with_context(|| format!("list versions in {registry_id}"))?
        .values;

    for (label, key) in [
        ("market", Some(&versions.market)),
        ("proxy_oracle", versions.proxy_oracle.as_ref()),
        ("proxy_governance", versions.proxy_governance.as_ref()),
    ]
    .into_iter()
    .filter_map(|(label, key)| key.map(|key| (label, key)))
    {
        anyhow::ensure!(
            live.iter().any(|known| known == key),
            "`{registry_id}` no longer offers the {label} version `{key}` this \
             deployment used; it has been removed. An exported spec naming it \
             could not be deployed, so pick a replacement version before exporting."
        );
    }

    Ok(versions)
}

async fn version_key(
    ctx: &CliContext,
    registry_id: &AccountId,
    account_id: &AccountId,
) -> anyhow::Result<String> {
    Ok(ctx
        .client
        .read(registry::GetDeployment {
            registry_id: registry_id.clone(),
            account_id: account_id.clone(),
        })
        .await
        .with_context(|| format!("read deployment record for {account_id}"))?
        .deployment
        .with_context(|| format!("`{registry_id}` has no deployment record for {account_id}"))?
        .version_key)
}

#[cfg(test)]
mod tests {
    use super::{is_unknown_kind, OperationKind};
    use templar_gateway_core::GatewayError;

    fn error(message: &str) -> GatewayError {
        GatewayError::NearQuery(message.to_owned())
    }

    /// The case the skip exists for: a governance contract older than this
    /// build does not know `SelfUpgrade`.
    #[test]
    fn a_postdated_kind_the_contract_rejects_is_skippable() {
        assert!(is_unknown_kind(
            &error("unknown variant `SelfUpgrade`, expected one of `SetProxy`, ..."),
            OperationKind::SelfUpgrade,
        ));
    }

    /// A kind that does not postdate any deployment is never absent by design,
    /// so its rejection is a real failure.
    #[test]
    fn a_kind_every_deployment_has_is_not_skippable() {
        assert!(!is_unknown_kind(
            &error("unknown variant `SetProxy`, expected one of ..."),
            OperationKind::SetProxy,
        ));
    }

    /// An error that does not name the kind asked for is a different failure,
    /// and swallowing it would drop that kind from the uniformity check.
    #[test]
    fn an_error_naming_another_variant_is_not_skippable() {
        assert!(!is_unknown_kind(
            &error("unknown variant `Something`, expected one of ..."),
            OperationKind::SelfUpgrade,
        ));
    }

    /// Any other failure propagates, however it is worded.
    #[test]
    fn an_unrelated_failure_is_not_skippable() {
        assert!(!is_unknown_kind(
            &error("timed out talking to the RPC"),
            OperationKind::SelfUpgrade,
        ));
    }
}
