use super::*;

const WAD: u128 = Wad::SCALE;

#[test]
fn builds_cap_group_and_record_from_fields() {
    let cap = cap_group_from_fields(1_000, Wad::from(WAD / 2));
    assert_eq!(cap.absolute_cap.map(|v| v.get()), Some(1_000));
    assert_eq!(cap.relative_cap, Some(Wad::from(WAD / 2)));

    let record = cap_group_record_from_fields(1_000, Wad::from(WAD / 2), 300);
    assert_eq!(record.principal, 300);
    assert_eq!(record.cap.absolute_cap.map(|v| v.get()), Some(1_000));
}

#[test]
fn alloc_helpers_match_cap_group_behavior() {
    assert!(can_allocate_from_fields(1_000, Wad::one(), 300, 500, 2_000));
    assert!(!can_allocate_from_fields(
        1_000,
        Wad::one(),
        300,
        800,
        2_000
    ));

    assert!(enforce_from_fields(1_000, Wad::one(), 300, 500, 2_000).is_ok());
    assert!(enforce_from_fields(1_000, Wad::one(), 300, 800, 2_000).is_err());
}

#[test]
fn computes_effective_and_available_from_fields() {
    assert_eq!(effective_cap_from_fields(1_000, Wad::one(), 500), 500);
    assert_eq!(
        available_capacity_from_fields(1_000, Wad::one(), 300, 500),
        200
    );
}

#[test]
fn record_field_helpers_preserve_unlimited_defaults_and_principal() {
    let mut record = cap_group_record_from_fields(0, Wad::one(), 123);

    assert_eq!(cap_group_record_absolute_cap(&record), 0);
    assert_eq!(cap_group_record_relative_cap(&record), Wad::one());
    assert_eq!(record.principal, 123);

    set_cap_group_record_absolute_cap(&mut record, 7_500);
    assert_eq!(cap_group_record_absolute_cap(&record), 7_500);
    assert_eq!(cap_group_record_relative_cap(&record), Wad::one());
    assert_eq!(record.principal, 123);

    let three_quarters = Wad::from(WAD * 3 / 4);
    set_cap_group_record_relative_cap(&mut record, three_quarters);
    assert_eq!(cap_group_record_absolute_cap(&record), 7_500);
    assert_eq!(cap_group_record_relative_cap(&record), three_quarters);
    assert_eq!(record.principal, 123);
}
