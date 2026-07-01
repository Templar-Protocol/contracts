use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_primitives::SU128;

/// Get a fungible token balance.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "ft.getBalanceOf", output = GetBalanceOfResult)]
pub struct GetBalanceOf {
    pub contract_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfResult {
    pub balance: SU128,
}

/// Transfer fungible tokens.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "ft.transfer")]
pub struct Transfer {
    pub contract_id: AccountId,
    pub receiver_id: AccountId,
    pub amount: SU128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// Transfer fungible tokens and call the receiver.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "ft.transferCall")]
pub struct TransferCall {
    pub contract_id: AccountId,
    pub receiver_id: AccountId,
    pub amount: SU128,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}
