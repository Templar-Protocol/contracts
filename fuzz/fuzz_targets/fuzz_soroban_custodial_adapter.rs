#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use templar_soroban_custodial_adapter::{simulate_progress_withdrawal, AdapterError};

#[derive(Arbitrary, Debug)]
struct CustodialAdapterInput {
    reported: i128,
    idle_balance: i128,
    requested: i128,
}

fn bounded(value: i128) -> i128 {
    value % 1_000_000_000_000_000_000
}

fuzz_target!(|input: CustodialAdapterInput| {
    let reported = bounded(input.reported);
    let idle_balance = bounded(input.idle_balance);
    let requested = bounded(input.requested);
    let result = simulate_progress_withdrawal(reported, idle_balance, requested);

    if requested <= 0 || reported < 0 || idle_balance < 0 {
        assert_eq!(result, Err(AdapterError::InvalidInput));
        return;
    }

    if reported == 0 || idle_balance == 0 {
        assert_eq!(result, Err(AdapterError::InsufficientReturnedLiquidity));
        return;
    }

    let (actual, next_reported) = result.expect("positive inputs should withdraw");
    assert!(actual > 0);
    assert!(actual <= reported);
    assert!(actual <= idle_balance);
    assert!(actual <= requested);
    assert_eq!(next_reported + actual, reported);
    assert!(next_reported >= 0);
});
