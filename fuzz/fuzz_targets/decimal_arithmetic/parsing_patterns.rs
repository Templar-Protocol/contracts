#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

fuzz_target!(|data: &[u8]| {
    // Test with constructed string patterns using fuzzed inputs
    if data.len() >= 4 {
        // Generate whole part from first two bytes
        let whole_part = u128::from(data[0]) * 256 + u128::from(data[1]);

        // Generate fractional digits from remaining bytes
        let frac_digits = data[2] % 10;
        let extra_frac = if data.len() > 3 { data[3] % 100 } else { 0 };

        // Create various decimal string patterns
        let test_patterns = [
            format!("{whole_part}"),
            format!("{whole_part}.{frac_digits}"),
            format!("{whole_part}.{frac_digits:02}{extra_frac:02}"),
            format!("0.{frac_digits}"),
            format!("0.{frac_digits:09}"), // Leading zeros
        ];

        for pattern in test_patterns {
            if let Ok(decimal) = Decimal::from_str(&pattern) {
                let precision = if data.len() > 4 {
                    (data[4] % 39) as usize
                } else {
                    10
                };
                let _ = decimal.to_fixed(precision);
            }
        }
    }
});
