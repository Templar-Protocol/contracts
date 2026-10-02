use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{
    common::Pagination, contract::ContractKind, primitive::PublicKey, Base64Bytes, NearToken,
    RegistryVersion,
};

/// List deployments in a registry.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.listDeployments", output = Vec<AccountId>)]
pub struct ListDeployments {
    pub registry_id: AccountId,
    #[serde(flatten)]
    #[method(default)]
    pub args: Pagination,
}

/// List deployments in a registry filtered by contract kind.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.listDeploymentsByKind", output = Vec<AccountId>)]
pub struct ListDeploymentsByKind {
    pub registry_id: AccountId,
    #[serde(flatten)]
    #[method(default)]
    pub args: Pagination,
    pub kind: ContractKind,
}

/// List versions in a registry.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.listVersions", output = Vec<String>)]
pub struct ListVersions {
    pub registry_id: AccountId,
    #[serde(flatten)]
    #[method(default)]
    pub args: Pagination,
}

/// Get a deployment record from a registry.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.getDeployment", output = Option<templar_common::registry::Deployment>)]
pub struct GetDeployment {
    pub registry_id: AccountId,
    pub account_id: AccountId,
}

/// Get a name's registry entry, including one merely reserved by an in-flight deploy.
///
/// Unlike `registry.getDeployment`, which reports a reserved name as absent even though `deploy`
/// would refuse it.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.getRegistryEntry", output = Option<templar_common::registry::RegistryEntryView>)]
pub struct GetRegistryEntry {
    pub registry_id: AccountId,
    pub account_id: AccountId,
}

/// Get a registered version's code hash and whether it can still be deployed.
///
/// Unlike `registry.listVersions`, which keeps listing a version whose code was removed.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.getVersion", output = Option<templar_common::registry::VersionInfo>)]
pub struct GetVersion {
    pub registry_id: AccountId,
    pub version_key: String,
}

/// Get the code hash a registered version reports, served by every registry release.
///
/// Unlike `registry.getVersion`, it cannot tell a removed version from a deployable one.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.getVersionCodeHash", output = Option<templar_gateway_types::CryptoHash>)]
pub struct GetVersionCodeHash {
    pub registry_id: AccountId,
    pub version_key: String,
}

/// Add a deployable version to a registry.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "registry.addVersion")]
pub struct AddVersion {
    pub registry_id: AccountId,
    pub version_key: String,
    pub source: templar_common::registry::VersionSource,
    pub deposit: NearToken,
}

/// The fields every deploy-from-registry method shares, flattened into each so they
/// stay at the top level of the wire JSON.
///
/// A contract this codebase models gets its own `<namespace>.create` declaring only
/// its init fields beside this, dispatched through `plan_create_from_registry`.
/// Unmodeled contracts use [`Deploy`]; its JSON init args are validated against the
/// target constructor ABI unless `skip_abi_check` is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeployTarget {
    pub registry_id: AccountId,
    pub name: String,
    pub version_key: String,
    /// Skip embedded-ABI validation for a deployment whose constructor cannot be checked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_abi_check: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_access_keys: Option<Vec<PublicKey>>,
    pub deposit: NearToken,
}

/// Deploy a contract from a registry version with JSON init args validated against
/// the target constructor ABI unless `skip_abi_check` is set.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "registry.deploy")]
pub struct Deploy {
    #[serde(flatten)]
    pub target: DeployTarget,
    pub init_args: Base64Bytes,
}

/// The state layout of a registry that predates versioned state, which is what names its
/// migration. Every such registry reports a stored state version of 0, so the layout follows from
/// the release it runs and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Migration {
    /// Releases before 1.1.0.
    PreGlobalContracts,
    /// Releases from 1.1.0 until versioned state.
    WithGlobalContracts,
}

impl Migration {
    /// The migration a registry at `version` needs, or `None` once it keeps a state version and
    /// upgrades itself through `upgrade`.
    pub fn for_version(version: RegistryVersion) -> Option<Self> {
        if version.supports_upgrade() {
            None
        } else if version.supports_global_contracts() {
            Some(Self::WithGlobalContracts)
        } else {
            Some(Self::PreGlobalContracts)
        }
    }
}

/// Replace the code of a registry that predates versioned state and migrate its state in the same
/// transaction, signed by the registry account itself.
///
/// The deploy and `migrate` share a receipt, so a migration that fails reverts the new code with
/// it. Refused unless `migration` is the one the registry's reported version needs and `wasm` is a
/// catalogued registry release with versioned state.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "registry.upgrade")]
pub struct Upgrade {
    pub registry_id: AccountId,
    pub wasm: Base64Bytes,
    pub migration: Migration,
}

/// Remove a version from a registry.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "registry.removeVersion")]
pub struct RemoveVersion {
    pub registry_id: AccountId,
    pub version_key: String,
}
