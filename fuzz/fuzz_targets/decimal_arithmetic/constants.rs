#![no_main]

use libfuzzer_sys::fuzz_target;
use templar_common::Decimal;

fuzz_target!(|data: u128| {
    let a = data;

    let dec_a = Decimal::from(a);

    // Operations with constants
    let _ = dec_a + Decimal::ZERO;
    let _ = dec_a + Decimal::ONE;
    let _ = dec_a + Decimal::TWO;
    let _ = dec_a * Decimal::ZERO;
    let _ = dec_a * Decimal::ONE;
    let _ = dec_a * Decimal::TWO;

    // Mathematical constants operations
    let _ = dec_a + Decimal::E;
    let _ = dec_a + Decimal::LN2;
    let _ = dec_a * Decimal::E;
    let _ = dec_a * Decimal::LN2;

    // Extreme values operations
    let _ = dec_a + Decimal::MAX;
    let _ = dec_a + Decimal::MIN;

    // Test constant properties
    let _ = Decimal::ZERO.is_zero();
    let _ = Decimal::ONE > Decimal::ZERO;
    let _ = Decimal::TWO > Decimal::ONE;
});
