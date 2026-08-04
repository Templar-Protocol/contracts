use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{common::ContractArgs, contract::ContractKind, ContractMethodName};

/// Call a contract view method with arbitrary arguments.
///
/// This is the generic escape hatch for read-only contract calls when a
/// more specific typed RPC method is not available.
#[derive(MethodSpec, Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[method(read = "contract.viewFunction", output = ViewFunctionResult)]
pub struct ViewFunction {
    pub contract_id: AccountId,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewFunctionResult {
    pub value: serde_json::Value,
}

/// Read a contract version from NEP-330 metadata.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "contract.getVersion", output = VersionResult)]
pub struct GetVersion {
    pub contract_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VersionResult {
    pub version_string: String,
    pub parsed: Option<templar_gateway_types::Version<()>>,
}

/// Read a contract's state-versioning status.
///
/// Answers the `MigrateExternalInterface` views every `impl_versioned_state!`
/// contract exposes, so an operator can tell whether a deployment is behind its
/// code's target state version before upgrading it.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "contract.getStateVersion", output = GetStateVersionResult)]
pub struct GetStateVersion {
    pub contract_id: AccountId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetStateVersionResult {
    /// The version of the state currently written to the account.
    pub stored: u32,
    /// The version the deployed code expects.
    pub target: u32,
    /// The contract's own answer rather than a derived `stored != target`: a
    /// stored version *newer* than the deployed code supports is an error the
    /// contract reports by panicking, which the derived comparison would hide.
    pub needs_migration: bool,
}

/// Identify the kind of deployed protocol contract.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "contract.getKind", output = GetKindResult)]
pub struct GetKind {
    pub contract_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetKindResult {
    pub kind: ContractKind,
}
