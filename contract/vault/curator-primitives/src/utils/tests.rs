use super::seconds_to_nanoseconds;

#[test]
fn converts_seconds_to_nanoseconds() {
    assert_eq!(seconds_to_nanoseconds(1), Some(1_000_000_000));
    assert_eq!(seconds_to_nanoseconds(42), Some(42_000_000_000));
}

#[test]
fn returns_none_on_overflow() {
    assert_eq!(seconds_to_nanoseconds(u64::MAX), None);
}
