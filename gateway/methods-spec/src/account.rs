use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::NearToken;

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

/// Delete a managed account and send remaining funds to a beneficiary.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "account.delete")]
pub struct Delete {
    pub beneficiary_id: AccountId,
}
