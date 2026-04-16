use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::WriteOperationResult,
    RegistryId, UniversalAccountId,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GetKeyArgs {
    pub key: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GetKeyParams {
    pub account_id: UniversalAccountId,
    #[serde(flatten)]
    pub args: GetKeyArgs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PayloadExecutionParametersView {
    pub block_height: u64,
    pub index: u64,
    pub nonce: u64,
    pub name: Option<String>,
    pub version: Option<String>,
    pub chain_id: Option<u128>,
    pub verifying_contract: near_account_id::AccountId,
    pub salt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetKeyResult {
    pub parameters: Option<PayloadExecutionParametersView>,
}

public_read_method_spec!(GetKey, "ua.getKey", GetKeyParams, GetKeyResult);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteBody {
    pub account_id: UniversalAccountId,
    pub args: serde_json::Value,
}

pub type ExecuteResult = WriteOperationResult;

write_method_spec!(Execute, "ua.execute", ExecuteBody, ExecuteResult);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CreateAccountBody {
    pub registry_id: RegistryId,
    pub account_name: String,
    pub key: serde_json::Value,
    pub chain_id: String,
    pub execute: Option<serde_json::Value>,
    pub full_access_keys: Option<Vec<String>>,
    pub deposit: crate::NearToken,
}

pub type CreateAccountResult = WriteOperationResult;

write_method_spec!(
    CreateAccount,
    "ua.createAccount",
    CreateAccountBody,
    CreateAccountResult
);
