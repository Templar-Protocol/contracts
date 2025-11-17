#![no_main]

use libfuzzer_sys::fuzz_target;
use templar_common::number::Decimal;

fuzz_target!(|data: (u128, u128, u128)| {
    let (a, b, c) = data;
    
    let dec_a = Decimal::from(a);
    let dec_b = Decimal::from(b);
    let dec_c = Decimal::from(c);

    // Addition operations
    let _ = dec_a + dec_b;
    let _ = dec_a + dec_b + dec_c;
    let mut mut_a = dec_a;
    mut_a += dec_b;

    // Subtraction (only if a >= b to avoid underflow)
    if a >= b {
        let _ = dec_a - dec_b;
        let mut mut_sub = dec_a;
        mut_sub -= dec_b;
    }

    // Multiplication operations  
    let _ = dec_a * dec_b;
    let _ = dec_a * dec_b * dec_c;
    let mut mut_mul = dec_a;
    mut_mul *= dec_b;

    // Division (avoid division by zero)
    if b > 0 {
        let _ = dec_a / dec_b;
        let mut mut_div = dec_a;
        mut_div /= dec_b;
    }

    // Mixed operations with integers
    let _ = dec_a * 2u32;
    let _ = 3u64 * dec_b;
    let _ = dec_a / 10u128;
    if b > 0 {
        let _ = 100u128 / dec_b;
    }

    // Chained operations
    if b > 0 && c > 0 {
        let _ = (dec_a + dec_b) * dec_c / Decimal::TWO;
    }
});