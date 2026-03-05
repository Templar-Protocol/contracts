use super::{nonnegative_i128_to_u128, seconds_to_nanoseconds, u128_to_i128_checked};

#[test]
fn converts_seconds_to_nanoseconds() {
    assert_eq!(seconds_to_nanoseconds(1), Some(1_000_000_000));
    assert_eq!(seconds_to_nanoseconds(42), Some(42_000_000_000));
}

#[test]
fn returns_none_on_overflow() {
    assert_eq!(seconds_to_nanoseconds(u64::MAX), None);
}

#[test]
fn converts_u128_to_i128_when_in_range() {
    assert_eq!(u128_to_i128_checked(0), Some(0));
    assert_eq!(u128_to_i128_checked(i128::MAX as u128), Some(i128::MAX));
}

#[test]
fn rejects_u128_to_i128_when_out_of_range() {
    assert_eq!(u128_to_i128_checked((i128::MAX as u128) + 1), None);
}

#[test]
fn converts_nonnegative_i128_to_u128() {
    assert_eq!(nonnegative_i128_to_u128(0), Some(0));
    assert_eq!(nonnegative_i128_to_u128(42), Some(42));
}

#[test]
fn rejects_negative_i128_to_u128() {
    assert_eq!(nonnegative_i128_to_u128(-1), None);
}
