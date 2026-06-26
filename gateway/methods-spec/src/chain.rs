use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::NearToken;

/// Fetch the current gas price (yoctoNEAR per unit of gas).
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[method(read = "chain.getGasPrice", output = GetGasPriceResult)]
pub struct GetGasPrice {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetGasPriceResult {
    pub gas_price: NearToken,
}
