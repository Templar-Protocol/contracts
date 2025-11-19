#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::number::Decimal;

fuzz_target!(|data: &[u8]| {
    // Try to parse as UTF-8 string
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(decimal) = Decimal::from_str(s) {
            // Test various precision levels using fuzzed values
            if !data.is_empty() {
                let fuzzed_precision = (data[0] % 39) as usize; // 0-38 range
                let fixed = decimal.to_fixed(fuzzed_precision);
                let _ = Decimal::from_str(&fixed);
            }

            // Test additional precision levels if we have more data
            if data.len() >= 3 {
                data.iter().take(3).for_each(|&byte| {
                    let precision = (byte % 39) as usize; // 0-38 range
                    let fixed = decimal.to_fixed(precision);
                    let _ = Decimal::from_str(&fixed);
                });
            }
        }
    }
});