use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::{pyth::PriceIdentifier, redstone};
use templar_gateway_macros::write_method_spec;
use templar_gateway_types::Base64Bytes;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePyth {
    pub oracle_id: near_account_id::AccountId,
    pub vaa: Base64Bytes,
}

write_method_spec!(
    /// Submit a Pyth oracle update.
    "oracle.updatePyth": UpdatePyth
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateRedStone {
    pub oracle_id: near_account_id::AccountId,
    pub feed_id: redstone::FeedId,
}

write_method_spec!(
    /// Submit a RedStone oracle update.
    "oracle.updateRedStone": UpdateRedStone
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePrices {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
}

write_method_spec!(
    /// Submit all updates needed for prices.
    "oracle.updatePrices": UpdatePrices
);
