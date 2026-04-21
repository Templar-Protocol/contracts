use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::WriteOperationResult,
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

public_read_method_spec!(
    GetBalanceOf,
    "mt.getBalanceOf",
    GetBalanceOfParams,
    GetBalanceOfResult
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

public_read_method_spec!(
    GetBatchBalanceOf,
    "mt.getBatchBalanceOf",
    GetBatchBalanceOfParams,
    GetBatchBalanceOfResult
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

public_read_method_spec!(GetSupply, "mt.getSupply", GetSupplyParams, GetSupplyResult);

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

public_read_method_spec!(
    GetBatchSupply,
    "mt.getBatchSupply",
    GetBatchSupplyParams,
    GetBatchSupplyResult
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

pub type TransferResult = WriteOperationResult;

write_method_spec!(Transfer, "mt.transfer", TransferBody, TransferResult);

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

pub type TransferCallResult = WriteOperationResult;

write_method_spec!(
    TransferCall,
    "mt.transferCall",
    TransferCallBody,
    TransferCallResult
);
