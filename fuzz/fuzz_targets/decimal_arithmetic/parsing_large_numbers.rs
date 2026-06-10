#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

fuzz_target!(|data: &[u8]| {
    // Generate additional numeric edge cases from fuzz data
    if data.len() >= 8 {
        let large_whole = u128::from_le_bytes([
            data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7], 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);

        let fuzz_edge_cases = [
            format!("{large_whole}"),
            format!("{large_whole}.0"),
            format!("0.{large_whole}"),
        ];

        for case in fuzz_edge_cases {
            if let Ok(decimal) = Decimal::from_str(&case) {
                let precision = (data[7] % 39) as usize;
                let _ = decimal.to_fixed(precision);
            }
        }
    }
});
