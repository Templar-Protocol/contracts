use crate::*;

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
    fn sources(&self) -> Vec<&S> {
        self.sources.iter().collect()
    }

    fn aggregate(&self, prices: Vec<Option<Price>>) -> Result<Price, super::Error> {
        if prices.len() != self.sources.len() {
            return Err(super::Error::LengthMismatch {
                expected: self.sources.len(),
                actual: prices.len(),
            });
        }

        prices
            .iter()
            .find_map(|p| p.clone())
            .ok_or(super::Error::TooFewValidSources {
                expected: 1,
                actual: 0,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            super::super::Error::TooFewValidSources {
                expected: 1,
                actual: 0,
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
            .aggregate(vec![
                Some(price(1_000, 100, 0)),
                Some(price(2_000, 0, 0)),
            ])
            .unwrap();
        assert_eq!(result.price, 1_000);
        assert_eq!(result.conf, 100);
    }

    #[test]
    fn priority_returns_first_valid_price_even_with_multiple_prices() {
        let prices = vec![
            Some(price(1_000_000, 0, 0)),
            Some(price(2_000_000, 0, 0)),
        ];
        let result = priority(prices.len()).aggregate(prices).unwrap();
        assert_eq!(result.price, 1_000_000);
    }
}
