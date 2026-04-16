use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    primitive::PublicKey,
    rpc::common::{Pagination, WriteOperationResult},
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

public_read_method_spec!(
    ListDeployments,
    "registry.listDeployments",
    ListDeploymentsParams,
    ListDeploymentsResult
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

public_read_method_spec!(
    ListVersions,
    "registry.listVersions",
    ListVersionsParams,
    ListVersionsResult
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

public_read_method_spec!(
    GetDeployment,
    "registry.getDeployment",
    GetDeploymentParams,
    GetDeploymentResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddVersionBody {
    pub registry_id: RegistryId,
    pub version_key: String,
    pub deploy_mode: templar_common::registry::DeployMode,
    pub code: Base64Bytes,
    pub deposit: NearToken,
}

pub type AddVersionResult = WriteOperationResult;

write_method_spec!(
    AddVersion,
    "registry.addVersion",
    AddVersionBody,
    AddVersionResult
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

pub type DeployResult = WriteOperationResult;

write_method_spec!(Deploy, "registry.deploy", DeployBody, DeployResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoveVersionBody {
    pub registry_id: RegistryId,
    pub version_key: String,
}

pub type RemoveVersionResult = WriteOperationResult;

write_method_spec!(
    RemoveVersion,
    "registry.removeVersion",
    RemoveVersionBody,
    RemoveVersionResult
);
