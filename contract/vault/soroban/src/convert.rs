use crate::error::RuntimeError;

#[inline]
fn u128_to_i128_with(
    value: u128,
    msg: &'static str,
    err: fn(&'static str) -> RuntimeError,
) -> Result<i128, RuntimeError> {
    i128::try_from(value).map_err(|_| err(msg))
}

#[inline]
fn i128_to_u128_with(
    value: i128,
    msg: &'static str,
    err: fn(&'static str) -> RuntimeError,
) -> Result<u128, RuntimeError> {
    u128::try_from(value).map_err(|_| err(msg))
}

#[inline]
pub(crate) fn u128_to_i128_storage(value: u128, msg: &'static str) -> Result<i128, RuntimeError> {
    u128_to_i128_with(value, msg, RuntimeError::storage_error)
}

#[inline]
pub(crate) fn i128_to_u128_storage(value: i128, msg: &'static str) -> Result<u128, RuntimeError> {
    i128_to_u128_with(value, msg, RuntimeError::storage_error)
}

#[inline]
pub(crate) fn u128_to_i128_effect(value: u128, msg: &'static str) -> Result<i128, RuntimeError> {
    u128_to_i128_with(value, msg, RuntimeError::effect_failed)
}
