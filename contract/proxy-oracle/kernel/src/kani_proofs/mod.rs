mod circuit_breaker;
mod freshness;
mod median;
mod priority;

use templar_primitives::Nanoseconds;

use crate::{proxy::FreshnessFilter, Price};

const MAX_SOURCES: usize = 3;

#[derive(Clone, Copy)]
struct SymbolicPriceInput {
    present: bool,
    mantissa: i64,
    confidence: u64,
    exponent: i32,
    publish_time_ns: u64,
}

impl SymbolicPriceInput {
    fn price(self) -> Price {
        Price {
            price: self.mantissa,
            conf: self.confidence,
            expo: self.exponent,
            publish_time_ns: Nanoseconds::from_ns(self.publish_time_ns),
        }
    }

    fn has_valid_confidence(self) -> bool {
        self.mantissa > 0 && self.mantissa as u64 > self.confidence
    }
}

#[derive(Clone, Copy)]
struct FreshnessInputs {
    now: u64,
    max_age: u64,
    max_clock_drift: u64,
    age_enabled: bool,
    clock_drift_enabled: bool,
}

impl FreshnessInputs {
    fn filter(self) -> FreshnessFilter {
        FreshnessFilter::new(
            optional_nanoseconds(self.age_enabled, self.max_age),
            optional_nanoseconds(self.clock_drift_enabled, self.max_clock_drift),
        )
    }

    fn accepts(self, publish_time_ns: u64) -> bool {
        reference_freshness_accepts(
            publish_time_ns,
            self.now,
            self.age_enabled.then_some(self.max_age),
            self.clock_drift_enabled.then_some(self.max_clock_drift),
        )
    }
}

fn bounded_i8(minimum: i8, maximum: i8) -> i8 {
    let value = kani::any::<i8>();
    kani::assume(value >= minimum);
    kani::assume(value <= maximum);
    value
}

fn bounded_u8(maximum: u8) -> u8 {
    let value = kani::any::<u8>();
    kani::assume(value <= maximum);
    value
}

fn symbolic_price_input() -> SymbolicPriceInput {
    SymbolicPriceInput {
        present: kani::any(),
        mantissa: i64::from(bounded_i8(-1, 32)),
        confidence: u64::from(bounded_u8(33)),
        exponent: i32::from(bounded_i8(-1, 1)),
        publish_time_ns: u64::from(bounded_u8(32)),
    }
}

fn symbolic_price_inputs() -> [SymbolicPriceInput; MAX_SOURCES] {
    [
        symbolic_price_input(),
        symbolic_price_input(),
        symbolic_price_input(),
    ]
}

fn symbolic_freshness_inputs() -> FreshnessInputs {
    FreshnessInputs {
        now: u64::from(bounded_u8(32)),
        max_age: u64::from(bounded_u8(32)),
        max_clock_drift: u64::from(bounded_u8(32)),
        age_enabled: kani::any(),
        clock_drift_enabled: kani::any(),
    }
}

fn optional_nanoseconds(enabled: bool, value: u64) -> Option<Nanoseconds> {
    if enabled {
        Some(Nanoseconds::from_ns(value))
    } else {
        None
    }
}

fn reference_freshness_accepts(
    publish_time_ns: u64,
    now: u64,
    max_age: Option<u64>,
    max_clock_drift: Option<u64>,
) -> bool {
    if now >= publish_time_ns {
        match max_age {
            Some(maximum) => now - publish_time_ns <= maximum,
            None => true,
        }
    } else {
        match max_clock_drift {
            Some(maximum) => publish_time_ns - now <= maximum,
            None => true,
        }
    }
}

fn normalized_at_exponent_minus_one(value: i64, exponent: i32) -> i64 {
    match exponent {
        -1 => value,
        0 => value.wrapping_mul(10),
        1 => value.wrapping_mul(100),
        _ => 0,
    }
}
