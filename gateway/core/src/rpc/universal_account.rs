use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_universal_account::{transaction::Transaction, ExecuteArgs, KeyId};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    primitive::PublicKey,
    rpc::common::WriteOperationResult,
    RegistryId, UniversalAccountId,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GetKeyParams {
    pub account_id: UniversalAccountId,
    pub key: KeyId,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteBody {
    pub account_id: UniversalAccountId,
    pub args: ExecuteArgs<Box<[Transaction]>>,
}

pub type ExecuteResult = WriteOperationResult;

write_method_spec!(Execute, "ua.execute", ExecuteBody, ExecuteResult);

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateBody {
    pub registry_id: RegistryId,
    pub account_name: String,
    pub version_key: String,
    pub key: KeyId,
    pub chain_id: crate::U128,
    pub execute: Option<Box<[Transaction]>>,
    pub full_access_keys: Option<Vec<PublicKey>>,
    pub deposit: crate::NearToken,
}

pub type CreateResult = WriteOperationResult;

write_method_spec!(Create, "ua.create", CreateBody, CreateResult);
