use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_macros::read_method_spec;
use templar_gateway_types::common::Pagination;
use templar_proxy_oracle_near_common::price_transformer::PriceTransformer;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOracleIdParams {
    pub oracle_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOracleIdResult {
    pub pyth_oracle_id: AccountId,
}

read_method_spec!(
    /// Get the backing Pyth oracle for an LST oracle.
    "lstOracle.getOracleId": GetOracleId(GetOracleIdParams) -> GetOracleIdResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListTransformersParams {
    pub oracle_id: AccountId,
    #[serde(flatten)]
    pub pagination: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListTransformersResult {
    pub price_ids: Vec<PriceIdentifier>,
}

read_method_spec!(
    /// List transformer price IDs on an LST oracle.
    "lstOracle.listTransformers": ListTransformers(ListTransformersParams) -> ListTransformersResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetTransformerParams {
    pub oracle_id: AccountId,
    pub price_identifier: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetTransformerResult {
    pub transformer: Option<PriceTransformer>,
}

read_method_spec!(
    /// Get a transformer definition for a price ID.
    "lstOracle.getTransformer": GetTransformer(GetTransformerParams) -> GetTransformerResult
);
