#![no_main]

use libfuzzer_sys::fuzz_target;
use templar_common::number::Decimal;

fuzz_target!(|data: u128| {
    let dec_a = Decimal::from(data);

    // Type conversions
    let _ = dec_a.to_u128_floor();
    let _ = dec_a.to_u128_ceil();
    let _ = dec_a.to_f64_lossy();

    // Test ceiling and floor relationship
    if let (Some(floor), Some(ceil)) = (dec_a.to_u128_floor(), dec_a.to_u128_ceil()) {
        let _ = ceil >= floor;
    }
});