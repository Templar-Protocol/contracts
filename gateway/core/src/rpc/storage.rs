use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::{StorageBalance, StorageBalanceBounds, WriteOperationResult},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceBoundsParams {
    pub contract_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceBoundsResult {
    pub bounds: StorageBalanceBounds,
}

public_read_method_spec!(
    GetBalanceBounds,
    "storage.getBalanceBounds",
    GetBalanceBoundsParams,
    GetBalanceBoundsResult
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

public_read_method_spec!(
    GetBalanceOf,
    "storage.getBalanceOf",
    GetBalanceOfParams,
    GetBalanceOfResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DepositBody {
    pub contract_id: AccountId,
    pub beneficiary_id: Option<AccountId>,
    #[serde(default)]
    pub registration_only: bool,
    pub deposit: crate::NearToken,
}

pub type DepositResult = WriteOperationResult;

write_method_spec!(Deposit, "storage.deposit", DepositBody, DepositResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnregisterBody {
    pub contract_id: AccountId,
    #[serde(default)]
    pub force: bool,
}

pub type UnregisterResult = WriteOperationResult;

write_method_spec!(
    Unregister,
    "storage.unregister",
    UnregisterBody,
    UnregisterResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnsureDepositBody {
    pub contract_id: AccountId,
    pub account_id: AccountId,
    pub mode: EnsureDepositMode,
}

pub type EnsureDepositResult = WriteOperationResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "mode", content = "amount")]
pub enum EnsureDepositMode {
    Registered,
    MinimumTotal(crate::NearToken),
    MinimumAvailable(crate::NearToken),
}

write_method_spec!(
    EnsureDeposit,
    "storage.ensureDeposit",
    EnsureDepositBody,
    EnsureDepositResult
);
