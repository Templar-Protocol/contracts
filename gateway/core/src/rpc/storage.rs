use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::{StorageBalance, StorageBalanceBounds, WriteOperationResult},
    PublicReadMethod, StorageReadMethod, StorageWriteMethod, WriteMethod,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceBoundsArgs {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceBoundsParams {
    pub contract_id: AccountId,
    #[serde(flatten)]
    pub args: GetBalanceBoundsArgs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceBoundsResult {
    pub bounds: StorageBalanceBounds,
}

public_read_method_spec!(
    GetBalanceBounds,
    "storage.getBalanceBounds",
    PublicReadMethod::Storage(StorageReadMethod::GetBalanceBounds),
    GetBalanceBoundsParams,
    GetBalanceBoundsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfArgs {
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfParams {
    pub contract_id: AccountId,
    #[serde(flatten)]
    pub args: GetBalanceOfArgs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBalanceOfResult {
    pub balance: Option<StorageBalance>,
}

public_read_method_spec!(
    GetBalanceOf,
    "storage.getBalanceOf",
    PublicReadMethod::Storage(StorageReadMethod::GetBalanceOf),
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

write_method_spec!(
    Deposit,
    "storage.deposit",
    WriteMethod::Storage(StorageWriteMethod::Deposit),
    DepositBody,
    DepositResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnsureDepositBody {
    pub contract_id: AccountId,
    pub account_id: AccountId,
    #[serde(default)]
    pub registration_only: bool,
}

pub type EnsureDepositResult = WriteOperationResult;

write_method_spec!(
    EnsureDeposit,
    "storage.ensureDeposit",
    WriteMethod::Storage(StorageWriteMethod::EnsureDeposit),
    EnsureDepositBody,
    EnsureDepositResult
);
