use near_sdk::near;

use crate::{oracle::pyth, time::Nanoseconds};

/// Filter configuration for the aggregation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[near(serializers = [json, borsh])]
pub struct Filter {
    /// Filter for each individual price.
    pub price: IndividualPriceFilter,
    /// Minimum number of sources required for the aggregation to produce a result.
    ///
    /// For example, if the proxy has a Pyth source and a RedStone source, and `min_sources` is set to `Some(2)`,
    /// the aggregation will only produce a result if both oracles provide a price.
    pub min_sources: Option<u32>,
}

impl Filter {
    pub fn new(max_age: Option<Nanoseconds>, max_clock_drift: Option<Nanoseconds>) -> Self {
        Self {
            price: IndividualPriceFilter {
                max_age,
                max_clock_drift,
            },
            min_sources: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[near(serializers = [json, borsh])]
pub struct IndividualPriceFilter {
    /// Maximum age of a price in nanoseconds. If a price is older than this, it will be excluded from the aggregation.
    pub max_age: Option<Nanoseconds>,
    /// Maximum clock drift in nanoseconds. This is the future-analog of `max_age`.
    pub max_clock_drift: Option<Nanoseconds>,
}

impl IndividualPriceFilter {
    pub fn apply(&self, p: &pyth::Price, now: Nanoseconds) -> bool {
        let Some(published) = Nanoseconds::try_from_pyth(p.publish_time) else {
            return false;
        };

        if now >= published {
            self.max_age
                .is_none_or(|max| now.saturating_sub(published) <= max)
        } else {
            self.max_clock_drift
                .is_none_or(|max| published.saturating_sub(now) <= max)
        }
    }
}
