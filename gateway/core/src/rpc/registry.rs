use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{read_method_spec, write_method_spec},
    primitive::PublicKey,
    rpc::common::Pagination,
    Base64Bytes, NearToken, RegistryId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListDeploymentsParams {
    pub registry_id: RegistryId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListDeploymentsResult {
    pub account_ids: Vec<near_account_id::AccountId>,
}

read_method_spec!(
    /// List deployments in a registry.
    "registry.listDeployments": ListDeployments(ListDeploymentsParams) -> ListDeploymentsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListVersionsParams {
    pub registry_id: RegistryId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListVersionsResult {
    pub values: Vec<String>,
}

read_method_spec!(
    /// List versions in a registry.
    "registry.listVersions": ListVersions(ListVersionsParams) -> ListVersionsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetDeploymentParams {
    pub registry_id: RegistryId,
    pub account_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetDeploymentResult {
    pub deployment: Option<templar_common::registry::Deployment>,
}

read_method_spec!(
    /// Get a deployment record from a registry.
    "registry.getDeployment": GetDeployment(GetDeploymentParams) -> GetDeploymentResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddVersionBody {
    pub registry_id: RegistryId,
    pub version_key: String,
    pub deploy_mode: templar_common::registry::DeployMode,
    pub code: Base64Bytes,
    pub deposit: NearToken,
}

write_method_spec!(
    /// Add a deployable version to a registry.
    "registry.addVersion": AddVersion(AddVersionBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeployBody {
    pub registry_id: RegistryId,
    pub name: String,
    pub version_key: String,
    pub init_args: Base64Bytes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_access_keys: Option<Vec<PublicKey>>,
    pub deposit: crate::NearToken,
}

write_method_spec!(
    /// Deploy a contract from a registry version.
    "registry.deploy": Deploy(DeployBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoveVersionBody {
    pub registry_id: RegistryId,
    pub version_key: String,
}

write_method_spec!(
    /// Remove a version from a registry.
    "registry.removeVersion": RemoveVersion(RemoveVersionBody)
);
