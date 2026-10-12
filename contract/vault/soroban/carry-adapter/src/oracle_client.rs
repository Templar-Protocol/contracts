//! Minimal SEP-40 oracle client used by the carry adapter.
//!
//! The adapter reads the oracle exactly like the Precept pool does:
//! `decimals()` and `lastprice(Asset)`. The types here are layout-compatible
//! with `sep-40-oracle` 1.2.0 (`Asset::Stellar` variant 0, `PriceData`
//! `{price, timestamp}`), declared locally to avoid taking a dependency on
//! the crate (which drags a newer soroban-sdk into the workspace).

use soroban_sdk::{contractclient, contracttype, Address, Env, Symbol};

/// Price data for an asset at a specific timestamp.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

/// Asset type; ABI-compatible with `sep_40_oracle::Asset`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Asset {
    Stellar(Address),
    Other(Symbol),
}

// The trait body is the ABI source for the generated `PriceFeedClient`;
// the trait itself is intentionally never referenced.
#[allow(dead_code)]
/// Oracle feed interface actually used by the adapter.
#[contractclient(name = "PriceFeedClient")]
pub trait PriceFeedTrait {
    /// Return the number of decimals for all assets quoted by the price feed.
    fn decimals(env: Env) -> u32;

    /// Get the most recent price for an asset.
    fn lastprice(env: Env, asset: Asset) -> Option<PriceData>;
}
