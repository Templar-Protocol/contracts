use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_primitives::SU128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MtApproval {
    pub owner_id: AccountId,
    pub approval_id: u64,
}

/// Get a multi-token balance.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "mt.getBalanceOf", output = SU128)]
pub struct GetBalanceOf {
    pub contract_id: AccountId,
    pub account_id: AccountId,
    pub token_id: String,
}

/// Get multiple multi-token balances.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "mt.getBatchBalanceOf", output = Vec<BalanceEntry>)]
pub struct GetBatchBalanceOf {
    pub contract_id: AccountId,
    pub account_id: AccountId,
    pub token_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BalanceEntry {
    pub token_id: String,
    pub balance: SU128,
}

/// Get total supply for a multi-token ID.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "mt.getSupply", output = Option<SU128>)]
pub struct GetSupply {
    pub contract_id: AccountId,
    pub token_id: String,
}

/// Get total supply for multiple multi-token IDs.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "mt.getBatchSupply", output = Vec<SupplyEntry>)]
pub struct GetBatchSupply {
    pub contract_id: AccountId,
    pub token_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SupplyEntry {
    pub token_id: String,
    pub supply: Option<SU128>,
}

/// Transfer multi-tokens.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "mt.transfer")]
pub struct Transfer {
    pub contract_id: AccountId,
    pub receiver_id: AccountId,
    pub token_id: String,
    pub amount: SU128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<MtApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// Transfer multi-tokens and call the receiver.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "mt.transferCall")]
pub struct TransferCall {
    pub contract_id: AccountId,
    pub receiver_id: AccountId,
    pub token_id: String,
    pub amount: SU128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<MtApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    pub msg: String,
}
