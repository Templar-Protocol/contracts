use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::{AccountIdList, Pagination, StringList, WriteOperationResult},
    Base64Bytes, PublicReadMethod, RegistryId, RegistryReadMethod, RegistryWriteMethod,
    WriteMethod,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListDeploymentsParams {
    pub registry_id: RegistryId,
    #[serde(flatten)]
    pub args: Pagination,
}

pub type ListDeploymentsResult = AccountIdList;

public_read_method_spec!(
    ListDeployments,
    "registry.listDeployments",
    PublicReadMethod::Registry(RegistryReadMethod::ListDeployments),
    ListDeploymentsParams,
    ListDeploymentsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListVersionsParams {
    pub registry_id: RegistryId,
    #[serde(flatten)]
    pub args: Pagination,
}

pub type ListVersionsResult = StringList;

public_read_method_spec!(
    ListVersions,
    "registry.listVersions",
    PublicReadMethod::Registry(RegistryReadMethod::ListVersions),
    ListVersionsParams,
    ListVersionsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeployBody {
    pub registry_id: RegistryId,
    pub name: String,
    pub version_key: String,
    pub init_args: Base64Bytes,
    pub full_access_keys: Option<Vec<String>>,
    pub deposit: crate::NearToken,
}

pub type DeployResult = WriteOperationResult;

write_method_spec!(
    Deploy,
    "registry.deploy",
    WriteMethod::Registry(RegistryWriteMethod::Deploy),
    DeployBody,
    DeployResult
);
