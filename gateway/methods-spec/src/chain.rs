use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{BlockSummary, CryptoHash};

/// Fetch summary header information for a block.
///
/// `block_hash` selects a specific block; omit it for the latest final block.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[method(read = "chain.getBlock", output = BlockSummary)]
pub struct GetBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[method(default)]
    pub block_hash: Option<CryptoHash>,
}

/// Fetch the current protocol transaction limits.
#[derive(
    MethodSpec, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default,
)]
#[method(read = "chain.getProtocolLimits", output = templar_gateway_types::ProtocolLimits)]
pub struct GetProtocolLimits;
