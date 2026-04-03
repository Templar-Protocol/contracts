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

#[cfg(test)]
mod tests {
    use near_sdk::json_types::{I64, U64};

    use crate::{
        oracle::{pyth::PythTimestamp, OracleRequest},
        time::Nanoseconds,
    };

    use super::*;

    fn price(value: i64, conf: u64, publish_time: PythTimestamp) -> pyth::Price {
        pyth::Price {
            price: I64(value),
            conf: U64(conf),
            expo: -6,
            publish_time,
        }
    }

    fn secs(s: i64) -> PythTimestamp {
        PythTimestamp::from_secs(s)
    }

    fn priority(filter: Filter, count: usize) -> Priority {
        Priority {
            sources: (0..count)
                .map(|_| OracleRequest::redstone("oracle.near".parse().unwrap(), "BTC").into())
                .collect(),
            filter,
        }
    }

    #[test]
    fn priority_empty_returns_too_few_valid_sources() {
        let error = Priority {
            sources: vec![],
            filter: Filter::default(),
        }
        .aggregate(&[], Nanoseconds::zero())
        .unwrap_err();
        assert!(matches!(
            error,
            super::super::Error::TooFewValidSources {
                expected: 1,
                actual: 0,
            }
        ));
    }

    #[test]
    fn priority_single_price() {
        let result = priority(Filter::default(), 1)
            .aggregate(&[Some(price(1_000_000, 0, secs(0)))], Nanoseconds::zero())
            .unwrap();
        assert_eq!(result.price.0, 1_000_000);
    }

    #[test]
    fn priority_selects_first_valid_price() {
        let prices = [
            None,
            Some(price(2_000_000, 0, secs(0))),
            Some(price(3_000_000, 0, secs(0))),
        ];
        let result = priority(Filter::default(), prices.len())
            .aggregate(&prices, Nanoseconds::zero())
            .unwrap();
        assert_eq!(result.price.0, 2_000_000);
    }

    #[test]
    fn priority_preserves_original_price_with_confidence() {
        let result = priority(Filter::default(), 2)
            .aggregate(
                &[
                    Some(price(1_000, 100, secs(0))),
                    Some(price(2_000, 0, secs(0))),
                ],
                Nanoseconds::zero(),
            )
            .unwrap();
        assert_eq!(result.price.0, 1_000);
        assert_eq!(result.conf.0, 100);
    }

    #[test]
    fn priority_respects_max_age_filter() {
        let filter = Filter {
            price: crate::oracle::proxy::aggregator::filter::IndividualPriceFilter {
                max_age: Some(Nanoseconds::from_secs(500)),
                max_clock_drift: None,
            },
            min_sources: None,
        };
        let prices = [
            Some(price(1_000_000, 0, secs(0))),
            Some(price(2_000_000, 0, secs(900))),
        ];
        let result = priority(filter, prices.len())
            .aggregate(&prices, Nanoseconds::from_secs(1000))
            .unwrap();
        assert_eq!(result.price.0, 2_000_000);
    }

    #[test]
    fn priority_ignores_min_sources_and_returns_first_valid_price() {
        let filter = Filter {
            min_sources: Some(3),
            ..Default::default()
        };
        let prices = [
            Some(price(1_000_000, 0, secs(0))),
            Some(price(2_000_000, 0, secs(0))),
        ];
        let result = priority(filter, prices.len())
            .aggregate(&prices, Nanoseconds::zero())
            .unwrap();
        assert_eq!(result.price.0, 1_000_000);
    }
}
