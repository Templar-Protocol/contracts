use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::redstone::{Config, FeedData, FeedId, Role};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::Base64Bytes;

/// Create a RedStone price adapter from the registry.
///
/// `admin_id` seats the adapter's administration roles at init. Registry
/// deployment normally validates the target constructor ABI before it plans the
/// transaction; `skip_abi_check` disables that protection.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "redstone.create")]
pub struct Create {
    #[serde(flatten)]
    pub target: crate::registry::DeployTarget,
    pub config: Config,
    pub admin_id: AccountId,
}

/// Get RedStone oracle config.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "redstone.getConfig", output = Config)]
pub struct GetConfig {
    pub oracle_id: AccountId,
}

/// Read RedStone price data.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "redstone.readPriceData", output = Vec<PriceDataEntry>)]
pub struct ReadPriceData {
    pub oracle_id: AccountId,
    pub feed_ids: Vec<FeedId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PriceDataEntry {
    pub feed_id: FeedId,
    pub data: FeedData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoleValue {
    #[serde(alias = "admin")]
    ModifyRoles,
    TrustedUpdater,
}

impl RoleValue {
    /// Administrative authority with the legacy gateway wire spelling.
    #[allow(
        non_upper_case_globals,
        reason = "variant-style alias preserves the public ModifyRoles variant"
    )]
    pub const Admin: Self = Self::ModifyRoles;
}

impl From<RoleValue> for Role {
    fn from(value: RoleValue) -> Self {
        match value {
            RoleValue::ModifyRoles => Self::Admin,
            RoleValue::TrustedUpdater => Self::TrustedUpdater,
        }
    }
}

/// List accounts for a RedStone role.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "redstone.listRole", output = Vec<AccountId>)]
pub struct ListRole {
    pub oracle_id: AccountId,
    pub role: RoleValue,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn role_inputs_preserve_canonical_wire_values() {
        for (input, expected, canonical, contract_role) in [
            (
                "modify_roles",
                RoleValue::Admin,
                "modify_roles",
                Role::Admin,
            ),
            ("admin", RoleValue::Admin, "modify_roles", Role::Admin),
            (
                "trusted_updater",
                RoleValue::TrustedUpdater,
                "trusted_updater",
                Role::TrustedUpdater,
            ),
        ] {
            let role: RoleValue = serde_json::from_value(json!(input)).unwrap();
            assert_eq!(role, expected);
            assert_eq!(serde_json::to_value(role).unwrap(), canonical);
            let role: Role = role.into();
            assert_eq!(role, contract_role);
            let canonical = match role {
                Role::Admin => "ModifyRoles",
                Role::TrustedUpdater => "TrustedUpdater",
            };
            assert_eq!(serde_json::to_value(role).unwrap(), canonical);
        }
    }

    #[test]
    fn role_schema_retains_only_legacy_canonical_values() {
        let schema = serde_json::to_value(schemars::schema_for!(RoleValue)).unwrap();
        assert_eq!(schema["enum"], json!(["modify_roles", "trusted_updater"]));
    }

    #[test]
    fn legacy_role_variants_remain_importable_and_exhaustive() {
        use RoleValue::{ModifyRoles, TrustedUpdater};

        let role = match RoleValue::Admin {
            ModifyRoles => Role::Admin,
            TrustedUpdater => Role::TrustedUpdater,
        };
        assert_eq!(role, Role::Admin);
        assert!(matches!(ModifyRoles, RoleValue::Admin));
    }
}
