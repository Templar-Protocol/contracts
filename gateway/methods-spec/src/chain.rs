use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{CryptoHash, NearToken};

/// Fetch the current gas price (yoctoNEAR per unit of gas).
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[method(read = "chain.getGasPrice", output = GetGasPriceResult)]
pub struct GetGasPrice {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetGasPriceResult {
    pub gas_price: NearToken,
}

/// Fetch summary header information for a block.
///
/// `block_hash` selects a specific block; omit it for the latest final block.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[method(read = "chain.getBlock", output = GetBlockResult)]
pub struct GetBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<CryptoHash>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBlockResult {
    pub height: u64,
    /// Block timestamp in nanoseconds since the Unix epoch.
    pub timestamp_ns: u64,
}
