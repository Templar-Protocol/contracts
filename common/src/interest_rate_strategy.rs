use near_sdk::{near, require};

use crate::number::Decimal;

pub trait UsageCurve {
    fn at(&self, utilization_ratio: Decimal) -> Decimal;
}

pub enum InterestRateStrategy {
    Piecewise(Piecewise),
    Exponential2(Exponential2),
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
    i_rate_2_b: Decimal,
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
            i_rate_2_b: &optimal * (&rate_1 - &rate_2) + &base,
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
            &self.rate_1 * utilization_ratio + &self.base
        } else {
            &self.rate_2 * utilization_ratio + &self.i_rate_2_b
        }
    }
}

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
            i_factor: (&top - &base) / (eccentricity.pow2().unwrap() - 1u32),
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

        &self.base
            + &self.i_factor * ((&self.eccentricity * &utilization_ratio).pow2().unwrap() - 1u32)
    }
}

#[cfg(test)]
mod tests {
    use crate::dec;

    use super::*;

    #[test]
    fn piecewise() {
        let s = Piecewise::new(Decimal::ZERO, dec!("0.9"), dec!("0.035"), dec!("0.6"));
    }
}
