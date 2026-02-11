use super::*;
use alloc::vec;

const WAD: u128 = 1_000_000_000_000_000_000_000_000;

#[test]
fn test_cap_group_unlimited() {
    let cap = CapGroup::new();
    assert!(cap.is_unlimited());
    assert!(cap.can_allocate(0, u128::MAX, 1000));
}

#[test]
fn test_cap_group_absolute_only() {
    let cap = CapGroup::absolute_only(1000);
    assert!(!cap.is_unlimited());
    assert!(cap.absolute_cap.is_some());
    assert!(cap.relative_cap.is_none());

    // Can allocate up to cap
    assert!(cap.can_allocate(0, 1000, 10000));
    assert!(cap.can_allocate(500, 500, 10000));

    // Cannot exceed cap
    assert!(!cap.can_allocate(500, 501, 10000));
    assert!(!cap.can_allocate(1000, 1, 10000));
}

#[test]
fn test_cap_group_relative_only() {
    // 50% relative cap
    let cap = CapGroup::relative_only(Wad::from(WAD / 2));
    assert!(!cap.is_unlimited());
    assert!(cap.absolute_cap.is_none());
    assert!(cap.relative_cap.is_some());

    // Total assets = 1000, effective cap = 500
    assert!(cap.can_allocate(0, 500, 1000));
    assert!(cap.can_allocate(200, 300, 1000));
    assert!(!cap.can_allocate(200, 301, 1000));
}

#[test]
fn test_cap_group_both_caps() {
    // 1000 absolute, 50% relative
    let cap = CapGroup::new()
        .with_absolute(1000)
        .with_relative(Wad::from(WAD / 2));

    // With 3000 total assets, relative cap = 1500, but absolute = 1000
    assert!(cap.can_allocate(0, 1000, 3000));
    assert!(!cap.can_allocate(0, 1001, 3000));

    // With 1000 total assets, relative cap = 500, which is stricter
    assert!(cap.can_allocate(0, 500, 1000));
    assert!(!cap.can_allocate(0, 501, 1000));
}

#[test]
fn test_compute_effective_cap() {
    let cap = CapGroup::new()
        .with_absolute(1000)
        .with_relative(Wad::from(WAD / 2));

    // When relative cap is stricter
    assert_eq!(cap.effective_cap(1000), 500);

    // When absolute cap is stricter
    assert_eq!(cap.effective_cap(3000), 1000);

    // Unlimited
    let unlimited = CapGroup::new();
    assert_eq!(unlimited.effective_cap(1000), u128::MAX);
}

#[test]
fn test_enforce_cap_group_errors() {
    let cap = CapGroup::new()
        .with_absolute(1000)
        .with_relative(Wad::from(WAD / 2));

    // Exceeds absolute cap
    let result = cap.enforce(0, 1001, 3000);
    assert!(matches!(
        result,
        Err(CapGroupError::ExceedsAbsoluteCap { .. })
    ));

    // Exceeds relative cap (500 effective cap when total = 1000)
    let result = cap.enforce(0, 501, 1000);
    assert!(matches!(
        result,
        Err(CapGroupError::ExceedsRelativeCap { .. })
    ));
}

#[test]
fn test_compute_available_capacity() {
    let cap = CapGroup::absolute_only(1000);

    assert_eq!(cap.available_capacity(0, 2000), 1000);
    assert_eq!(cap.available_capacity(300, 2000), 700);
    assert_eq!(cap.available_capacity(1000, 2000), 0);
    assert_eq!(cap.available_capacity(1500, 2000), 0); // Already over, saturates to 0
}

#[test]
fn test_apply_and_remove_allocation() {
    let cap = CapGroup::absolute_only(1000);
    let record = CapGroupRecord::new(cap);

    let updated = record.apply_allocation(300);
    assert_eq!(updated.principal, 300);

    let reduced = updated.remove_allocation(100);
    assert_eq!(reduced.principal, 200);

    // Saturating subtraction
    let zero = reduced.remove_allocation(500);
    assert_eq!(zero.principal, 0);
}

#[test]
fn test_validate_allocations() {
    let cap1 = CapGroupRecord::new(CapGroup::absolute_only(1000));
    let cap2 = CapGroupRecord::new(CapGroup::absolute_only(500));

    // Valid allocations
    let allocations = vec![(cap1.clone(), 500), (cap2.clone(), 300)];
    assert!(validate_allocations(&allocations, 2000).is_ok());

    // Invalid - second exceeds cap
    let invalid = vec![(cap1, 500), (cap2, 600)];
    assert!(validate_allocations(&invalid, 2000).is_err());
}

#[test]
fn test_cap_group_record_methods() {
    let record = CapGroupRecord::new(CapGroup::absolute_only(1000));

    assert!(record.can_allocate(500, 2000));
    assert!(!record.can_allocate(1001, 2000));
    assert_eq!(record.available_capacity(2000), 1000);

    assert!(record.enforce(500, 2000).is_ok());
    assert!(record.enforce(1001, 2000).is_err());
}

#[test]
fn test_zero_absolute_cap_is_unlimited() {
    let cap = CapGroup::absolute_only(0);
    // NonZeroU128::new(0) returns None, so this should be unlimited
    assert!(cap.absolute_cap.is_none());
}

proptest::proptest! {
    #[test]
    fn prop_available_capacity_matches_effective_cap(
        absolute in 0u128..=1_000_000_000_000u128,
        relative in 0u128..=WAD,
        current in 0u128..=1_000_000_000_000u128,
        total in 0u128..=1_000_000_000_000u128,
    ) {
        let cap = CapGroup::new()
            .with_absolute(absolute)
            .with_relative(Wad::from(relative));
        let effective = cap.effective_cap(total);
        let available = cap.available_capacity(current, total);

        if cap.is_unlimited() {
            proptest::prop_assert_eq!(available, u128::MAX);
        } else {
            proptest::prop_assert_eq!(available, effective.saturating_sub(current));
        }
    }
}
