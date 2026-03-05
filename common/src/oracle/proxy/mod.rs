use std::ops::Deref;

use near_sdk::{near, AccountId};

use super::{price_transformer::ProxyPriceTransformer, OracleRequest};

pub mod governance;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Proxy(pub Vec<ProxyEntry>);

impl Deref for Proxy {
    type Target = [ProxyEntry];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<ProxyEntry>> for Proxy {
    fn from(proxies: Vec<ProxyEntry>) -> Self {
        Self(proxies)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum ProxyEntry {
    Request(OracleRequest),
    Transformer(ProxyPriceTransformer),
}

impl From<ProxyPriceTransformer> for ProxyEntry {
    fn from(transformer: ProxyPriceTransformer) -> Self {
        Self::Transformer(transformer)
    }
}

impl From<OracleRequest> for ProxyEntry {
    fn from(oracle_price: OracleRequest) -> Self {
        Self::Request(oracle_price)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub enum Oracle {
    Pyth,
    RedStone,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub enum OracleType {
    Pyth(AccountId),
    RedStone(AccountId),
}
