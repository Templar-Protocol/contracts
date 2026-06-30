use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::primitive::PublicKey;
use templar_universal_account::{transaction::Transaction, KeyId};

/// Get key parameters from a universal account.
#[derive(MethodSpec, Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[method(read = "ua.getKey", output = GetKeyResult)]
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

/// Execute a universal account payload.
///
/// `args` is the user's signed `ExecuteArgs` payload, forwarded to the contract
/// verbatim (as raw JSON) rather than re-serialized through a typed model: the
/// payload is signed and targets a specific contract version, so canonicalizing
/// it risks breaking deserialization or the signature.
#[derive(MethodSpec, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[method(write = "ua.execute")]
pub struct Execute {
    pub account_id: AccountId,
    pub args: serde_json::Value,
}

/// Create a universal account from the registry.
#[derive(MethodSpec, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[method(write = "ua.create")]
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
