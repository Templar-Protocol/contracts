#![no_main]

use libfuzzer_sys::fuzz_target;
use templar_common::Decimal;

fuzz_target!(|data: (u128, u128)| {
    let (a, b) = data;

    let dec_a = Decimal::from(a);
    let dec_b = Decimal::from(b);

    // Utility functions
    let _ = dec_a.abs_diff(dec_b);
    let _ = dec_b.abs_diff(dec_a);
    let _ = dec_a.is_zero();
    let _ = Decimal::ZERO.is_zero();
    let _ = dec_a.fractional_part_as_u128_dividend();
});
