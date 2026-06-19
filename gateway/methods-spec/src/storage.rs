use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::common::{StorageBalance, StorageBalanceBounds};

/// Get storage balance bounds for a contract.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "storage.getBalanceBounds", output = GetBalanceBoundsResult)]
pub struct GetBalanceBounds {
    pub contract_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceBoundsResult {
    pub bounds: StorageBalanceBounds,
}

/// Get storage balance for an account.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "storage.getBalanceOf", output = GetBalanceOfResult)]
pub struct GetBalanceOf {
    pub contract_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfResult {
    pub balance: Option<StorageBalance>,
}

/// Deposit storage for an account.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "storage.deposit")]
pub struct Deposit {
    pub contract_id: AccountId,
    pub beneficiary_id: Option<AccountId>,
    #[serde(default)]
    pub registration_only: bool,
    pub deposit: templar_gateway_types::NearToken,
}

/// Unregister storage for an account.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "storage.unregister")]
pub struct Unregister {
    pub contract_id: AccountId,
    #[serde(default)]
    pub force: bool,
}

/// Ensure an account has enough storage deposit.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "storage.ensureDeposit")]
pub struct EnsureDeposit {
    pub contract_id: AccountId,
    pub account_id: AccountId,
    pub mode: EnsureDepositMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "mode", content = "amount")]
pub enum EnsureDepositMode {
    Registered,
    MinimumTotal(templar_gateway_types::NearToken),
    MinimumAvailable(templar_gateway_types::NearToken),
}
