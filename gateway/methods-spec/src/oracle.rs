use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::{
    lazer,
    pyth::{self, OracleResponse, PriceIdentifier},
    redstone,
};
use templar_gateway_macros::MethodSpec;
pub use templar_gateway_types::OracleContractKind;
use templar_proxy_oracle_near_common::request::OracleRequest;

/// Get update dependencies for a price.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "oracle.getPriceResolutionDependencies", output = GetPriceResolutionDependenciesResult)]
pub struct GetPriceResolutionDependencies {
    pub oracle_id: near_account_id::AccountId,
    pub price_id: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPriceResolutionDependenciesResult {
    pub kind: OracleContractKind,
    pub requests: Vec<OracleRequest>,
}

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

/// A single Lazer feed's raw stored data, keyed by native `u32` feed id. Carries the adapter's
/// `FeedData` (not a projected price), mirroring [`RedStonePriceEntry`]; `resolvePrice` projects it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LazerPriceEntry {
    pub feed_id: u32,
    pub data: lazer::FeedData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LazerOraclePrices {
    pub oracle_id: near_account_id::AccountId,
    pub response: Vec<LazerPriceEntry>,
}

/// Resolve a single price from supplied inputs.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "oracle.resolvePrice", output = ResolvePriceResult)]
pub struct ResolvePrice {
    pub oracle_id: near_account_id::AccountId,
    pub price_id: PriceIdentifier,
    pub age: u64,
    pub pyth: Vec<PythOraclePrices>,
    pub redstone: Vec<RedStoneOraclePrices>,
    #[serde(default)]
    pub lazer: Vec<LazerOraclePrices>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePriceResult {
    pub price: Option<pyth::Price>,
}

/// Resolve multiple prices from supplied inputs.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "oracle.resolvePrices", output = ResolvePricesResult)]
pub struct ResolvePrices {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
    pub age: u64,
    pub pyth: Vec<PythOraclePrices>,
    pub redstone: Vec<RedStoneOraclePrices>,
    #[serde(default)]
    pub lazer: Vec<LazerOraclePrices>,
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

/// Read a single on-chain oracle price.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "oracle.getPrice", output = GetPriceResult)]
pub struct GetPrice {
    pub oracle_id: near_account_id::AccountId,
    pub price_id: PriceIdentifier,
    pub age: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetPriceResult {
    pub price: Option<pyth::Price>,
}

/// Read multiple on-chain oracle prices.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "oracle.getPrices", output = ResolvePricesResult)]
pub struct GetPrices {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
    pub age: u64,
}
