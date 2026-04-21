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

public_read_method_spec!(
    /// Fetch transaction execution status and result details.
    Get,
    "tx.get",
    GetParams,
    GetResult
);

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
    /// Submit a single function-call transaction.
    FunctionCall,
    "tx.functionCall",
    FunctionCallBody,
    FunctionCallResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransferBody {
    pub receiver_id: AccountId,
    pub amount: NearToken,
}

pub type TransferResult = WriteOperationResult;

write_method_spec!(
    /// Transfer native NEAR to another account.
    Transfer,
    "tx.transfer",
    TransferBody,
    TransferResult
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeployContractBody {
    pub account_id: AccountId,
    pub code: Base64Bytes,
}

pub type DeployContractResult = WriteOperationResult;

write_method_spec!(
    /// Deploy contract code to an existing account in a single transaction.
    DeployContract,
    "tx.deployContract",
    DeployContractBody,
    DeployContractResult
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeployAndInitBody {
    pub account_id: AccountId,
    pub code: Base64Bytes,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
    pub gas: NearGas,
    pub deposit: NearToken,
}

pub type DeployAndInitResult = WriteOperationResult;

write_method_spec!(
    /// Deploy contract code and call its init method in one transaction.
    DeployAndInit,
    "tx.deployAndInit",
    DeployAndInitBody,
    DeployAndInitResult
);
