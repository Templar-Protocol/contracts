use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{BlockSummary, CryptoHash};

/// Fetch summary header information for a block.
///
/// `block_hash` selects a specific block; omit it for the latest final block.
/// The result ([`BlockSummary`]) carries the block's `gas_price`, so a caller
/// needing only a current gas estimate can read it from the latest block.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[method(read = "chain.getBlock", output = BlockSummary)]
pub struct GetBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<CryptoHash>,
}
