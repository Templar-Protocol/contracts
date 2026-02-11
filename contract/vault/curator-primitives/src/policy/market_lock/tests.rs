use super::validate_lock_expiry;

#[test]
fn validate_lock_expiry_accepts_valid_window() {
    assert!(validate_lock_expiry(100, 200, 200));
}

#[test]
fn validate_lock_expiry_rejects_past_or_equal_expiry() {
    assert!(!validate_lock_expiry(100, 100, 200));
    assert!(!validate_lock_expiry(100, 99, 200));
}

#[test]
fn validate_lock_expiry_rejects_excessive_duration() {
    assert!(!validate_lock_expiry(100, 400, 200));
}
