use near_sdk::near;

use super::{price_transformer::ProxyPriceTransformer, time::Milliseconds, OracleRequest};

pub mod aggregator;
use aggregator::{Aggregator, Filter};
pub mod governance;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Proxy {
    pub aggregator: Aggregator,
    pub entries: Vec<Entry>,
}

impl Proxy {
    pub fn median_low(entries: impl IntoIterator<Item = Source>) -> Self {
        Self {
            aggregator: Aggregator::median_low(Filter {
                max_age: Some(Milliseconds::from_ms(60 * 1000)),
                max_clock_drift: Some(Milliseconds::from_ms(10 * 1000)),
                min_sources: Some(1),
            }),
            entries: entries.into_iter().map(|s| Entry::new(s, 1)).collect(),
        }
    }

    pub fn priority(entries: impl IntoIterator<Item = Source>) -> Self {
        Self {
            aggregator: Aggregator::priority(Filter {
                max_age: Some(Milliseconds::from_ms(60 * 1000)),
                max_clock_drift: Some(Milliseconds::from_ms(10 * 1000)),
                min_sources: Some(1),
            }),
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
