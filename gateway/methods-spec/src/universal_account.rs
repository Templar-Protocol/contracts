use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::{read_method_spec, write_method_spec};
use templar_gateway_types::primitive::PublicKey;
use templar_universal_account::{transaction::Transaction, ExecuteArgs, KeyId};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GetKey {
    pub account_id: AccountId,
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
    pub verifying_contract: AccountId,
    pub salt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetKeyResult {
    pub parameters: Option<PayloadExecutionParametersView>,
}

read_method_spec!(
    /// Get key parameters from a universal account.
    "ua.getKey": GetKey -> GetKeyResult
);

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Execute {
    pub account_id: AccountId,
    pub args: ExecuteArgs<Box<[Transaction]>>,
}

write_method_spec!(
    /// Execute a universal account payload.
    "ua.execute": Execute
);

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Create {
    pub registry_id: AccountId,
    pub account_name: String,
    pub version_key: String,
    pub key: KeyId,
    pub chain_id: templar_gateway_types::U128,
    pub execute: Option<Box<[Transaction]>>,
    pub full_access_keys: Option<Vec<PublicKey>>,
    pub deposit: templar_gateway_types::NearToken,
}

write_method_spec!(
    /// Create a universal account from the registry.
    "ua.create": Create
);
