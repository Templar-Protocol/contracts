use std::marker::PhantomData;

use near_sdk::near;

use crate::{
    oracle::{
        proxy::aggregator::{
            filter::Filter,
            source::{Source, WeightedSource},
            specific_price::SpecificPrice,
        },
        pyth,
    },
    panic_with_message,
    time::Nanoseconds,
};

use super::AggregationMethod;

/// Calculates the weighted median of a sorted list of weighted items.
///
/// If all of the weights are zero, returns the first item.
///
/// Only definitely correct for lists where `sum(weights)` does not overflow `u32`.
fn median<T>(sorted_weighted_items: &[(T, u32)]) -> (usize, usize) {
    if sorted_weighted_items.len() == 1 {
        return (0, 0);
    }

    let mut lo = 0;
    let mut hi = sorted_weighted_items.len() - 1;
    let mut acc: u32 = 0;

    while lo < hi {
        acc = acc.saturating_add(sorted_weighted_items[lo].1);
        lo += 1;

        while acc >= sorted_weighted_items[hi].1 && hi != 0 {
            acc = acc.saturating_sub(sorted_weighted_items[hi].1);
            hi -= 1;
        }
    }

    (lo, hi)
}

pub trait MedianVariant {
    fn median<T>(sorted_weighted_items: &[(T, u32)]) -> usize;
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Low;

impl MedianVariant for Low {
    fn median<T>(sorted_weighted_items: &[(T, u32)]) -> usize {
        let (lo, hi) = median(sorted_weighted_items);
        lo.min(hi)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct High;

impl MedianVariant for High {
    fn median<T>(sorted_weighted_items: &[(T, u32)]) -> usize {
        let (lo, hi) = median(sorted_weighted_items);
        lo.max(hi)
    }
}

pub type MedianLow = Median<Low>;
pub type MedianHigh = Median<High>;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Median<V: MedianVariant> {
    _variant: PhantomData<V>,
    pub sources: Vec<WeightedSource>,
    pub filter: Filter,
}

impl<V: MedianVariant> Median<V> {
    pub fn new(sources: impl IntoIterator<Item = WeightedSource>, filter: Filter) -> Self {
        Self {
            _variant: PhantomData,
            sources: sources.into_iter().collect(),
            filter,
        }
    }
}

impl<V: MedianVariant> AggregationMethod for Median<V> {
    fn sources(&self) -> Vec<&Source> {
        self.sources.iter().map(|e| &e.source).collect()
    }

    fn aggregate(
        &self,
        prices: &[Option<pyth::Price>],
        now: Nanoseconds,
    ) -> Result<pyth::Price, super::Error> {
        if prices.len() != self.sources.len() {
            panic_with_message("Invariant violation: length mismatch");
        }

        let prices_filtered = self
            .sources
            .iter()
            .zip(prices)
            .filter_map(|(entry, price)| {
                if let Some(price) = price {
                    self.filter
                        .price
                        .apply(price, now)
                        .then_some((price, entry.weight))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        let min_sources = self.filter.min_sources.unwrap_or(1).max(1);

        if prices_filtered.len() < min_sources as usize {
            return Err(super::Error::TooFewValidSources {
                expected: min_sources as usize,
                actual: prices_filtered.len(),
            });
        }

        let mut values = prices_filtered
            .into_iter()
            .flat_map(|(price, weight)| {
                // Split apart prices so that we don't need to worry about confidence when sorting.
                let (lower, upper) = SpecificPrice::split(price);
                [(lower, weight), (upper, weight)]
            })
            .collect::<Vec<_>>();

        if values.is_empty() {
            panic_with_message("Invariant violation: must not be empty after splitting");
        }

        values.sort_unstable();

        Ok(values.swap_remove(V::median(&values)).0.into())
    }
}
