#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

fuzz_target!(|data: &[u8]| {
    // Test invalid decimal separators don't panic
    if !data.is_empty() {
        let fuzz_byte = data[0];

        let invalid_separator_patterns = [
            format!(".{fuzz_byte}"),
            format!("{fuzz_byte}.."),
            format!("..{fuzz_byte}"),
            ".".to_string(),
        ];

        for malformed in invalid_separator_patterns {
            let _ = Decimal::from_str(&malformed);
        }
    }
});
