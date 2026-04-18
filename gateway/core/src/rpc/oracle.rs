use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::{
    pyth::{self, OracleResponse, PriceIdentifier},
    redstone, OracleRequest,
};

use crate::macros::public_read_method_spec;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OracleContractKind {
    Direct,
    Lst { pyth_id: near_account_id::AccountId },
    Proxy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetKindParams {
    pub oracle_id: near_account_id::AccountId,
}

pub type GetKindResult = OracleContractKind;

public_read_method_spec!(GetKind, "oracle.getKind", GetKindParams, GetKindResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPriceResolutionDependenciesParams {
    pub oracle_id: near_account_id::AccountId,
    pub price_id: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPriceResolutionDependenciesResult {
    pub kind: OracleContractKind,
    pub requests: Vec<OracleRequest>,
}

public_read_method_spec!(
    GetPriceResolutionDependencies,
    "oracle.getPriceResolutionDependencies",
    GetPriceResolutionDependenciesParams,
    GetPriceResolutionDependenciesResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PythOraclePrices {
    pub oracle_id: near_account_id::AccountId,
    pub response: OracleResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RedStonePriceEntry {
    pub feed_id: redstone::FeedId,
    pub data: redstone::FeedData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RedStoneOraclePrices {
    pub oracle_id: near_account_id::AccountId,
    pub response: Vec<RedStonePriceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePricesParams {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
    pub age: u64,
    pub pyth: Vec<PythOraclePrices>,
    pub redstone: Vec<RedStoneOraclePrices>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePriceParams {
    pub oracle_id: near_account_id::AccountId,
    pub price_id: PriceIdentifier,
    pub age: u64,
    pub pyth: Vec<PythOraclePrices>,
    pub redstone: Vec<RedStoneOraclePrices>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePriceResult {
    pub price: Option<pyth::Price>,
}

public_read_method_spec!(
    ResolvePrice,
    "oracle.resolvePrice",
    ResolvePriceParams,
    ResolvePriceResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedPrice {
    pub price_id: PriceIdentifier,
    pub price: Option<pyth::Price>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePricesResult {
    pub prices: Vec<ResolvedPrice>,
}

public_read_method_spec!(
    ResolvePrices,
    "oracle.resolvePrices",
    ResolvePricesParams,
    ResolvePricesResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPricesParams {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
    pub age: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPriceParams {
    pub oracle_id: near_account_id::AccountId,
    pub price_id: PriceIdentifier,
    pub age: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPriceResult {
    pub price: Option<pyth::Price>,
}

public_read_method_spec!(GetPrice, "oracle.getPrice", GetPriceParams, GetPriceResult);

pub type GetPricesResult = ResolvePricesResult;

public_read_method_spec!(
    GetPrices,
    "oracle.getPrices",
    GetPricesParams,
    GetPricesResult
);
