use std::ops::Deref;

use near_sdk::{near, require};

use crate::number::Decimal;

pub trait UsageCurve {
    fn at(&self, utilization_ratio: Decimal) -> Decimal;
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub enum InterestRateStrategy {
    Linear(Linear),
    Piecewise(Piecewise),
    Exponential2(Exponential2),
}

impl InterestRateStrategy {
    #[must_use]
    pub fn linear(base: Decimal, top: Decimal) -> Option<Self> {
        Some(Self::Linear(Linear::new(base, top)?))
    }

    #[must_use]
    pub fn piecewise(
        base: Decimal,
        optimal: Decimal,
        rate_1: Decimal,
        rate_2: Decimal,
    ) -> Option<Self> {
        Some(Self::Piecewise(Piecewise::new(
            base, optimal, rate_1, rate_2,
        )?))
    }

    #[must_use]
    pub fn exponential2(base: Decimal, top: Decimal, eccentricity: Decimal) -> Option<Self> {
        Some(Self::Exponential2(Exponential2::new(
            base,
            top,
            eccentricity,
        )?))
    }
}

impl Deref for InterestRateStrategy {
    type Target = dyn UsageCurve;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Linear(linear) => linear as &dyn UsageCurve,
            Self::Piecewise(piecewise) => piecewise as &dyn UsageCurve,
            Self::Exponential2(exponential2) => exponential2 as &dyn UsageCurve,
        }
    }
}

/// ```text,no_run
/// r(u) = u * (t - b) + b
/// ```
#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub struct Linear {
    base: Decimal,
    top: Decimal,
}

impl Linear {
    pub fn new(base: Decimal, top: Decimal) -> Option<Self> {
        if base > top {
            None
        } else {
            Some(Self { base, top })
        }
    }
}

impl UsageCurve for Linear {
    fn at(&self, utilization_ratio: Decimal) -> Decimal {
        utilization_ratio * (self.top - self.base) + self.base
    }
}

/// ```text,no_run
/// r(u) = {
///     if u < o : r_1 * u + b,
///     else     : r_2 * u + o * (r_1 - r_2) + b
/// }
/// ```
#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub struct Piecewise {
    base: Decimal,
    optimal: Decimal,
    rate_1: Decimal,
    rate_2: Decimal,
    i_negative_rate_2_b: Decimal,
}

impl Piecewise {
    pub fn new(base: Decimal, optimal: Decimal, rate_1: Decimal, rate_2: Decimal) -> Option<Self> {
        if optimal > 1u32 {
            return None;
        }

        if rate_1 > rate_2 {
            return None;
        }

        Some(Self {
            i_negative_rate_2_b: optimal * (rate_2 - rate_1) - base,
            base,
            optimal,
            rate_1,
            rate_2,
        })
    }
}

impl UsageCurve for Piecewise {
    fn at(&self, utilization_ratio: Decimal) -> Decimal {
        require!(utilization_ratio <= Decimal::ONE);

        if utilization_ratio < self.optimal {
            self.rate_1 * utilization_ratio + self.base
        } else {
            self.rate_2 * utilization_ratio - self.i_negative_rate_2_b
        }
    }
}

/// ```text,no_run
/// r(u) = b + (t - b) * (2^ku - 1) / (2^k - 1)
/// ```
#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub struct Exponential2 {
    base: Decimal,
    top: Decimal,
    eccentricity: Decimal,
    i_factor: Decimal,
}

impl Exponential2 {
    #[allow(clippy::missing_panics_doc)]
    pub fn new(base: Decimal, top: Decimal, eccentricity: Decimal) -> Option<Self> {
        if base > top {
            return None;
        }

        if eccentricity > 24u32 {
            return None;
        }

        #[allow(clippy::unwrap_used)]
        Some(Self {
            i_factor: (top - base) / (eccentricity.pow2().unwrap() - 1u32),
            base,
            top,
            eccentricity,
        })
    }
}

impl UsageCurve for Exponential2 {
    #[allow(clippy::unwrap_used)]
    fn at(&self, utilization_ratio: Decimal) -> Decimal {
        require!(utilization_ratio <= Decimal::ONE);

        self.base + self.i_factor * ((self.eccentricity * utilization_ratio).pow2().unwrap() - 1u32)
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Div;

    use crate::dec;

    use super::*;

    #[test]
    fn piecewise() {
        let s = Piecewise::new(Decimal::ZERO, dec!("0.9"), dec!("0.035"), dec!("0.6")).unwrap();

        assert!(s.at(Decimal::ZERO).near_equal(Decimal::ZERO));
        assert!(s.at(dec!("0.1")).near_equal(dec!("0.0035")));
        assert!(s.at(dec!("0.5")).near_equal(dec!("0.0175")));
        assert!(s.at(dec!("0.6")).near_equal(dec!("0.021")));
        assert!(s.at(dec!("0.9")).near_equal(dec!("0.0315")));
        assert!(s.at(dec!("0.95")).near_equal(dec!("0.0615")));
        assert!(s.at(Decimal::ONE).near_equal(dec!("0.0915")));
    }

    #[test]
    fn exponential2() {
        let s = Exponential2::new(dec!("0.005"), dec!("0.08"), dec!("6")).unwrap();
        assert!(s.at(Decimal::ZERO).near_equal(dec!("0.005")));
        assert!(s.at(dec!("0.25")).near_equal(dec!(
            "0.00717669895803117868762306839097547161564207589375463826946828509045412494"
        )));
        assert!(s.at(Decimal::ONE_HALF).near_equal(Decimal::ONE.div(75u32)));
    }
}
