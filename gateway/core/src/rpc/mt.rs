use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{read_method_spec, write_method_spec},
    U128,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MtApproval {
    pub owner_id: AccountId,
    pub approval_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfParams {
    pub contract_id: AccountId,
    pub account_id: AccountId,
    pub token_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfResult {
    pub balance: U128,
}

read_method_spec!(
    /// Get a multi-token balance.
    "mt.getBalanceOf": GetBalanceOf(GetBalanceOfParams) -> GetBalanceOfResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBatchBalanceOfParams {
    pub contract_id: AccountId,
    pub account_id: AccountId,
    pub token_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BalanceEntry {
    pub token_id: String,
    pub balance: U128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBatchBalanceOfResult {
    pub balances: Vec<BalanceEntry>,
}

read_method_spec!(
    /// Get multiple multi-token balances.
    "mt.getBatchBalanceOf": GetBatchBalanceOf(GetBatchBalanceOfParams) -> GetBatchBalanceOfResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyParams {
    pub contract_id: AccountId,
    pub token_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyResult {
    pub supply: Option<U128>,
}

read_method_spec!(
    /// Get total supply for a multi-token ID.
    "mt.getSupply": GetSupply(GetSupplyParams) -> GetSupplyResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBatchSupplyParams {
    pub contract_id: AccountId,
    pub token_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SupplyEntry {
    pub token_id: String,
    pub supply: Option<U128>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBatchSupplyResult {
    pub supplies: Vec<SupplyEntry>,
}

read_method_spec!(
    /// Get total supply for multiple multi-token IDs.
    "mt.getBatchSupply": GetBatchSupply(GetBatchSupplyParams) -> GetBatchSupplyResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransferBody {
    pub contract_id: AccountId,
    pub receiver_id: AccountId,
    pub token_id: String,
    pub amount: U128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<MtApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

write_method_spec!(
    /// Transfer multi-tokens.
    "mt.transfer": Transfer(TransferBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransferCallBody {
    pub contract_id: AccountId,
    pub receiver_id: AccountId,
    pub token_id: String,
    pub amount: U128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<MtApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    pub msg: String,
}

write_method_spec!(
    /// Transfer multi-tokens and call the receiver.
    "mt.transferCall": TransferCall(TransferCallBody)
);
