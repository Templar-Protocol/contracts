mod list_ema_prices_no_older_than;
mod list_ema_prices_unsafe;
mod update_price_feeds;

pub use list_ema_prices_no_older_than::ListEmaPricesNoOlderThan;
pub use list_ema_prices_unsafe::ListEmaPricesUnsafe;
pub use update_price_feeds::UpdatePriceFeeds;

use clap::Subcommand;

/// Direct reads and writes against a Pyth oracle contract. Unlike `oracle update-pyth`,
/// these fetch no payload — `update-price-feeds` submits caller-supplied update data
/// verbatim.
#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum PythNs {
    /// List EMA prices, rejecting any older than `--age-s`.
    ListEmaPricesNoOlderThan(ListEmaPricesNoOlderThan),
    /// List EMA prices with no age limit.
    ListEmaPricesUnsafe(ListEmaPricesUnsafe),
    /// Submit raw Pyth update data on-chain.
    UpdatePriceFeeds(UpdatePriceFeeds),
}
