use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::{pyth::PriceIdentifier, redstone};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::Base64Bytes;

/// Submit a Pyth oracle update.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "oracle.updatePyth")]
pub struct UpdatePyth {
    pub oracle_id: near_account_id::AccountId,
    pub vaa: Base64Bytes,
}

/// Submit a RedStone oracle update for one or more feeds.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "oracle.updateRedStone")]
pub struct UpdateRedStone {
    pub oracle_id: near_account_id::AccountId,
    /// Fetched as one payload and written in a single `redstone.writePrices` call,
    /// matching the adapter's own multi-feed method.
    pub feed_ids: Vec<redstone::FeedId>,
}

/// Submit a Pyth Lazer oracle update for a single feed.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "oracle.updateLazer")]
pub struct UpdateLazer {
    pub oracle_id: near_account_id::AccountId,
    pub feed_id: u32,
}

/// Submit all updates needed for prices.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "oracle.updatePrices")]
pub struct UpdatePrices {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
}
