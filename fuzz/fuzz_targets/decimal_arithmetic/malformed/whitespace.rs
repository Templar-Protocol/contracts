#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::number::Decimal;

fuzz_target!(|data: &[u8]| {
    // Test empty and whitespace strings don't panic
    if !data.is_empty() {
        let fuzz_byte = data[0];

        let whitespace_patterns = [
            String::new(),
            " ".to_string(),
            format!(" {fuzz_byte} "),
            format!("\t{fuzz_byte}"),
        ];

        for malformed in whitespace_patterns {
            let _ = Decimal::from_str(&malformed);
        }
    }
});

