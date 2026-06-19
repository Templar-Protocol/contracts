use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::redstone::{Config, FeedData, FeedId, Role};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::Base64Bytes;

/// Get RedStone oracle config.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "redstone.getConfig", output = GetConfigResult)]
pub struct GetConfig {
    pub oracle_id: AccountId,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetConfigResult {
    pub config: Config,
}

/// Read RedStone price data.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "redstone.readPriceData", output = ReadPriceDataResult)]
pub struct ReadPriceData {
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

/// List accounts for a RedStone role.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "redstone.listRole", output = ListRoleResult)]
pub struct ListRole {
    pub oracle_id: AccountId,
    pub role: RoleValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListRoleResult {
    pub account_ids: Vec<AccountId>,
}

/// Update a RedStone role membership.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "redstone.setRole")]
pub struct SetRole {
    pub oracle_id: AccountId,
    pub account_id: AccountId,
    pub role: RoleValue,
    pub set: bool,
}

/// Submit RedStone price payloads.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "redstone.writePrices")]
pub struct WritePrices {
    pub oracle_id: AccountId,
    pub feed_ids: Vec<FeedId>,
    pub payload: Base64Bytes,
}
