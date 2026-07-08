mod get_proxy;
mod list_proxies;
mod price_feed_exists;
mod update_prices;

pub use get_proxy::GetProxy;
pub use list_proxies::ListProxies;
pub use price_feed_exists::PriceFeedExists;
pub use update_prices::UpdatePrices;

use anyhow::Context as _;
use clap::Subcommand;
use templar_common::oracle::pyth::PriceIdentifier;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleNs {
    GetProxy(GetProxy),
    ListProxies(ListProxies),
    PriceFeedExists(PriceFeedExists),
    UpdatePrices(UpdatePrices),
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
