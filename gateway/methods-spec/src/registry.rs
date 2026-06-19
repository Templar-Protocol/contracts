use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::{read_method_spec, write_method_spec};
use templar_gateway_types::{
    common::Pagination, contract::ContractKind, primitive::PublicKey, Base64Bytes, NearToken,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListDeployments {
    pub registry_id: AccountId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListDeploymentsResult {
    pub account_ids: Vec<AccountId>,
}

read_method_spec!(
    /// List deployments in a registry.
    "registry.listDeployments": ListDeployments -> ListDeploymentsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListDeploymentsByKind {
    pub registry_id: AccountId,
    #[serde(flatten)]
    pub args: Pagination,
    pub kind: ContractKind,
}

read_method_spec!(
    /// List deployments in a registry filtered by contract kind.
    "registry.listDeploymentsByKind": ListDeploymentsByKind -> ListDeploymentsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListVersions {
    pub registry_id: AccountId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListVersionsResult {
    pub values: Vec<String>,
}

read_method_spec!(
    /// List versions in a registry.
    "registry.listVersions": ListVersions -> ListVersionsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetDeployment {
    pub registry_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetDeploymentResult {
    pub deployment: Option<templar_common::registry::Deployment>,
}

read_method_spec!(
    /// Get a deployment record from a registry.
    "registry.getDeployment": GetDeployment -> GetDeploymentResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddVersion {
    pub registry_id: AccountId,
    pub version_key: String,
    pub deploy_mode: templar_common::registry::DeployMode,
    pub code: Base64Bytes,
    pub deposit: NearToken,
}

write_method_spec!(
    /// Add a deployable version to a registry.
    "registry.addVersion": AddVersion
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Deploy {
    pub registry_id: AccountId,
    pub name: String,
    pub version_key: String,
    pub init_args: Base64Bytes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_access_keys: Option<Vec<PublicKey>>,
    pub deposit: NearToken,
}

write_method_spec!(
    /// Deploy a contract from a registry version.
    "registry.deploy": Deploy
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoveVersion {
    pub registry_id: AccountId,
    pub version_key: String,
}

write_method_spec!(
    /// Remove a version from a registry.
    "registry.removeVersion": RemoveVersion
);
