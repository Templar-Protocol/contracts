use alloc::vec::Vec;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::{format, string::ToString};

use crate::Price;

use super::Aggregate;

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Priority<S> {
        pub sources: Vec<S>,
    }
}

impl<S> Priority<S> {
    pub fn new(sources: impl IntoIterator<Item = S>) -> Self {
        Self {
            sources: sources.into_iter().collect(),
        }
    }
}

impl<S> Aggregate<S> for Priority<S> {
    fn aggregate<I>(&self, prices: I) -> Result<Price, super::Error>
    where
        I: IntoIterator<Item = Option<Price>>,
        I::IntoIter: ExactSizeIterator<Item = Option<Price>>,
    {
        let prices = prices.into_iter();

        if prices.len() != self.sources.len() {
            return Err(super::Error::LengthMismatch {
                expected: self.sources.len(),
                actual: prices.len(),
            });
        }

        prices
            .flatten()
            .next()
            .ok_or(super::Error::TooFewValidSources {
                expected: 1,
                actual: 0,
            })
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::proxy::aggregator::method::Error;

    fn price(value: i64, conf: u64, publish_time_s: u64) -> Price {
        Price {
            price: value,
            conf,
            expo: -6,
            publish_time_ns: templar_primitives::Nanoseconds::from_secs(publish_time_s),
        }
    }

    fn priority(count: usize) -> Priority<&'static str> {
        Priority {
            sources: (0..count).map(|_| "source").collect(),
        }
    }

    #[test]
    fn priority_empty_returns_too_few_valid_sources() {
        let error = Priority::<&'static str> { sources: vec![] }
            .aggregate(vec![])
            .unwrap_err();
        assert!(matches!(
            error,
            Error::TooFewValidSources {
                expected: 1,
                actual: 0,
            }
        ));
    }

    #[test]
    fn priority_all_none_returns_too_few_valid_sources() {
        let error = Priority::<&'static str> {
            sources: vec!["s1", "s2"],
        }
        .aggregate(vec![None, None])
        .unwrap_err();

        assert!(matches!(
            error,
            Error::TooFewValidSources {
                expected: 1,
                actual: 0,
            }
        ));
    }

    #[test]
    fn priority_returns_length_mismatch_when_prices_len_differs_from_sources() {
        let error = priority(2)
            .aggregate(vec![Some(price(1_000_000, 0, 0))])
            .unwrap_err();

        assert!(matches!(
            error,
            Error::LengthMismatch {
                expected: 2,
                actual: 1,
            }
        ));
    }

    #[test]
    fn priority_single_price() {
        let result = priority(1)
            .aggregate(vec![Some(price(1_000_000, 0, 0))])
            .unwrap();
        assert_eq!(result.price, 1_000_000);
    }

    #[test]
    fn priority_selects_first_valid_price() {
        let prices = vec![
            None,
            Some(price(2_000_000, 0, 0)),
            Some(price(3_000_000, 0, 0)),
        ];
        let result = priority(prices.len()).aggregate(prices).unwrap();
        assert_eq!(result.price, 2_000_000);
    }

    #[test]
    fn priority_preserves_original_price_with_confidence() {
        let result = priority(2)
            .aggregate(vec![Some(price(1_000, 100, 0)), Some(price(2_000, 0, 0))])
            .unwrap();
        assert_eq!(result.price, 1_000);
        assert_eq!(result.conf, 100);
    }

    #[test]
    fn priority_returns_first_valid_price_even_with_multiple_prices() {
        let prices = vec![Some(price(1_000_000, 0, 0)), Some(price(2_000_000, 0, 0))];
        let result = priority(prices.len()).aggregate(prices).unwrap();
        assert_eq!(result.price, 1_000_000);
    }
}
