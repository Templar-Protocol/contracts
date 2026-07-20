mod create;
mod get_proxy;
mod get_proxy_circuit_breaker_set;
mod governance;
mod list_proxies;
mod price_feed_exists;
mod update_prices;
mod upgrade;

pub use create::Create;
pub use get_proxy::GetProxy;
pub use get_proxy_circuit_breaker_set::GetProxyCircuitBreakerSet;
pub use governance::{CreateProposal, ExecuteProposalArgs, ProxyOracleGovernanceNs};
pub use list_proxies::ListProxies;
pub use price_feed_exists::PriceFeedExists;
pub use update_prices::UpdatePrices;
pub use upgrade::Upgrade;

use anyhow::Context as _;
use clap::Subcommand;
use templar_common::oracle::pyth::PriceIdentifier;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleNs {
    /// Deploy a proxy oracle from a registry, optionally owned by `--owner-id`.
    Create(Create),
    /// Administer a proxy oracle through its governance contract.
    #[command(subcommand, visible_alias = "gov")]
    Governance(ProxyOracleGovernanceNs),
    /// Read a single price feed's proxy configuration.
    GetProxy(GetProxy),
    /// List the oracle's configured price feeds.
    ListProxies(ListProxies),
    /// Check whether a price feed exists.
    PriceFeedExists(PriceFeedExists),
    /// Read the circuit breaker set configured for a price feed.
    GetProxyCircuitBreakerSet(GetProxyCircuitBreakerSet),
    /// Refresh on-chain prices for one or more feeds.
    UpdatePrices(UpdatePrices),
    /// Upgrade with an explicit migration and audited local WASM.
    Upgrade(Upgrade),
}

/// Parse a 32-byte hex price identifier (accepting an optional `0x` prefix).
/// Shared across the oracle and governance commands.
pub(crate) fn parse_price_identifier(hex: &str) -> anyhow::Result<PriceIdentifier> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    let bytes = hex::decode(hex).context("decode hex price identifier")?;
    if bytes.len() != 32 {
        anyhow::bail!("price identifier must be 32 bytes, got {}", bytes.len());
    }
    let mut id = [0u8; 32];
    id.copy_from_slice(&bytes);
    Ok(PriceIdentifier(id))
}
