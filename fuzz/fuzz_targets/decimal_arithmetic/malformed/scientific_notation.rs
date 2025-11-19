#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::number::Decimal;

fuzz_target!(|data: &[u8]| {
    // Test scientific notation (not supported) doesn't panic
    if !data.is_empty() {
        let fuzz_byte = data[0];

        let scientific_notation_patterns = [
            format!("{}e{}", fuzz_byte % 10, fuzz_byte % 10),
            format!("{}E{}", fuzz_byte, fuzz_byte % 100),
            format!("1e{fuzz_byte}"),
            format!("{}e+{}", fuzz_byte % 10, fuzz_byte % 10),
        ];

        for malformed in scientific_notation_patterns {
            let _ = Decimal::from_str(&malformed);
        }
    }
});

