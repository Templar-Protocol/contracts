use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::pyth::{Price, PriceIdentifier};
use templar_gateway_macros::{read_method_spec, write_method_spec};
use templar_gateway_types::Base64Bytes;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PriceEntry {
    pub price_id: PriceIdentifier,
    pub price: Option<Price>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListEmaPricesNoOlderThan {
    pub oracle_id: AccountId,
    pub price_ids: Vec<PriceIdentifier>,
    pub age: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListEmaPricesNoOlderThanResult {
    pub prices: Vec<PriceEntry>,
}

read_method_spec!(
    /// List EMA prices within an age limit.
    "pyth.listEmaPricesNoOlderThan": ListEmaPricesNoOlderThan -> ListEmaPricesNoOlderThanResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListEmaPricesUnsafe {
    pub oracle_id: AccountId,
    pub price_ids: Vec<PriceIdentifier>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListEmaPricesUnsafeResult {
    pub prices: Vec<PriceEntry>,
}

read_method_spec!(
    /// List EMA prices without an age limit.
    "pyth.listEmaPricesUnsafe": ListEmaPricesUnsafe -> ListEmaPricesUnsafeResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePriceFeeds {
    pub oracle_id: AccountId,
    pub data: Base64Bytes,
}

write_method_spec!(
    /// Submit raw Pyth update data.
    "pyth.updatePriceFeeds": UpdatePriceFeeds
);
