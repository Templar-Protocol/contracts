use near_sdk::near;

use crate::{
    oracle::{
        proxy::aggregator::{filter::Filter, source::Source},
        pyth,
    },
    panic_with_message,
    time::Nanoseconds,
};

use super::AggregationMethod;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Priority {
    pub sources: Vec<Source>,
    pub filter: Filter,
}

impl AggregationMethod for Priority {
    fn sources(&self) -> Vec<&Source> {
        self.sources.iter().collect()
    }

    fn aggregate(
        &self,
        prices: &[Option<pyth::Price>],
        now: Nanoseconds,
    ) -> Result<pyth::Price, super::Error> {
        if prices.len() != self.sources.len() {
            panic_with_message("Invariant violation: length mismatch");
        }

        for price in prices.iter().filter_map(|p| p.as_ref()) {
            if self.filter.price.apply(price, now) {
                return Ok(price.clone());
            }
        }

        Err(super::Error::TooFewValidSources {
            expected: 1,
            actual: 0,
        })
    }
}
