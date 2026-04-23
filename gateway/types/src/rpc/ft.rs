use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{read_method_spec, write_method_spec},
    U128,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfParams {
    pub contract_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfResult {
    pub balance: U128,
}

read_method_spec!(
    /// Get a fungible token balance.
    "ft.getBalanceOf": GetBalanceOf(GetBalanceOfParams) -> GetBalanceOfResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransferBody {
    pub contract_id: AccountId,
    pub receiver_id: AccountId,
    pub amount: U128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

write_method_spec!(
    /// Transfer fungible tokens.
    "ft.transfer": Transfer(TransferBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransferCallBody {
    pub contract_id: AccountId,
    pub receiver_id: AccountId,
    pub amount: U128,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

write_method_spec!(
    /// Transfer fungible tokens and call the receiver.
    "ft.transferCall": TransferCall(TransferCallBody)
);
