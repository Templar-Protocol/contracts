//! `market export` — read a deployed market and reconstruct its spec.
//!
//! A multi-step read chain (the configuration, both proxies, three registry
//! deployment records), so it lives here rather than in `commands`, which stays
//! free of IO.

use anyhow::Context as _;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_methods_spec::{market, proxy_oracle, registry};
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
    let (name, registry_id) = split_market_id(&args.market_id)?;

    let configuration = ctx
        .client
        .read(market::GetConfiguration {
            market_id: args.market_id.clone(),
        })
        .await
        .context("read market configuration")?;

    let oracle_id = configuration.price_oracle_configuration.account_id.clone();
    let spec = MarketSpec::from_deployed(Deployed {
        versions: versions(&ctx, &name, &registry_id, &oracle_id, &args.market_id).await?,
        governance: GovernanceSpec {
            admin: args.governance_admin.clone(),
            ttl_default: args.governance_ttl,
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
    Ok(Versions {
        market: version_key(ctx, registry_id, market_id).await?,
        proxy_oracle: version_key(ctx, registry_id, oracle_id).await?,
        proxy_governance: version_key(ctx, registry_id, &governance_id).await?,
    })
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
