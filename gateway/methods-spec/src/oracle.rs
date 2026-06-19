use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::{
    pyth::{self, OracleResponse, PriceIdentifier},
    redstone,
};
use templar_gateway_macros::read_method_spec;
use templar_proxy_oracle_near_common::request::OracleRequest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OracleContractKind {
    Direct,
    Lst { pyth_id: near_account_id::AccountId },
    Proxy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPriceResolutionDependencies {
    pub oracle_id: near_account_id::AccountId,
    pub price_id: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPriceResolutionDependenciesResult {
    pub kind: OracleContractKind,
    pub requests: Vec<OracleRequest>,
}

read_method_spec!(
    /// Get update dependencies for a price.
    "oracle.getPriceResolutionDependencies": GetPriceResolutionDependencies -> GetPriceResolutionDependenciesResult
);

// Shared price inputs supplied to the resolve operations below.
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
pub struct ResolvePrice {
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

read_method_spec!(
    /// Resolve a single price from supplied inputs.
    "oracle.resolvePrice": ResolvePrice -> ResolvePriceResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePrices {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
    pub age: u64,
    pub pyth: Vec<PythOraclePrices>,
    pub redstone: Vec<RedStoneOraclePrices>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedPrice {
    pub price_id: PriceIdentifier,
    pub price: Option<pyth::Price>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePricesResult {
    pub prices: Vec<ResolvedPrice>,
}

read_method_spec!(
    /// Resolve multiple prices from supplied inputs.
    "oracle.resolvePrices": ResolvePrices -> ResolvePricesResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPrice {
    pub oracle_id: near_account_id::AccountId,
    pub price_id: PriceIdentifier,
    pub age: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPriceResult {
    pub price: Option<pyth::Price>,
}

read_method_spec!(
    /// Read a single on-chain oracle price.
    "oracle.getPrice": GetPrice -> GetPriceResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPrices {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
    pub age: u64,
}

pub type GetPricesResult = ResolvePricesResult;

read_method_spec!(
    /// Read multiple on-chain oracle prices.
    "oracle.getPrices": GetPrices -> GetPricesResult
);
