use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::common::Pagination;
use templar_proxy_oracle_near_common::price_transformer::PriceTransformer;

/// Get the backing Pyth oracle for an LST oracle.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "lstOracle.getOracleId", output = GetOracleIdResult)]
pub struct GetOracleId {
    pub oracle_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOracleIdResult {
    pub pyth_oracle_id: AccountId,
}

/// List transformer price IDs on an LST oracle.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "lstOracle.listTransformers", output = ListTransformersResult)]
pub struct ListTransformers {
    pub oracle_id: AccountId,
    #[serde(flatten)]
    pub pagination: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListTransformersResult {
    pub price_ids: Vec<PriceIdentifier>,
}

/// Get a transformer definition for a price ID.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "lstOracle.getTransformer", output = GetTransformerResult)]
pub struct GetTransformer {
    pub oracle_id: AccountId,
    pub price_identifier: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetTransformerResult {
    pub transformer: Option<PriceTransformer>,
}
