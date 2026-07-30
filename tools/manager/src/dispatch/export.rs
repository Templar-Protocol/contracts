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
    let (name, registry_id) = split_market_id(&args.market_id)?;

    let configuration = ctx
        .client
        .read(market::GetConfiguration {
            market_id: args.market_id.clone(),
        })
        .await
        .context("read market configuration")?;

    let oracle_id = configuration.price_oracle_configuration.account_id.clone();
    let governance_id = governance_account_id(&name, &registry_id)?;
    let spec = MarketSpec::from_deployed(Deployed {
        versions: versions(&ctx, &name, &registry_id, &oracle_id, &args.market_id).await?,
        governance: GovernanceSpec {
            admin: args.governance_admin.clone(),
            ttl_default: governance_ttl(&ctx, &governance_id).await?,
        },
        collateral_proxy: proxy(&ctx, &oracle_id, COLLATERAL_PRICE_ID).await?,
        borrow_proxy: proxy(&ctx, &oracle_id, BORROW_PRICE_ID).await?,
        market_id: args.market_id.clone(),
        configuration,
    })?;

    let rendered = toml::to_string_pretty(&spec).context("render spec as TOML")?;
    match &args.out {
        Some(path) => {
            std::fs::write(path, &rendered).with_context(|| format!("write {}", path.display()))?;
        }
        None => print!("{rendered}"),
    }
    Ok(())
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
        let ttl = ctx
            .client
            .read(governance::GetOperationTtl {
                governance_id: governance_id.clone(),
                kind: *kind,
            })
            .await
            .with_context(|| format!("read {kind:?} TTL from {governance_id}"))?
            .ttl_ns;

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
        proxy_oracle: version_key(ctx, registry_id, oracle_id).await?,
        proxy_governance: version_key(ctx, registry_id, &governance_id).await?,
    };

    // A deployment record outlives its version. `remove_version` soft-deletes,
    // clearing the code but leaving the record, so a version key can be recovered
    // faithfully and still be undeployable — the exported spec would fail with
    // "Version code has been deleted". Better to refuse than to emit it.
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
        ("market", &versions.market),
        ("proxy_oracle", &versions.proxy_oracle),
        ("proxy_governance", &versions.proxy_governance),
    ] {
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
