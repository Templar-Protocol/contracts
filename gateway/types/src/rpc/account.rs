use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{read_method_spec, write_method_spec},
    NearToken,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetParams {
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

read_method_spec!(
    /// Get chain state for a NEAR account.
    ///
    /// Returns balances, storage usage, and contract hash information for the
    /// requested account.
    "account.get": Get(GetParams) -> GetResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeleteBody {
    pub beneficiary_id: AccountId,
}

write_method_spec!(
    /// Delete a managed account and send remaining funds to a beneficiary.
    "account.delete": Delete(DeleteBody)
);
