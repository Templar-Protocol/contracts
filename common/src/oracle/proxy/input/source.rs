use near_sdk::near;

use crate::oracle::OracleRequest;

use super::ProxyPriceTransformer;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct WeightedSource {
    pub source: Source,
    pub weight: u32,
}

impl WeightedSource {
    pub fn new(source: impl Into<Source>, weight: u32) -> Self {
        Self {
            source: source.into(),
            weight,
        }
    }
}

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
