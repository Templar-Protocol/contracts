#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

// MUTATION-CHECK (P5): in `Decimal::from_str` (primitives/src/number.rs), make
// `is_zero` parsing lossy — e.g. treat a tiny non-zero fractional input as
// zero. Then the `decimal.is_zero() ⇒ decimal == Decimal::ZERO` consistency
// assertion, or the `to_fixed(38)` round-trip near-equality, must fire.

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

            // Basic operations on parsed value, guarded against overflow:
            // skip if `decimal` is close to MAX so `+ ONE` / `* TWO` won't
            // panic on intentional overflow checks.
            if decimal <= Decimal::MAX / Decimal::TWO {
                let _ = decimal + Decimal::ONE;
                let _ = decimal * Decimal::TWO;
            }
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

    // Migrated from decimal_arithmetic/malformed/*.rs: nudge the fuzzer toward
    // specific shapes of invalid input that exercise distinct branches of the
    // parser. libFuzzer can find these via raw &[u8] mutation but seeding
    // shaped strings makes coverage discovery much faster on cold corpora.
    if !data.is_empty() {
        let b = data[0];
        let alpha = (b'a' + (b % 26)) as char;
        let specials = ["NaN", "Infinity", "inf", "null"];
        // Owned to satisfy the array's homogeneous String type.
        let shaped: [String; 24] = [
            // alphabetic
            format!("{b}{alpha}"),
            format!("{alpha}{b}c"),
            format!("ab{b}"),
            alpha.to_string(),
            // invalid separators
            format!(".{b}"),
            format!("{b}.."),
            format!("..{b}"),
            // multiple decimal points
            format!("{}.{}.{}", b % 10, b % 100, b % 200),
            format!("1.2.{b}"),
            // sign errors
            format!("-{b}"),
            format!("-{}.{}", b % 10, b % 100),
            format!("+-{b}"),
            "-".to_string(),
            // scientific notation (unsupported)
            format!("{}e{}", b % 10, b % 10),
            format!("{b}E{}", b % 100),
            format!("1e{b}"),
            format!("{}e+{}", b % 10, b % 10),
            // mixed garbage
            format!("{b}#{b}"),
            format!("@{}.{}", b % 10, b % 10),
            format!("${b}"),
            // special values + suffixed
            specials[(b % 4) as usize].to_string(),
            format!("{}{b}", specials[(b % 4) as usize]),
            // whitespace
            format!(" {b} "),
            format!("\t{b}"),
        ];
        for s in &shaped {
            let _ = Decimal::from_str(s);
        }
    }
});
