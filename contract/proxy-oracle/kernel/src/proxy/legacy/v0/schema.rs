use near_sdk::near;
use templar_common::{
    oracle::pyth::{self, PythTimestamp},
    time::Nanoseconds,
};

use crate::{
    price_transformer::{Action, Call},
    request::OracleRequest,
};

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Proxy {
    pub aggregator: Aggregator,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Entry {
    pub source: LegacySource,
    pub weight: u32,
}

impl Entry {
    pub fn new(source: impl Into<LegacySource>, weight: u32) -> Self {
        Self {
            source: source.into(),
            weight,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct ProxyPriceTransformer {
    pub request: OracleRequest,
    pub call: Call,
    pub action: Action,
}

impl ProxyPriceTransformer {
    pub fn lst(price_id: OracleRequest, decimals: u32, call: Call) -> Self {
        Self {
            request: price_id,
            call,
            action: Action::NormalizeNativeLstPrice { decimals },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum LegacySource {
    Request(OracleRequest),
    Transformer(ProxyPriceTransformer),
}

impl From<OracleRequest> for LegacySource {
    fn from(value: OracleRequest) -> Self {
        Self::Request(value)
    }
}

impl From<ProxyPriceTransformer> for LegacySource {
    fn from(value: ProxyPriceTransformer) -> Self {
        Self::Transformer(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Aggregator {
    pub method: AggregationMethod,
    pub filter: Filter,
}

impl Aggregator {
    pub fn median_low(filter: Filter) -> Self {
        Self {
            method: AggregationMethod::MedianLow,
            filter,
        }
    }

    pub fn priority(filter: Filter) -> Self {
        Self {
            method: AggregationMethod::Priority,
            filter,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum AggregationMethod {
    MedianLow,
    Priority,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[near(serializers = [json, borsh])]
pub struct Filter {
    pub max_age: Option<Nanoseconds>,
    pub max_clock_drift: Option<Nanoseconds>,
    pub min_sources: Option<u32>,
}

#[derive(Debug, Clone, Eq)]
pub struct SpecificPrice {
    pub value: i64,
    pub exponent: i32,
    pub publish_time: PythTimestamp,
}

impl From<SpecificPrice> for pyth::Price {
    fn from(s: SpecificPrice) -> Self {
        Self {
            price: s.value.into(),
            conf: 0.into(),
            expo: s.exponent,
            publish_time: s.publish_time,
        }
    }
}

impl PartialEq for SpecificPrice {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl PartialOrd for SpecificPrice {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SpecificPrice {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let expo_diff = self.exponent.abs_diff(other.exponent);
        let scale = 10i128.saturating_pow(expo_diff);
        let (lhs, rhs) = if self.exponent >= other.exponent {
            (
                i128::from(self.value).saturating_mul(scale),
                i128::from(other.value),
            )
        } else {
            (
                i128::from(self.value),
                i128::from(other.value).saturating_mul(scale),
            )
        };
        lhs.cmp(&rhs)
    }
}
