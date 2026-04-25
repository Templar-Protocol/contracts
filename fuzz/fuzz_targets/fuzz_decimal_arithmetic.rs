#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

fuzz_target!(|data: (u128, u128, u128, i32, i32, u32, u8)| {
    let (a, b, c, exp1, _exp2, pow_exp, op_selector) = data;

    // Create decimals from various u128 values
    let dec_a = Decimal::from(a);
    let dec_b = Decimal::from(b);
    let dec_c = Decimal::from(c);

    // Fuzz basic arithmetic operations
    // Addition
    let _ = dec_a + dec_b;
    let _ = dec_a + dec_b + dec_c;
    let mut mut_a = dec_a;
    mut_a += dec_b;

    // Subtraction (only if a >= b to avoid underflow in unsigned context)
    if a >= b {
        let _ = dec_a - dec_b;
        let mut mut_sub = dec_a;
        mut_sub -= dec_b;
    }

    // Multiplication
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

    // Fuzz power operations
    #[allow(clippy::cast_possible_wrap, reason = "Fuzzing context")]
    let small_pow = (pow_exp % 20) as i32; // Keep small to avoid overflows
    let _ = dec_a.pow(small_pow);
    let _ = dec_a.pow(-small_pow);
    let _ = dec_a.pow(0);
    let _ = dec_a.pow(1);

    // Fuzz pow2_int
    let pow2_exp = pow_exp % 384; // Keep within valid range
    let _ = Decimal::pow2_int(pow2_exp);

    // Fuzz pow2
    if dec_a <= Decimal::ONE {
        let _ = dec_a.pow2();
    }

    // Fuzz mul_pow10
    let safe_exp1 = exp1.clamp(-38, 115); // Within valid range
    let _ = dec_a.mul_pow10(safe_exp1);
    let _ = dec_b.mul_pow10(-safe_exp1);

    // Fuzz comparison operations
    let _ = dec_a == dec_b;
    let _ = dec_a < dec_b;
    let _ = dec_a > dec_b;
    let _ = dec_a <= dec_b;
    let _ = dec_a >= dec_b;
    let _ = dec_a.near_equal(dec_b);

    // Fuzz conversions
    let _ = dec_a.to_u128_floor();
    let _ = dec_a.to_u128_ceil();
    let _ = dec_a.to_f64_lossy();

    // Fuzz string operations
    let _ = dec_a.to_fixed(38);
    let _ = dec_a.to_fixed(10);
    let _ = dec_a.to_fixed(0);

    // Fuzz abs_diff
    let _ = dec_a.abs_diff(dec_b);
    let _ = dec_b.abs_diff(dec_a);

    // Fuzz is_zero
    let _ = dec_a.is_zero();
    let _ = Decimal::ZERO.is_zero();

    // Fuzz fractional part
    let _ = dec_a.fractional_part_as_u128_dividend();

    // Test edge cases based on operation selector
    match op_selector % 10 {
        0 => {
            // Test with constants
            let _ = dec_a + Decimal::ONE;
            let _ = dec_a * Decimal::ZERO;
            let _ = dec_a + Decimal::TWO;
        }
        1 => {
            // Test with ONE_HALF
            let _ = dec_a * Decimal::ONE_HALF;
            if b > 0 {
                let _ = Decimal::ONE_HALF / dec_b;
            }
        }
        2 => {
            // Test with MAX and MIN
            let _ = Decimal::MAX.to_u128_floor();
            let _ = Decimal::MIN.is_zero();
        }
        3 => {
            // Test pow with specific values
            let _ = Decimal::TWO.pow(10);
            let _ = Decimal::ONE_HALF.pow(5);
        }
        4 => {
            // Test mul_pow10 edge cases
            let _ = Decimal::ONE.mul_pow10(0);
            let _ = dec_a.mul_pow10(1);
            let _ = dec_a.mul_pow10(-1);
        }
        5 => {
            // Test with E and LN2 constants
            let _ = Decimal::E + dec_a;
            let _ = Decimal::LN2 * dec_b;
        }
        6 => {
            // Chained operations
            if b > 0 && c > 0 {
                let _ = (dec_a + dec_b) * dec_c / Decimal::TWO;
            }
        }
        7 => {
            // Test near_equal with close values
            let close_a = dec_a + Decimal::from(1u32);
            let _ = dec_a.near_equal(close_a);
        }
        8 => {
            // Test ceiling and floor differences
            if let (Some(floor), Some(ceil)) = (dec_a.to_u128_floor(), dec_a.to_u128_ceil()) {
                let _ = ceil >= floor;
            }
        }
        _ => {
            // Test string round-trip
            let str_repr = dec_a.to_fixed(20);
            if let Ok(parsed) = Decimal::from_str(&str_repr) {
                let _ = dec_a.near_equal(parsed);
            }
        }
    }
});
