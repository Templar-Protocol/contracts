#![no_main]

use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

fuzz_target!(|data: u128| {
    let dec_a = Decimal::from(data);

    // String formatting with different precisions
    let _ = dec_a.to_fixed(38);
    let _ = dec_a.to_fixed(10);
    let _ = dec_a.to_fixed(0);

    // Round-trip string conversion
    let str_repr = dec_a.to_fixed(20);
    if let Ok(parsed) = Decimal::from_str(&str_repr) {
        let _ = dec_a.near_equal(parsed);
    }
});
