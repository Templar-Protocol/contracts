use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::common::Pagination;
use templar_proxy_oracle_near_common::price_transformer::PriceTransformer;

/// Get the backing Pyth oracle for an LST oracle.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "lstOracle.getOracleId", output = AccountId)]
pub struct GetOracleId {
    pub oracle_id: AccountId,
}

/// List transformer price IDs on an LST oracle.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "lstOracle.listTransformers", output = Vec<PriceIdentifier>)]
pub struct ListTransformers {
    pub oracle_id: AccountId,
    #[serde(flatten)]
    pub pagination: Pagination,
}

/// Get a transformer definition for a price ID.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "lstOracle.getTransformer", output = Option<PriceTransformer>)]
pub struct GetTransformer {
    pub oracle_id: AccountId,
    pub price_identifier: PriceIdentifier,
}

/// Create a transformer for a price ID. Owner-gated, and charged a 1-yoctoNEAR
/// confirmation deposit by the contract.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "lstOracle.createTransformer")]
pub struct CreateTransformer {
    pub oracle_id: AccountId,
    pub price_identifier: PriceIdentifier,
    pub entry: PriceTransformer,
}
