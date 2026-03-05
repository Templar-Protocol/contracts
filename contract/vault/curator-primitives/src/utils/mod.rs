/// Convert seconds to nanoseconds, returning `None` on overflow.
#[must_use]
pub fn seconds_to_nanoseconds(seconds: u64) -> Option<u64> {
    seconds.checked_mul(1_000_000_000)
}

#[cfg(test)]
mod tests;
