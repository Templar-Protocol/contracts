use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::redstone::{Config, FeedData, FeedId, Role};

use crate::{
    macros::{read_method_spec, write_method_spec},
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

read_method_spec!(
    /// Get RedStone oracle config.
    "redstone.getConfig": GetConfig(GetConfigParams) -> GetConfigResult
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

read_method_spec!(
    /// Read RedStone price data.
    "redstone.readPriceData": ReadPriceData(ReadPriceDataParams) -> ReadPriceDataResult
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

read_method_spec!(
    /// List accounts for a RedStone role.
    "redstone.listRole": ListRole(ListRoleParams) -> ListRoleResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetRoleBody {
    pub oracle_id: AccountId,
    pub account_id: AccountId,
    pub role: RoleValue,
    pub set: bool,
}

write_method_spec!(
    /// Update a RedStone role membership.
    "redstone.setRole": SetRole(SetRoleBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WritePricesBody {
    pub oracle_id: AccountId,
    pub feed_ids: Vec<FeedId>,
    pub payload: Base64Bytes,
}

write_method_spec!(
    /// Submit RedStone price payloads.
    "redstone.writePrices": WritePrices(WritePricesBody)
);
