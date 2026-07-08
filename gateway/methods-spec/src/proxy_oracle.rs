use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_macros::MethodSpec;
use templar_proxy_oracle_kernel::proxy::{circuit_breaker::CircuitBreakerSet, Proxy};
use templar_proxy_oracle_near_common::input::Source;

/// List proxy price feeds.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracle.listProxies", output = ListProxiesResult)]
pub struct ListProxies {
    pub oracle_id: near_account_id::AccountId,
    pub offset: Option<u32>,
    pub count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListProxiesResult {
    pub proxies: Vec<PriceIdentifier>,
}

/// Get a proxy price feed definition.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracle.getProxy", output = GetProxyResult)]
pub struct GetProxy {
    pub oracle_id: near_account_id::AccountId,
    pub id: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProxyResult {
    pub proxy: Option<Proxy<Source>>,
}

/// Check whether a proxy price feed exists.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracle.priceFeedExists", output = PriceFeedExistsResult)]
pub struct PriceFeedExists {
    pub oracle_id: near_account_id::AccountId,
    pub price_identifier: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PriceFeedExistsResult {
    pub exists: bool,
}

/// Get the circuit breaker set configured for a proxy price feed.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracle.getProxyCircuitBreakerSet", output = GetProxyCircuitBreakerSetResult)]
pub struct GetProxyCircuitBreakerSet {
    pub oracle_id: near_account_id::AccountId,
    pub id: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProxyCircuitBreakerSetResult {
    pub circuit_breaker_set: Option<CircuitBreakerSet>,
}

/// Refresh the proxy oracle's cached prices for the given feeds.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracle.updatePrices")]
pub struct UpdatePrices {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
}
