use crate::request::OracleRequest;
use near_sdk::near;

use super::ProxyPriceTransformer;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Source {
    Request(OracleRequest),
    Transformer(ProxyPriceTransformer),
}

impl From<ProxyPriceTransformer> for Source {
    fn from(transformer: ProxyPriceTransformer) -> Self {
        Self::Transformer(transformer)
    }
}

impl From<OracleRequest> for Source {
    fn from(oracle_price: OracleRequest) -> Self {
        Self::Request(oracle_price)
    }
}
