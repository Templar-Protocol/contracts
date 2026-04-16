use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{macros::public_read_method_spec, rpc::common::ContractArgs, ContractMethodName};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewFunctionParams {
    pub contract_id: AccountId,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewFunctionResult {
    pub value: serde_json::Value,
}

public_read_method_spec!(
    ViewFunction,
    "contract.viewFunction",
    ViewFunctionParams,
    ViewFunctionResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetVersionParams {
    pub contract_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VersionResult {
    pub version_string: String,
    pub parsed: Option<crate::Version<()>>,
}

public_read_method_spec!(
    GetVersion,
    "contract.getVersion",
    GetVersionParams,
    VersionResult
);
