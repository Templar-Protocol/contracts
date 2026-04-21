use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::pyth::{Price, PriceIdentifier};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::WriteOperationResult,
    Base64Bytes,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PriceEntry {
    pub price_id: PriceIdentifier,
    pub price: Option<Price>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListEmaPricesNoOlderThanParams {
    pub oracle_id: AccountId,
    pub price_ids: Vec<PriceIdentifier>,
    pub age: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListEmaPricesNoOlderThanResult {
    pub prices: Vec<PriceEntry>,
}

public_read_method_spec!(
    ListEmaPricesNoOlderThan,
    "pyth.listEmaPricesNoOlderThan",
    ListEmaPricesNoOlderThanParams,
    ListEmaPricesNoOlderThanResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListEmaPricesUnsafeParams {
    pub oracle_id: AccountId,
    pub price_ids: Vec<PriceIdentifier>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListEmaPricesUnsafeResult {
    pub prices: Vec<PriceEntry>,
}

public_read_method_spec!(
    ListEmaPricesUnsafe,
    "pyth.listEmaPricesUnsafe",
    ListEmaPricesUnsafeParams,
    ListEmaPricesUnsafeResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePriceFeedsBody {
    pub oracle_id: AccountId,
    pub data: Base64Bytes,
}

pub type UpdatePriceFeedsResult = WriteOperationResult;

write_method_spec!(
    UpdatePriceFeeds,
    "pyth.updatePriceFeeds",
    UpdatePriceFeedsBody,
    UpdatePriceFeedsResult
);
