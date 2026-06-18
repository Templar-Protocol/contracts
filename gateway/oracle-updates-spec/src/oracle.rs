use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::{pyth::PriceIdentifier, redstone};
use templar_gateway_macros::write_method_spec;
use templar_gateway_types::Base64Bytes;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePythBody {
    pub oracle_id: near_account_id::AccountId,
    pub vaa: Base64Bytes,
}

write_method_spec!(
    /// Submit a Pyth oracle update.
    "oracle.updatePyth": UpdatePyth(UpdatePythBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateRedStoneBody {
    pub oracle_id: near_account_id::AccountId,
    pub feed_id: redstone::FeedId,
}

write_method_spec!(
    /// Submit a RedStone oracle update.
    "oracle.updateRedStone": UpdateRedStone(UpdateRedStoneBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePricesBody {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
}

write_method_spec!(
    /// Submit all updates needed for prices.
    "oracle.updatePrices": UpdatePrices(UpdatePricesBody)
);
