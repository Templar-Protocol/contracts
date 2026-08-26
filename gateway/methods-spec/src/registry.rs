use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{
    common::Pagination, contract::ContractKind, primitive::PublicKey, Base64Bytes, NearToken,
};

/// List deployments in a registry.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.listDeployments", output = ListDeploymentsResult)]
pub struct ListDeployments {
    pub registry_id: AccountId,
    #[serde(flatten)]
    #[method(default)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListDeploymentsResult {
    pub account_ids: Vec<AccountId>,
}

/// List deployments in a registry filtered by contract kind.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.listDeploymentsByKind", output = ListDeploymentsResult)]
pub struct ListDeploymentsByKind {
    pub registry_id: AccountId,
    #[serde(flatten)]
    #[method(default)]
    pub args: Pagination,
    pub kind: ContractKind,
}

/// List versions in a registry.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.listVersions", output = ListVersionsResult)]
pub struct ListVersions {
    pub registry_id: AccountId,
    #[serde(flatten)]
    #[method(default)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListVersionsResult {
    pub values: Vec<String>,
}

/// Get a deployment record from a registry.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.getDeployment", output = GetDeploymentResult)]
pub struct GetDeployment {
    pub registry_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetDeploymentResult {
    pub deployment: Option<templar_common::registry::Deployment>,
}

/// Get a name's registry entry, including one merely reserved by an in-flight deploy.
///
/// Unlike `registry.getDeployment`, which reports a reserved name as absent even though `deploy`
/// would refuse it.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.getRegistryEntry", output = GetRegistryEntryResult)]
pub struct GetRegistryEntry {
    pub registry_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetRegistryEntryResult {
    pub entry: Option<templar_common::registry::RegistryEntryView>,
}

/// Get a registered version's code hash and whether it can still be deployed.
///
/// Unlike `registry.listVersions`, which keeps listing a version whose code was removed.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "registry.getVersion", output = GetVersionResult)]
pub struct GetVersion {
    pub registry_id: AccountId,
    pub version_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetVersionResult {
    pub version: Option<templar_common::registry::VersionInfo>,
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
    #[serde(default)]
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

/// Remove a version from a registry.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "registry.removeVersion")]
pub struct RemoveVersion {
    pub registry_id: AccountId,
    pub version_key: String,
}
