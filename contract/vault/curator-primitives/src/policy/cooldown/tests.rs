use super::*;

#[test]
fn test_unlimited_cooldown() {
    let cooldown = Cooldown::unlimited();
    assert!(cooldown.is_unlimited());
    assert!(cooldown.is_ready(0));
    assert!(cooldown.is_ready(u64::MAX));
}

#[test]
fn test_first_operation_always_ready() {
    let cooldown = Cooldown::new(1000);
    assert!(cooldown.is_ready(0));
    assert!(cooldown.is_ready(500));
}

#[test]
fn test_cooldown_enforced() {
    let cooldown = Cooldown::new(1000);
    let cooldown = cooldown.record(100);

    // Not ready yet
    assert!(!cooldown.is_ready(100));
    assert!(!cooldown.is_ready(500));
    assert!(!cooldown.is_ready(1099));

    // Ready at exactly interval
    assert!(cooldown.is_ready(1100));
    assert!(cooldown.is_ready(2000));
}

#[test]
fn test_check_returns_error() {
    let cooldown = Cooldown::with_last_event(1000, 100);

    let result = cooldown.check(500);
    assert!(matches!(result, Err(CooldownError::OnCooldown { .. })));

    let result = cooldown.check(1100);
    assert!(result.is_ok());
}

#[test]
fn test_ready_at() {
    let cooldown = Cooldown::new(1000);
    assert_eq!(cooldown.ready_at(), None); // No last event

    let cooldown = cooldown.record(100);
    assert_eq!(cooldown.ready_at(), Some(1100));

    let unlimited = Cooldown::unlimited();
    assert_eq!(unlimited.ready_at(), None);
}

#[test]
fn test_remaining() {
    let cooldown = Cooldown::with_last_event(1000, 100);

    assert_eq!(cooldown.remaining(100), 1000);
    assert_eq!(cooldown.remaining(500), 600);
    assert_eq!(cooldown.remaining(1100), 0);
    assert_eq!(cooldown.remaining(2000), 0);
}

#[test]
fn test_record_updates_last_event() {
    let cooldown = Cooldown::new(1000);
    assert_eq!(cooldown.last_event_ns, None);

    let cooldown = cooldown.record(500);
    assert_eq!(cooldown.last_event_ns, Some(500));

    let cooldown = cooldown.record(1500);
    assert_eq!(cooldown.last_event_ns, Some(1500));
}
