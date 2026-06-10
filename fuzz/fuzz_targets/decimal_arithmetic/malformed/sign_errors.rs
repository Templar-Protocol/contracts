#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

fuzz_target!(|data: &[u8]| {
    // Test sign errors (negative numbers) don't panic
    if !data.is_empty() {
        let fuzz_byte = data[0];

        let sign_error_patterns = [
            format!("-{fuzz_byte}"),
            format!("-{}.{}", fuzz_byte % 10, fuzz_byte % 100),
            format!("+-{fuzz_byte}"),
            "-".to_string(),
        ];

        for malformed in sign_error_patterns {
            let _ = Decimal::from_str(&malformed);
        }
    }
});
