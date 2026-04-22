use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{macros::read_method_spec, U128};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPoolsParams {
    pub exchange_id: AccountId,
    pub from_index: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PoolInfo {
    pub token_account_ids: Vec<AccountId>,
    pub shares_total_supply: U128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPoolsResult {
    pub pools: Vec<PoolInfo>,
}

read_method_spec!(
    /// List pools from a Ref Finance exchange.
    "refFinance.getPools": GetPools(GetPoolsParams) -> GetPoolsResult
);
