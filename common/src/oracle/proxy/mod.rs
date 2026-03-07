use near_sdk::{near, AccountId};

use super::{price_transformer::ProxyPriceTransformer, OracleRequest};

pub mod aggregator;
use aggregator::{Aggregator, Confidence, Sample};
pub mod governance;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Proxy {
    pub aggregator: Aggregator,
    pub entries: Vec<Entry>,
}

impl Proxy {
    pub fn median(entries: impl IntoIterator<Item = Source>) -> Self {
        Self {
            aggregator: Aggregator {
                confidence: Confidence::MedianLow { ignore_zeros: true },
                sample: Sample::MedianLow,
            },
            entries: entries.into_iter().map(|s| Entry::new(s, 1)).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Entry {
    pub source: Source,
    pub weight: u32,
}

impl Entry {
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
