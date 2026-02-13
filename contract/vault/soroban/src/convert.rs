use crate::error::RuntimeError;

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
