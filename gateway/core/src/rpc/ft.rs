use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::WriteOperationResult,
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

public_read_method_spec!(
    GetBalanceOf,
    "ft.getBalanceOf",
    GetBalanceOfParams,
    GetBalanceOfResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransferBody {
    pub contract_id: AccountId,
    pub receiver_id: AccountId,
    pub amount: U128,
}

pub type TransferResult = WriteOperationResult;

write_method_spec!(Transfer, "ft.transfer", TransferBody, TransferResult);
