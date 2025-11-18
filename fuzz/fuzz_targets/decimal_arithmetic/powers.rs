#![no_main]

use libfuzzer_sys::fuzz_target;
use templar_common::number::Decimal;

fuzz_target!(|data: (u128, u32)| {
    let (a, pow_exp) = data;

    let dec_a = Decimal::from(a);

    // Power operations with safe exponents
    #[allow(clippy::cast_possible_wrap, reason = "Fuzzing context")]
    let small_pow = (pow_exp % 20) as i32;
    let _ = dec_a.pow(small_pow);
    let _ = dec_a.pow(-small_pow);
    let _ = dec_a.pow(0);
    let _ = dec_a.pow(1);

    // pow2_int with valid range
    let pow2_exp = pow_exp % 384;
    let _ = Decimal::pow2_int(pow2_exp);

    // pow2 (only for values <= 1)
    if dec_a <= Decimal::ONE {
        let _ = dec_a.pow2();
    }

    // Test specific constant powers
    let _ = Decimal::TWO.pow(10);
    let _ = Decimal::ONE_HALF.pow(5);
});

