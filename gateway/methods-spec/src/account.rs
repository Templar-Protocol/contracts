use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{primitive::PublicKey, ContractMethodName, NearToken};

/// Get chain state for a NEAR account.
///
/// Returns balances, storage usage, and contract hash information for the
/// requested account.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "account.get", output = GetResult)]
pub struct Get {
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetResult {
    pub amount: NearToken,
    pub locked: NearToken,
    pub code_hash: String,
    pub storage_usage: u64,
    pub global_contract_hash: Option<String>,
    pub global_contract_account_id: Option<AccountId>,
}

/// Get an access key's nonce and permission scope for an account.
///
/// Mirrors NEAR's `view_access_key`: returns the key's current `nonce` (for
/// building the next transaction/meta-transaction) and its permission scope.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "account.getAccessKey", output = GetAccessKeyResult)]
pub struct GetAccessKey {
    pub account_id: AccountId,
    pub public_key: PublicKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetAccessKeyResult {
    pub nonce: u64,
    pub permission: AccessKeyPermission,
}

/// An access key's permission scope (mirrors NEAR's `AccessKeyPermissionView`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AccessKeyPermission {
    /// Full access to the account.
    FullAccess,
    /// Restricted to function calls against `receiver_id` (optionally limited to
    /// `method_names` and a remaining `allowance`).
    FunctionCall {
        allowance: Option<NearToken>,
        receiver_id: AccountId,
        method_names: Vec<ContractMethodName>,
    },
}

/// Delete a managed account and send remaining funds to a beneficiary.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "account.delete")]
pub struct Delete {
    pub beneficiary_id: AccountId,
}
