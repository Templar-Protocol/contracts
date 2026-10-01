use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::pyth::{Price, PriceIdentifier};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::Base64Bytes;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PriceEntry {
    pub price_id: PriceIdentifier,
    pub price: Option<Price>,
}

/// List EMA prices within an age limit.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "pyth.listEmaPricesNoOlderThan", output = Vec<PriceEntry>)]
pub struct ListEmaPricesNoOlderThan {
    pub oracle_id: AccountId,
    pub price_ids: Vec<PriceIdentifier>,
    pub age: u64,
}

/// List EMA prices without an age limit.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "pyth.listEmaPricesUnsafe", output = Vec<PriceEntry>)]
pub struct ListEmaPricesUnsafe {
    pub oracle_id: AccountId,
    pub price_ids: Vec<PriceIdentifier>,
}

/// Submit raw Pyth update data.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "pyth.updatePriceFeeds")]
pub struct UpdatePriceFeeds {
    pub oracle_id: AccountId,
    pub data: Base64Bytes,
}
