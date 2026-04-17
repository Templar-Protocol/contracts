use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::{proxy::Proxy, pyth::PriceIdentifier};

use crate::macros::public_read_method_spec;

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

public_read_method_spec!(
    ListProxies,
    "proxyOracle.listProxies",
    ListProxiesParams,
    ListProxiesResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProxyParams {
    pub oracle_id: near_account_id::AccountId,
    pub id: PriceIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProxyResult {
    pub proxy: Option<Proxy>,
}

public_read_method_spec!(
    GetProxy,
    "proxyOracle.getProxy",
    GetProxyParams,
    GetProxyResult
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

public_read_method_spec!(
    PriceFeedExists,
    "proxyOracle.priceFeedExists",
    PriceFeedExistsParams,
    PriceFeedExistsResult
);
