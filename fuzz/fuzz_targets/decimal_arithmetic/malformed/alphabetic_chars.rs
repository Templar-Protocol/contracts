#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

fuzz_target!(|data: &[u8]| {
    // Test alphabetic characters don't panic
    if !data.is_empty() {
        let fuzz_byte = data[0];
        let alpha_char = (b'a' + (fuzz_byte % 26)) as char;

        let alphabetic_patterns = [
            format!("{fuzz_byte}{alpha_char}"),
            format!("{alpha_char}{fuzz_byte}c"),
            format!("ab{fuzz_byte}"),
            alpha_char.to_string(),
        ];

        for malformed in alphabetic_patterns {
            let _ = Decimal::from_str(&malformed);
        }
    }
});
