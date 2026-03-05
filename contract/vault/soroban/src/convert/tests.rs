use super::{ledger_timestamp_ns, runtime_to_contract, to_i128, to_u128, u128_to_i128_effect};
use crate::error::{ContractError, RuntimeError};
use soroban_sdk::testutils::{Ledger as _, LedgerInfo};
use soroban_sdk::Env;

#[test]
fn to_i128_converts_in_range() {
    assert_eq!(to_i128(0).expect("zero must convert"), 0);
    assert_eq!(
        to_i128(i128::MAX as u128).expect("max must convert"),
        i128::MAX
    );
}

#[test]
fn to_i128_rejects_overflow() {
    assert_eq!(
        to_i128((i128::MAX as u128) + 1).expect_err("overflow must fail"),
        ContractError::ConversionOverflow
    );
}

#[test]
fn to_u128_rejects_negative() {
    assert_eq!(
        to_u128(-1).expect_err("negative must fail"),
        ContractError::InvalidInput
    );
}

#[test]
fn u128_to_i128_effect_sets_effect_error() {
    let err = u128_to_i128_effect((i128::MAX as u128) + 1, "event amount overflow")
        .expect_err("overflow must fail");
    assert_eq!(err, RuntimeError::effect_failed("event amount overflow"));
}

#[test]
fn runtime_to_contract_maps_error() {
    let err =
        runtime_to_contract::<()>(Err(RuntimeError::InvalidInput)).expect_err("error must map");
    assert_eq!(err, ContractError::InvalidInput);
}

#[test]
fn ledger_timestamp_converts_to_ns() {
    let env = Env::default();
    env.ledger().set(LedgerInfo {
        timestamp: 123,
        protocol_version: 23,
        ..Default::default()
    });

    assert_eq!(
        ledger_timestamp_ns(&env).expect("timestamp conversion must succeed"),
        123_000_000_000
    );
}
