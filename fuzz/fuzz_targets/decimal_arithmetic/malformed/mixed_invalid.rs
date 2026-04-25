#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

fuzz_target!(|data: &[u8]| {
    // Test mixed invalid patterns don't panic
    if !data.is_empty() {
        let fuzz_byte = data[0];

        let mixed_invalid_patterns = [
            format!("{fuzz_byte}#{fuzz_byte}"),
            format!("@{}.{}", fuzz_byte % 10, fuzz_byte % 10),
            format!("{fuzz_byte}%"),
            format!("${fuzz_byte}"),
        ];

        for malformed in mixed_invalid_patterns {
            let _ = Decimal::from_str(&malformed);
        }
    }
});
