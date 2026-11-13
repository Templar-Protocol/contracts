use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Observed token metadata; this does not certify conversion or value equivalence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenMetadata {
    pub decimals: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}
