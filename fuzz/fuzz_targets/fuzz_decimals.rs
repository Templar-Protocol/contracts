#![no_main]

use libfuzzer_sys::fuzz_target;
use templar_common::interest_rate_strategy::UsageCurve;
use templar_common::interest_rate_strategy::{Exponential2, Piecewise};
use templar_common::number::Decimal;

// Helper to convert u64 to a Decimal in [0, 1]
fn to_decimal01(x: u64) -> Decimal {
    Decimal::from(x) / Decimal::from(u64::MAX)
}

fuzz_target!(|data: (u64, u64, u64, u64, u64, u64, u64)| {
    // Fuzz Piecewise
    let base = to_decimal01(data.0);
    let optimal = to_decimal01(data.1); // must be <= 1
    let rate_1 = to_decimal01(data.2);
    let rate_2 = to_decimal01(data.3);
    let usage = to_decimal01(data.4);

    if let Some(piecewise) = Piecewise::new(base, optimal, rate_1, rate_2) {
        // Should not panic for usage <= 1
        let _ = piecewise.at(usage.min(Decimal::ONE));
    }

    // Fuzz Exponential2
    let base = to_decimal01(data.0);
    let top = to_decimal01(data.5).max(base); // top >= base
    let eccentricity = to_decimal01(data.6) * Decimal::from(24u32); // [0,24]
    let usage = to_decimal01(data.4);

    if let Some(exp2) = Exponential2::new(base, top, eccentricity) {
        let _ = exp2.at(usage.min(Decimal::ONE));
    }
});
