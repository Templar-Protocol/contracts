use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::pyth::{Price, PriceIdentifier};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::Base64Bytes;
use templar_proxy_oracle_kernel::proxy::{circuit_breaker::CircuitBreakerSet, Proxy};
use templar_proxy_oracle_near_common::input::Source;

/// Create a proxy oracle from the registry.
///
/// `owner_id` seats the owner at init; omitting it leaves the registry as owner.
/// Constructor ABI validation normally rejects a target that cannot honor it.
/// `skip_abi_check` disables that protection and can leave the registry as owner.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracle.create")]
pub struct Create {
    #[serde(flatten)]
    pub target: crate::registry::DeployTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<near_account_id::AccountId>,
}

/// List proxy price feeds.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracle.listProxies", output = Vec<PriceIdentifier>)]
pub struct ListProxies {
    pub oracle_id: near_account_id::AccountId,
    pub offset: Option<u32>,
    pub count: Option<u32>,
}

/// Read a proxy's accepted cached price, subject to its stored breaker state and age.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracle.getCachedPrice", output = Option<Price>)]
pub struct GetCachedPrice {
    pub oracle_id: near_account_id::AccountId,
    pub id: PriceIdentifier,
    pub age: u64,
}

/// Resolve on-chain sources only when every transformed source has a strictly
/// positive confidence interval and is fresh under both the deployed filter and
/// `age` (seconds), at the chain clock. Future publications are rejected.
/// This does not read the accepted cache or certify breaker state; it establishes
/// all-source readiness before aggregation.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracle.getFreshSourcePrice", output = Option<Price>)]
pub struct GetFreshSourcePrice {
    pub oracle_id: near_account_id::AccountId,
    pub id: PriceIdentifier,
    pub age: u64,
}

/// Get a proxy price feed definition.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracle.getProxy", output = Option<Proxy<Source>>)]
pub struct GetProxy {
    pub oracle_id: near_account_id::AccountId,
    pub id: PriceIdentifier,
}

/// Check whether a proxy price feed exists.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracle.priceFeedExists", output = bool)]
pub struct PriceFeedExists {
    pub oracle_id: near_account_id::AccountId,
    pub price_identifier: PriceIdentifier,
}

/// Get the circuit breaker set configured for a proxy price feed.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracle.getProxyCircuitBreakerSet", output = Option<CircuitBreakerSet>)]
pub struct GetProxyCircuitBreakerSet {
    pub oracle_id: near_account_id::AccountId,
    pub id: PriceIdentifier,
}

/// Refresh the proxy oracle's cached prices for the given feeds.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracle.updatePrices")]
pub struct UpdatePrices {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
}

/// Set (or clear, with a null `proxy`) a proxy price feed definition.
///
/// The oracle's own owner-gated write. In a `>= 0.2.0` deployment whose owner is
/// a governance contract, the equivalent action is a
/// [`proxyOracleGovernance.createProposal`](crate::proxy_oracle_governance::CreateProposal)
/// carrying `Operation::SetProxy`.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracle.adminSetProxy")]
pub struct AdminSetProxy {
    pub oracle_id: near_account_id::AccountId,
    pub id: PriceIdentifier,
    pub proxy: Option<Proxy<Source>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Migration {
    V0,
}

/// Upgrade a proxy oracle with supplied WASM and an explicit state migration.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracle.upgrade")]
pub struct Upgrade {
    pub oracle_id: near_account_id::AccountId,
    pub wasm: Base64Bytes,
    pub migration: Migration,
}
