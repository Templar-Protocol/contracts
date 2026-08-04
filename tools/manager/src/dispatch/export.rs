//! `market export` — read a deployed market and reconstruct its spec.
//!
//! A multi-step read chain (the configuration, both proxies, three registry
//! deployment records), so it lives here rather than in `commands`, which stays
//! free of IO.

use anyhow::Context as _;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_methods_spec::{
    contract, market, owner, proxy_oracle, proxy_oracle_governance as governance, registry,
};
use templar_gateway_types::contract::ContractKind;
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_governance_common::{GovernancePolicy, GovernancePolicyWire};

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
    // reports the same facts as checks, so failing hard inside it would turn a
    // monitoring run into an abort. An export is a record, and a record that
    // redeploys into something else is worse than none.
    if !spec.oracle.is_direct() {
        let governance_id = spec.governance_id()?;
        ensure_admin_holds_the_role(&ctx, &governance_id, &args.governance_admin).await?;
        ensure_policy_is_expressible(&ctx, &governance_id).await?;
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

    let (versions, governance, proxies) =
        if let Some(governance_id) = governing_contract(ctx, &oracle_id).await? {
            // A spec derives this name rather than storing it, so governance living
            // anywhere else is a deployment the spec cannot express.
            let derived = governance_account_id(&name, &registry_id)?;
            anyhow::ensure!(
                derived == governance_id,
                "`{oracle_id}` is governed by `{governance_id}`, but a spec for \
             `{market_id}` derives `{derived}`. This deployment cannot be \
             expressed as a spec.",
            );
            (
                versions(ctx, &name, &registry_id, &oracle_id, market_id).await?,
                Some(GovernanceSpec {
                    admin: governance_admin.clone(),
                    ttl_default: governance_policy(ctx, &governance_id)
                        .await?
                        .default_target
                        .ttl,
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

/// The governance contract that controls `oracle_id`, or `None` when the market
/// reads an oracle it does not control.
///
/// Ownership, confirmed in both directions: the oracle names its owner and the
/// owner names the oracle it governs. Inferring this from the account name and
/// the placeholder price ids instead would misread a proxy deployed under any
/// other name as a plain oracle.
async fn governing_contract(
    ctx: &CliContext,
    oracle_id: &AccountId,
) -> anyhow::Result<Option<AccountId>> {
    if kind_of(ctx, oracle_id).await? != ContractKind::ProxyOracle {
        return Ok(None);
    }

    let Some(owner) = ctx
        .client
        .read(owner::GetOwner {
            contract_id: oracle_id.clone(),
        })
        .await
        .with_context(|| format!("read the owner of `{oracle_id}`"))?
        .owner
    else {
        return Ok(None);
    };

    // Classified before it is questioned: plenty of proxies are owned by an
    // account that governs nothing, and only that answer means "not ours". A
    // read that merely failed must not demote a governed oracle to a direct one.
    if kind_of(ctx, &owner).await? != ContractKind::ProxyGovernance {
        return Ok(None);
    }

    let governed = ctx
        .client
        .read(governance::GetProxyOracleId {
            governance_id: owner.clone(),
        })
        .await
        .with_context(|| format!("ask `{owner}` which oracle it governs"))?
        .proxy_oracle_id;

    Ok((governed == *oracle_id).then_some(owner))
}

async fn kind_of(ctx: &CliContext, contract_id: &AccountId) -> anyhow::Result<ContractKind> {
    Ok(ctx
        .client
        .read(contract::GetKind {
            contract_id: contract_id.clone(),
        })
        .await
        .with_context(|| format!("classify `{contract_id}`"))?
        .kind)
}

async fn governance_policy(
    ctx: &CliContext,
    governance_id: &AccountId,
) -> anyhow::Result<GovernancePolicyWire> {
    Ok(ctx
        .client
        .read(governance::GetGovernancePolicy {
            governance_id: governance_id.clone(),
        })
        .await
        .with_context(|| format!("read the governance policy from {governance_id}"))?
        .policy)
}

/// Refuse to record a policy the spec would not redeploy.
async fn ensure_policy_is_expressible(
    ctx: &CliContext,
    governance_id: &AccountId,
) -> anyhow::Result<()> {
    let wire = governance_policy(ctx, governance_id).await?;
    ensure_expressible(governance_id, &wire)
}

/// The pure half of [`ensure_policy_is_expressible`], so the refusal is testable
/// offline.
///
/// A spec carries one `ttl_default`, which deploys as
/// [`GovernancePolicy::uniform`]: that TTL everywhere, `Admin` on every method,
/// no overrides. Any other policy loses its difference on the next deploy.
pub(crate) fn ensure_expressible(
    governance_id: &AccountId,
    wire: &GovernancePolicyWire,
) -> anyhow::Result<()> {
    let deployed = GovernancePolicy::try_from(wire.clone())
        .with_context(|| format!("parse the governance policy from `{governance_id}`"))?;
    let expressible = GovernancePolicy::uniform(wire.default_target.ttl)
        .with_context(|| format!("build the uniform policy for `{governance_id}`"))?;

    anyhow::ensure!(
        deployed == expressible,
        "`{governance_id}` runs a policy no spec can express: a spec carries one \
         `ttl_default`, which deploys as that TTL everywhere, `Admin` on every \
         method, and no overrides. Exporting it anyway would drop the difference \
         on the next deploy. Deployed policy:\n{}",
        serde_json::to_string_pretty(&wire).unwrap_or_else(|_| format!("{wire:?}")),
    );

    Ok(())
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
