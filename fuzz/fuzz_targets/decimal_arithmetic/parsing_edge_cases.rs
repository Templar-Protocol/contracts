#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

fuzz_target!(|data: &[u8]| {
    // Generate edge case strings using fuzz data
    if data.len() >= 6 {
        // Generate edge cases from fuzz input bytes
        let edge_byte1 = data[5];
        let edge_byte2 = if data.len() > 6 { data[6] } else { 0 };

        // Create various edge case patterns from fuzz data
        let generated_edge_cases = [
            // Simple digit patterns
            format!("{}", edge_byte1 % 10),
            format!("{}.{}", edge_byte1 % 10, edge_byte2 % 10),
            // Zero variations
            format!(
                "0.{:08}",
                u64::from(edge_byte1) * 256 + u64::from(edge_byte2)
            ),
            format!("{edge_byte1}.0"),
            // Large number variations
            format!("{edge_byte1}{edge_byte2}{edge_byte1}"),
            format!(
                "{}.{}{}",
                u64::from(edge_byte1) * u64::from(edge_byte2),
                edge_byte1,
                edge_byte2
            ),
            // Precision edge cases
            format!(
                "0.{:038}",
                u128::from(edge_byte1) * 256 + u128::from(edge_byte2)
            ), // Max precision
            format!(
                "{}",
                u128::from(edge_byte1) * u128::from(edge_byte2) * 1_000_000
            ),
        ];

        // Test each generated edge case
        for edge_case in generated_edge_cases {
            if let Ok(decimal) = Decimal::from_str(&edge_case) {
                let precision = (data[0] % 39) as usize; // Use first byte for precision
                let stringified = decimal.to_fixed(precision);
                let _ = Decimal::from_str(&stringified);
            }
        }
    }
});
