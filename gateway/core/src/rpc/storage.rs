use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{read_method_spec, write_method_spec},
    rpc::common::{StorageBalance, StorageBalanceBounds},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceBoundsParams {
    pub contract_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceBoundsResult {
    pub bounds: StorageBalanceBounds,
}

read_method_spec!(
    /// Get storage balance bounds for a contract.
    "storage.getBalanceBounds": GetBalanceBounds(GetBalanceBoundsParams) -> GetBalanceBoundsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfParams {
    pub contract_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfResult {
    pub balance: Option<StorageBalance>,
}

read_method_spec!(
    /// Get storage balance for an account.
    "storage.getBalanceOf": GetBalanceOf(GetBalanceOfParams) -> GetBalanceOfResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DepositBody {
    pub contract_id: AccountId,
    pub beneficiary_id: Option<AccountId>,
    #[serde(default)]
    pub registration_only: bool,
    pub deposit: crate::NearToken,
}

write_method_spec!(
    /// Deposit storage for an account.
    "storage.deposit": Deposit(DepositBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnregisterBody {
    pub contract_id: AccountId,
    #[serde(default)]
    pub force: bool,
}

write_method_spec!(
    /// Unregister storage for an account.
    "storage.unregister": Unregister(UnregisterBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnsureDepositBody {
    pub contract_id: AccountId,
    pub account_id: AccountId,
    pub mode: EnsureDepositMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "mode", content = "amount")]
pub enum EnsureDepositMode {
    Registered,
    MinimumTotal(crate::NearToken),
    MinimumAvailable(crate::NearToken),
}

write_method_spec!(
    /// Ensure an account has enough storage deposit.
    "storage.ensureDeposit": EnsureDeposit(EnsureDepositBody)
);
