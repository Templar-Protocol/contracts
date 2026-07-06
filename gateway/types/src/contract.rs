use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[cfg_attr(feature = "clap", value(rename_all = "snake_case"))]
pub enum ContractKind {
    Unknown,
    Registry,
    Market,
    Vault,
    ProxyOracle,
    ProxyGovernance,
    LstOracle,
    UniversalAccount,
    RedstoneOracle,
    PythOracle,
}
