use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::{price_transformer::PriceTransformer, pyth::PriceIdentifier};

use crate::{macros::public_read_method_spec, rpc::common::Pagination};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOracleIdParams {
    pub oracle_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOracleIdResult {
    pub pyth_oracle_id: AccountId,
}

public_read_method_spec!(
    GetOracleId,
    "lstOracle.getOracleId",
    GetOracleIdParams,
    GetOracleIdResult
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

public_read_method_spec!(
    ListTransformers,
    "lstOracle.listTransformers",
    ListTransformersParams,
    ListTransformersResult
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

public_read_method_spec!(
    GetTransformer,
    "lstOracle.getTransformer",
    GetTransformerParams,
    GetTransformerResult
);
