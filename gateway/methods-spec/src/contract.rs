use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::read_method_spec;
use templar_gateway_types::{common::ContractArgs, contract::ContractKind, ContractMethodName};

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

read_method_spec!(
    /// Call a contract view method with arbitrary arguments.
    ///
    /// This is the generic escape hatch for read-only contract calls when a
    /// more specific typed RPC method is not available.
    "contract.viewFunction": ViewFunction(ViewFunctionParams) -> ViewFunctionResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetVersionParams {
    pub contract_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetKindParams {
    pub contract_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetKindResult {
    pub kind: ContractKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VersionResult {
    pub version_string: String,
    pub parsed: Option<templar_gateway_types::Version<()>>,
}

impl VersionResult {
    /// The parsed version reinterpreted under a specific contract-kind tag (e.g.
    /// `Market`), or `None` if the on-chain version string did not parse.
    #[must_use]
    pub fn parsed_as<T>(&self) -> Option<templar_gateway_types::Version<T>> {
        self.parsed.map(|version| version.cast())
    }
}

read_method_spec!(
    /// Read a contract version from NEP-330 metadata.
    "contract.getVersion": GetVersion(GetVersionParams) -> VersionResult
);

read_method_spec!(
    /// Identify the kind of deployed protocol contract.
    "contract.getKind": GetKind(GetKindParams) -> GetKindResult
);
