#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::number::Decimal;

fuzz_target!(|data: &[u8]| {
    // Test multiple decimal points don't panic
    if !data.is_empty() {
        let fuzz_byte = data[0];

        let multiple_decimal_patterns = [
            format!("{}.{}.{}", fuzz_byte % 10, fuzz_byte % 100, fuzz_byte % 200),
            format!("1.2.{fuzz_byte}"),
            format!("{fuzz_byte}.0.0"),
        ];

        for malformed in multiple_decimal_patterns {
            let _ = Decimal::from_str(&malformed);
        }
    }
});

