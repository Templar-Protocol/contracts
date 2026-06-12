#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

fuzz_target!(|data: &[u8]| {
    // Try to parse as UTF-8 string
    if let Ok(s) = std::str::from_utf8(data) {
        // Attempt to parse as Decimal
        if let Ok(decimal) = Decimal::from_str(s) {
            // If parsing succeeds, test round-trip
            let to_string = decimal.to_fixed(38);

            // Parse again and check near equality
            if let Ok(reparsed) = Decimal::from_str(&to_string) {
                assert!(
                    decimal.near_equal(reparsed),
                    "Round-trip failed: original={decimal:?}, string={to_string}, reparsed={reparsed:?}",
                );
            }

            // Test various precision levels
            for precision in [0, 1, 5, 10, 20, 38] {
                let fixed = decimal.to_fixed(precision);
                let _ = Decimal::from_str(&fixed);
            }

            // Test conversions
            let _ = decimal.to_u128_floor();
            let _ = decimal.to_u128_ceil();
            let _ = decimal.to_f64_lossy();

            // Test that is_zero is consistent
            if decimal.is_zero() {
                assert_eq!(decimal, Decimal::ZERO);
            }

            // Test basic operations on parsed value
            let _ = decimal + Decimal::ONE;
            let _ = decimal * Decimal::TWO;
            if !decimal.is_zero() {
                let _ = Decimal::ONE / decimal;
            }
        }
    }

    // Test with constructed string patterns
    if data.len() >= 2 {
        let whole_part = u128::from(data[0]);
        let frac_digit = data[1] % 10;

        // Create a decimal string manually
        let test_str = format!("{whole_part}.{frac_digit}");
        if let Ok(decimal) = Decimal::from_str(&test_str) {
            let _ = decimal.to_fixed(10);
        }
    }

    // Test edge case strings
    let edge_cases = [
        "0",
        "1",
        "0.0",
        "1.0",
        "0.1",
        "0.00000001",
        "999999999999999",
    ];

    for case in edge_cases {
        if let Ok(decimal) = Decimal::from_str(case) {
            let stringified = decimal.to_fixed(38);
            let _ = Decimal::from_str(&stringified);
        }
    }

    // Test malformed strings don't panic
    let malformed = [
        ".", ".0", "0.", "abc", "1.2.3", "-1", "1e10", "NaN", "Infinity", "",
    ];

    for mal in malformed {
        let _ = Decimal::from_str(mal);
    }
});
