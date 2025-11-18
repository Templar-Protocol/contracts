#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::number::Decimal;

fuzz_target!(|data: &[u8]| {
    // Try to parse as UTF-8 string
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(decimal) = Decimal::from_str(s) {
            // Test round-trip parsing with maximum precision
            let to_string = decimal.to_fixed(38);

            // Parse again and check near equality
            if let Ok(reparsed) = Decimal::from_str(&to_string) {
                assert!(
                    decimal.near_equal(reparsed),
                    "Round-trip failed: original={decimal:?}, string={to_string}, reparsed={reparsed:?}",
                );
            }
        }
    }
});