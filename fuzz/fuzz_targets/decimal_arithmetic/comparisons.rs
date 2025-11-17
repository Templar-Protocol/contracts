#![no_main]

use libfuzzer_sys::fuzz_target;
use templar_common::number::Decimal;

fuzz_target!(|data: (u128, u128)| {
    let (a, b) = data;
    
    let dec_a = Decimal::from(a);
    let dec_b = Decimal::from(b);

    // All comparison operations
    let _ = dec_a == dec_b;
    let _ = dec_a < dec_b;
    let _ = dec_a > dec_b;
    let _ = dec_a <= dec_b;
    let _ = dec_a >= dec_b;
    let _ = dec_a.near_equal(dec_b);

    // Test near_equal with close values
    let close_a = dec_a + Decimal::from(1u32);
    let _ = dec_a.near_equal(close_a);
});