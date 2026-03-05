use crate::error::{ContractError, RuntimeError};

#[inline]
fn u128_to_i128_with(
    value: u128,
    msg: &'static str,
    err: fn(&'static str) -> RuntimeError,
) -> Result<i128, RuntimeError> {
    match i128::try_from(value) {
        Ok(value) => Ok(value),
        Err(_) => Err(err(msg)),
    }
}

#[inline]
pub(crate) fn u128_to_i128_effect(value: u128, msg: &'static str) -> Result<i128, RuntimeError> {
    u128_to_i128_with(value, msg, RuntimeError::effect_failed)
}

/// Shared ledger timestamp → nanoseconds conversion.
pub(crate) fn ledger_timestamp_ns(env: &soroban_sdk::Env) -> Result<u64, ContractError> {
    match env.ledger().timestamp().checked_mul(1_000_000_000) {
        Some(ns) => Ok(ns),
        None => Err(ContractError::ConversionOverflow),
    }
}

/// Convert RuntimeError to ContractError.
#[inline]
pub(crate) fn runtime_to_contract<T>(result: Result<T, RuntimeError>) -> Result<T, ContractError> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => Err(ContractError::from(err)),
    }
}

/// Safe u128 → i128 conversion.
pub(crate) fn to_i128(v: u128) -> Result<i128, ContractError> {
    match i128::try_from(v) {
        Ok(value) => Ok(value),
        Err(_) => Err(ContractError::ConversionOverflow),
    }
}

/// Safe i128 → u128 conversion (rejects negative).
pub(crate) fn to_u128(v: i128) -> Result<u128, ContractError> {
    if v < 0 {
        return Err(ContractError::InvalidInput);
    }
    Ok(v as u128)
}
