use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContractKind {
    Unknown,
    Registry,
    Market,
    ProxyOracle,
    ProxyGovernance,
    LstOracle,
    UniversalAccount,
    RedstoneOracle,
    PythOracle,
}
