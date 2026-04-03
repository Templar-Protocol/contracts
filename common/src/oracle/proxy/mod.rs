pub mod aggregator;
pub mod governance;

use near_sdk::near;

use aggregator::{filter::Filter, method::Aggregate, Aggregator};

use crate::time::Nanoseconds;

use super::pyth;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Proxy {
    pub aggregator: Aggregator,
    pub filter: Filter,
}

impl Proxy {
    pub fn new(aggregator: Aggregator, filter: Filter) -> Self {
        Self { aggregator, filter }
    }

    pub fn filter_and_aggregate(
        &self,
        prices: Vec<Option<pyth::Price>>,
        now: Nanoseconds,
    ) -> Result<pyth::Price, aggregator::method::Error> {
        let prices = prices
            .into_iter()
            .map(|price| {
                if price.as_ref().is_some_and(|p| self.filter.accepts(p, now)) {
                    price
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        self.aggregator.aggregate(prices)
    }
}
