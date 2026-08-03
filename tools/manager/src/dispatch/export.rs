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
    if !spec.oracle.is_direct() {
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
    // The contract's own membership test, not a client-side scan of a paginated
    // list. The list is read only to say who *does* hold it.
    let holds = ctx
        .client
        .read(governance::HasRole {
            governance_id: governance_id.clone(),
            account_id: admin.clone(),
            role: templar_proxy_oracle_near_governance_common::Role::Admin,
        })
        .await
        .with_context(|| format!("check the Admin role on {governance_id}"))?
        .has_role;

    if holds {
        return Ok(());
    }

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

    anyhow::bail!(
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
    )
}

/// The governance contract's single proposal TTL, or a refusal.
///
/// The policy carries an independent timelock per reflexive bucket, one for the
/// target default, and one per method override. A spec carries a single
/// `ttl_default`, so a policy that is not uniform cannot be expressed and is
/// refused rather than flattened — flattening would drop a real timelock on the
/// next deploy.
async fn governance_ttl(
    ctx: &CliContext,
    governance_id: &AccountId,
) -> anyhow::Result<Nanoseconds> {
    let policy = ctx
        .client
        .read(governance::GetGovernancePolicy {
            governance_id: governance_id.clone(),
        })
        .await
        .with_context(|| format!("read the governance policy from {governance_id}"))?
        .policy;

    let labeled = [
        ("reflexive.set_policy", policy.reflexive_ttls.set_policy),
        ("reflexive.set_role", policy.reflexive_ttls.set_role),
        ("reflexive.self_upgrade", policy.reflexive_ttls.self_upgrade),
        ("default_target", policy.default_target.ttl),
    ]
    .into_iter()
    .map(|(label, ttl)| (label.to_owned(), ttl))
    .chain(
        policy
            .method_policies
            .iter()
            .map(|(method, entry)| (format!("method_policies.{method}"), entry.ttl)),
    );

    let mut uniform: Option<(String, Nanoseconds)> = None;
    for (label, ttl) in labeled {
        match &uniform {
            None => uniform = Some((label, ttl)),
            Some((first_label, first)) => anyhow::ensure!(
                *first == ttl,
                "`{governance_id}` uses per-operation TTLs ({first_label} is {}ns, \
                 {label} is {}ns). A spec carries a single `ttl_default` and cannot \
                 express that, so this market cannot be exported.",
                first.as_ns(),
                ttl.as_ns(),
            ),
        }
    }

    uniform
        .map(|(_, ttl)| ttl)
        .context("governance exposes no TTLs to read")
}

/// A configured proxy, or a legible error. An oracle serving neither constant is
/// not a proxy-oracle deployment, and the spec cannot express it.
pub(super) async fn proxy(
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
