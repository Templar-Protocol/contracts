use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::{
    pyth::{self, OracleResponse, PriceIdentifier},
    redstone, OracleRequest,
};

use crate::{
    macros::{read_method_spec, write_method_spec},
    Base64Bytes,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OracleContractKind {
    Direct,
    Lst { pyth_id: near_account_id::AccountId },
    Proxy,
}

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

read_method_spec!(
    /// Get update dependencies for a price.
    "oracle.getPriceResolutionDependencies": GetPriceResolutionDependencies(GetPriceResolutionDependenciesParams) -> GetPriceResolutionDependenciesResult
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

read_method_spec!(
    /// Resolve a single price from supplied inputs.
    "oracle.resolvePrice": ResolvePrice(ResolvePriceParams) -> ResolvePriceResult
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

read_method_spec!(
    /// Resolve multiple prices from supplied inputs.
    "oracle.resolvePrices": ResolvePrices(ResolvePricesParams) -> ResolvePricesResult
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

read_method_spec!(
    /// Read a single on-chain oracle price.
    "oracle.getPrice": GetPrice(GetPriceParams) -> GetPriceResult
);

pub type GetPricesResult = ResolvePricesResult;

read_method_spec!(
    /// Read multiple on-chain oracle prices.
    "oracle.getPrices": GetPrices(GetPricesParams) -> GetPricesResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePythBody {
    pub oracle_id: near_account_id::AccountId,
    pub vaa: Base64Bytes,
}

write_method_spec!(
    /// Submit a Pyth oracle update.
    "oracle.updatePyth": UpdatePyth(UpdatePythBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateRedStoneBody {
    pub oracle_id: near_account_id::AccountId,
    pub feed_id: redstone::FeedId,
}

write_method_spec!(
    /// Submit a RedStone oracle update.
    "oracle.updateRedStone": UpdateRedStone(UpdateRedStoneBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePricesBody {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
}

write_method_spec!(
    /// Submit all updates needed for prices.
    "oracle.updatePrices": UpdatePrices(UpdatePricesBody)
);
