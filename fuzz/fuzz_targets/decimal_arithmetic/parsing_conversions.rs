#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::number::Decimal;

fuzz_target!(|data: &[u8]| {
    // Try to parse as UTF-8 string
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(decimal) = Decimal::from_str(s) {
            // Test conversions
            let _ = decimal.to_u128_floor();
            let _ = decimal.to_u128_ceil();
            let _ = decimal.to_f64_lossy();
        }
    }
});