use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{primitive::PublicKey, NearToken};
use templar_proxy_oracle_kernel::proxy::{circuit_breaker::CircuitBreakerSet, Proxy};
use templar_proxy_oracle_near_common::input::Source;

/// Create a proxy oracle from the registry.
///
/// `owner_id` seats the owner at init; omitting it leaves the registry as owner.
/// It is only honored by a version whose `new` accepts one (`>= 0.3.0`), and an
/// older `new` ignores it rather than failing — see
/// [`ProxyOracleVersion::new_accepts_owner_id`](templar_gateway_types::version::ProxyOracleVersion::new_accepts_owner_id).
/// The version is not checked here: it can only be inferred from the version key,
/// which the registry does not validate against the code.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracle.create")]
pub struct Create {
    pub registry_id: near_account_id::AccountId,
    pub name: String,
    pub version_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<near_account_id::AccountId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_access_keys: Option<Vec<PublicKey>>,
    pub deposit: NearToken,
}

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
