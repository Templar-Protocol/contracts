use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::{ContractArgs, TxExecutionStatus, WriteOperationResult},
    Base64Bytes, ContractMethodName, CryptoHash, NearGas, NearToken,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetParams {
    pub tx_hash: CryptoHash,
    pub sender_account_id: AccountId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_until: Option<TxExecutionStatus>,
    #[serde(default)]
    pub encoding: ValueEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ValueEncoding {
    #[default]
    Json,
    Base64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
pub enum ReturnValue {
    Json(serde_json::Value),
    Base64(Base64Bytes),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GetResult {
    pub status: Status,
    pub total_gas_burnt: NearGas,
    pub logs: Vec<String>,
    pub return_value: Option<ReturnValue>,
}

public_read_method_spec!(Get, "tx.get", GetParams, GetResult);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FunctionCallBody {
    pub receiver_id: AccountId,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
    pub gas: NearGas,
    pub deposit: NearToken,
}

pub type FunctionCallResult = WriteOperationResult;

write_method_spec!(
    FunctionCall,
    "tx.functionCall",
    FunctionCallBody,
    FunctionCallResult
);
