/// Convert seconds to nanoseconds, returning `None` on overflow.
#[must_use]
pub fn seconds_to_nanoseconds(seconds: u64) -> Option<u64> {
    seconds.checked_mul(1_000_000_000)
}

/// Convert `u128` to `i128`, returning `None` when out of range.
#[must_use]
pub fn u128_to_i128_checked(value: u128) -> Option<i128> {
    i128::try_from(value).ok()
}

/// Convert `i128` to `u128`, rejecting negative values.
#[must_use]
pub fn nonnegative_i128_to_u128(value: i128) -> Option<u128> {
    u128::try_from(value).ok()
}
