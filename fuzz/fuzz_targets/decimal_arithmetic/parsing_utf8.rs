#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::number::Decimal;

fuzz_target!(|data: &[u8]| {
    // Try to parse as UTF-8 string and test basic parsing
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(decimal) = Decimal::from_str(s) {
            // Test that is_zero is consistent
            if decimal.is_zero() {
                assert_eq!(decimal, Decimal::ZERO);
            }

            // Test basic operations on parsed value
            let _ = decimal + Decimal::ONE;
            let _ = decimal * Decimal::TWO;
            if !decimal.is_zero() {
                let _ = Decimal::ONE / decimal;
            }
        }
    }
});