use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::borrow::BorrowPosition;
use templar_common::market::MarketConfiguration;

use crate::{macros::public_read_method_spec, rpc::common::Pagination, MarketId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetConfigurationParams {
    pub market_id: MarketId,
}

pub type GetConfigurationResult = MarketConfiguration;

public_read_method_spec!(
    GetConfiguration,
    "market.getConfiguration",
    GetConfigurationParams,
    GetConfigurationResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListBorrowPositionsParams {
    pub market_id: MarketId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListBorrowPositionsResult {
    pub positions: HashMap<near_account_id::AccountId, BorrowPosition>,
}

public_read_method_spec!(
    ListBorrowPositions,
    "market.listBorrowPositions",
    ListBorrowPositionsParams,
    ListBorrowPositionsResult
);
