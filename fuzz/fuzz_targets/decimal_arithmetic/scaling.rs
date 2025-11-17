#![no_main]

use libfuzzer_sys::fuzz_target;
use templar_common::number::Decimal;

fuzz_target!(|data: (u128, i32)| {
    let (a, exp1) = data;
    
    let dec_a = Decimal::from(a);

    // mul_pow10 with safe exponents
    let safe_exp1 = exp1.clamp(-38, 115);
    let _ = dec_a.mul_pow10(safe_exp1);
    let _ = dec_a.mul_pow10(-safe_exp1);

    // Identity and basic scaling tests
    let _ = Decimal::ONE.mul_pow10(0);
    let _ = dec_a.mul_pow10(1);
    let _ = dec_a.mul_pow10(-1);
});