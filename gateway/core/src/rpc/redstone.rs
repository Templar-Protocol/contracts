use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::redstone::{Config, FeedData, FeedId, Role};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::WriteOperationResult,
    Base64Bytes,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetConfigParams {
    pub oracle_id: AccountId,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetConfigResult {
    pub config: Config,
}

public_read_method_spec!(
    GetConfig,
    "redstone.getConfig",
    GetConfigParams,
    GetConfigResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadPriceDataParams {
    pub oracle_id: AccountId,
    pub feed_ids: Vec<FeedId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PriceDataEntry {
    pub feed_id: FeedId,
    pub data: FeedData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadPriceDataResult {
    pub entries: Vec<PriceDataEntry>,
}

public_read_method_spec!(
    ReadPriceData,
    "redstone.readPriceData",
    ReadPriceDataParams,
    ReadPriceDataResult
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoleValue {
    ModifyRoles,
    TrustedUpdater,
}

impl From<RoleValue> for Role {
    fn from(value: RoleValue) -> Self {
        match value {
            RoleValue::ModifyRoles => Self::ModifyRoles,
            RoleValue::TrustedUpdater => Self::TrustedUpdater,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListRoleParams {
    pub oracle_id: AccountId,
    pub role: RoleValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListRoleResult {
    pub account_ids: Vec<AccountId>,
}

public_read_method_spec!(
    ListRole,
    "redstone.listRole",
    ListRoleParams,
    ListRoleResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetRoleBody {
    pub oracle_id: AccountId,
    pub account_id: AccountId,
    pub role: RoleValue,
    pub set: bool,
}

pub type SetRoleResult = WriteOperationResult;

write_method_spec!(SetRole, "redstone.setRole", SetRoleBody, SetRoleResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WritePricesBody {
    pub oracle_id: AccountId,
    pub feed_ids: Vec<FeedId>,
    pub payload: Base64Bytes,
}

pub type WritePricesResult = WriteOperationResult;

write_method_spec!(
    WritePrices,
    "redstone.writePrices",
    WritePricesBody,
    WritePricesResult
);
