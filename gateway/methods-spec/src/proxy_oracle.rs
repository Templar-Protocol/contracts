use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_macros::read_method_spec;
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::input::Source;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListProxiesParams {
    pub oracle_id: near_account_id::AccountId,
    pub offset: Option<u32>,
    pub count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListProxiesResult {
    pub proxies: Vec<PriceIdentifier>,
}

read_method_spec!(
    /// List proxy price feeds.
    "proxyOracle.listProxies": ListProxies(ListProxiesParams) -> ListProxiesResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProxyParams {
    pub oracle_id: near_account_id::AccountId,
    pub id: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProxyResult {
    pub proxy: Option<Proxy<Source>>,
}

read_method_spec!(
    /// Get a proxy price feed definition.
    "proxyOracle.getProxy": GetProxy(GetProxyParams) -> GetProxyResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PriceFeedExistsParams {
    pub oracle_id: near_account_id::AccountId,
    pub price_identifier: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PriceFeedExistsResult {
    pub exists: bool,
}

read_method_spec!(
    /// Check whether a proxy price feed exists.
    "proxyOracle.priceFeedExists": PriceFeedExists(PriceFeedExistsParams) -> PriceFeedExistsResult
);
