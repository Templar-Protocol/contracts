#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::number::Decimal;

fuzz_target!(|data: &[u8]| {
    // Test special values don't panic
    if !data.is_empty() {
        let fuzz_byte = data[0];
        let special_idx = fuzz_byte % 4;
        let specials = ["NaN", "Infinity", "inf", "null"];
        
        let special_value_patterns = [
            specials[special_idx as usize].to_string(),
            format!("{}{}", specials[special_idx as usize], fuzz_byte),
        ];

        for malformed in special_value_patterns {
            let _ = Decimal::from_str(&malformed);
        }
    }
});