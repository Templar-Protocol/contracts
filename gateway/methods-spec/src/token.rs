use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::{read_method_spec, write_method_spec};
use templar_gateway_types::U128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "standard", rename_all = "snake_case")]
pub enum TokenReference {
    Ft {
        contract_id: AccountId,
    },
    Mt {
        contract_id: AccountId,
        token_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOf {
    pub token: TokenReference,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfResult {
    pub balance: U128,
}

read_method_spec!(
    /// Get a token balance across supported standards.
    "token.getBalanceOf": GetBalanceOf -> GetBalanceOfResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Transfer {
    pub token: TokenReference,
    pub receiver_id: AccountId,
    pub amount: U128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

write_method_spec!(
    /// Transfer a token across supported standards.
    "token.transfer": Transfer
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransferCall {
    pub token: TokenReference,
    pub receiver_id: AccountId,
    pub amount: U128,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

write_method_spec!(
    /// Transfer a token and call the receiver.
    "token.transferCall": TransferCall
);
