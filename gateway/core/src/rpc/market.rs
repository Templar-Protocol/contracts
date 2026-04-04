use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::market::MarketConfiguration;

use crate::{
    macros::public_read_method_spec,
    rpc::common::{JsonValueResult, Pagination},
    MarketId, MarketReadMethod, PublicReadMethod,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetConfigurationParams {
    pub market_id: MarketId,
}

pub type GetConfigurationResult = MarketConfiguration;

public_read_method_spec!(
    GetConfiguration,
    "market.getConfiguration",
    PublicReadMethod::Market(MarketReadMethod::GetConfiguration),
    GetConfigurationParams,
    GetConfigurationResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListBorrowPositionsParams {
    pub market_id: MarketId,
    #[serde(flatten)]
    pub args: Pagination,
}

pub type ListBorrowPositionsResult = JsonValueResult;

public_read_method_spec!(
    ListBorrowPositions,
    "market.listBorrowPositions",
    PublicReadMethod::Market(MarketReadMethod::ListBorrowPositions),
    ListBorrowPositionsParams,
    ListBorrowPositionsResult
);
