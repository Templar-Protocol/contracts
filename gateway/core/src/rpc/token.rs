use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::WriteOperationResult,
    U128,
};

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
pub struct GetBalanceOfParams {
    pub token: TokenReference,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfResult {
    pub balance: U128,
}

public_read_method_spec!(
    GetBalanceOf,
    "token.getBalanceOf",
    GetBalanceOfParams,
    GetBalanceOfResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransferBody {
    pub token: TokenReference,
    pub receiver_id: AccountId,
    pub amount: U128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

pub type TransferResult = WriteOperationResult;

write_method_spec!(Transfer, "token.transfer", TransferBody, TransferResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransferCallBody {
    pub token: TokenReference,
    pub receiver_id: AccountId,
    pub amount: U128,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

pub type TransferCallResult = WriteOperationResult;

write_method_spec!(
    TransferCall,
    "token.transferCall",
    TransferCallBody,
    TransferCallResult
);
