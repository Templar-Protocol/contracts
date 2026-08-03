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

    // Here rather than in `reconstruct`: `market verify` shares that function and
    // reports the same fact as a `governance.admin` check, so failing hard inside
    // it would turn a monitoring run into an abort. An export is a record, and a
    // record naming the wrong admin is worse than none.
    if spec.governance.is_some() {
        ensure_admin_holds_the_role(&ctx, &spec.governance_id()?, &args.governance_admin).await?;
    }

    let rendered = toml::to_string_pretty(&spec).context("render spec as TOML")?;
    match &args.out {
        Some(path) => {
            std::fs::write(path, &rendered).with_context(|| format!("write {}", path.display()))?;
        }
        None => print!("{rendered}"),
    }
    Ok(())
}

/// The spec equivalent to what is deployed at `market_id`, shared with `market
/// verify` so the two cannot disagree about what a deployment is.
///
/// Reads the oracle's actual proxies: `admin_set_proxy` is dispatched detached,
/// so on-chain state is the only witness that a feed was ever configured.
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

    // Decided before anything proxy-shaped is read or derived, since neither
    // exists for a direct market. An underivable proxy name proves direct mode.
    let proxy_mode = crate::spec::oracle_account_id(&name, &registry_id)
        .is_ok_and(|derived| derived == oracle_id)
        && oracle.collateral_asset_price_id == COLLATERAL_PRICE_ID
        && oracle.borrow_asset_price_id == BORROW_PRICE_ID;

    let (versions, governance, proxies) = if proxy_mode {
        let governance_id = governance_account_id(&name, &registry_id)?;
        (
            versions(ctx, &name, &registry_id, &oracle_id, market_id).await?,
            Some(GovernanceSpec {
                admin: governance_admin.clone(),
                ttl_default: governance_ttl(ctx, &governance_id).await?,
            }),
            Some((
                proxy(ctx, &oracle_id, COLLATERAL_PRICE_ID).await?,
                proxy(ctx, &oracle_id, BORROW_PRICE_ID).await?,
            )),
        )
    } else {
        (
            Versions {
                market: version_key(ctx, &registry_id, market_id).await?,
                proxy_oracle: None,
                proxy_governance: None,
            },
            None,
            None,
        )
    };

    let (collateral_proxy, borrow_proxy) = proxies.unzip();
    MarketSpec::from_deployed(Deployed {
        versions,
        governance,
        collateral_proxy,
        borrow_proxy,
        market_id: market_id.clone(),
        configuration,
    })
}

/// `--governance-admin` is supplied, not read, so it is checked against the role
/// before it enters a spec that presents itself as a record of what is deployed.
/// A typo would otherwise re-deploy control to the wrong account.
async fn ensure_admin_holds_the_role(
    ctx: &CliContext,
    governance_id: &AccountId,
    admin: &AccountId,
) -> anyhow::Result<()> {
    let holders = ctx
        .client
        .read(governance::ListRole {
            governance_id: governance_id.clone(),
            role: templar_proxy_oracle_near_governance_common::Role::Admin,
            offset: None,
            count: None,
        })
        .await
        .with_context(|| format!("list the Admin role on {governance_id}"))?
        .members;

    anyhow::ensure!(
        holders.contains(admin),
        "`{admin}` does not hold Admin on `{governance_id}`; it is held by {}. \
         Re-run `--governance-admin` with one of those.",
        if holders.is_empty() {
            "nobody".to_owned()
        } else {
            holders
                .iter()
                .map(|holder| format!("`{holder}`"))
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    Ok(())
}

/// The governance contract's default proposal TTL. Stored per operation kind,
/// so every kind is queried and a non-uniform set refused: flattening one would
/// remove a real timelock on the next deploy.
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
            // A kind the deployment predates has no TTL to recover, which is
            // not the same as a failed read.
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

/// Operation kinds that may postdate a deployed governance contract. An
/// allowlist, because a wrongly skipped kind drops out of the uniformity check
/// and its timelock with it.
const POSTDATED_KINDS: [OperationKind; 1] = [OperationKind::SelfUpgrade];

/// Whether the contract rejected `kind` as a variant it does not know. Narrow
/// on both axes — allowlisted kind, and the error must name it — because a bare
/// phrase match would swallow genuine query failures.
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

    // Catches a version the registry no longer offers, but not soft-deletion:
    // `remove_version` keeps the key and the hash, and no view distinguishes
    // that from a live version.
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
