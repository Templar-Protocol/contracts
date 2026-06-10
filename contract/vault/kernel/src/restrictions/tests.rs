use super::*;

fn addr(tag: u8) -> Address {
    Address([tag; 32])
}

#[test]
fn test_blacklist_blocks_listed() {
    let restrictions = Restrictions::blacklist(alloc::vec![addr(1)]);

    assert_eq!(
        restrictions.is_restricted(&addr(1)),
        Some(RestrictionKind::Blacklisted)
    );
    assert_eq!(restrictions.is_restricted(&addr(3)), None);
}

#[test]
fn whitelist_blocks_non_members_by_default() {
    let restrictions = Restrictions::whitelist(alloc::vec![addr(1)]);

    let self_id = addr(2);
    assert_eq!(restrictions.is_restricted(&addr(1)), None);
    assert_eq!(
        restrictions.is_restricted(&self_id),
        Some(RestrictionKind::NotWhitelisted)
    );
    assert_eq!(
        restrictions.is_restricted(&addr(3)),
        Some(RestrictionKind::NotWhitelisted)
    );
}

#[test]
fn whitelist_self_bypass_is_explicit() {
    let restrictions = Restrictions::whitelist(alloc::vec![addr(1)]);
    let self_id = addr(2);

    assert_eq!(
        restrictions.is_restricted_allowing_self(&self_id, &self_id),
        None
    );
    assert_eq!(
        restrictions.is_restricted_allowing_self(&addr(3), &self_id),
        Some(RestrictionKind::NotWhitelisted)
    );
}

#[test]
fn normalized_restriction_lists_dedup_preserves_order() {
    let restrictions = Restrictions::blacklist(alloc::vec![addr(3), addr(1), addr(3), addr(2)]);

    assert_eq!(
        restrictions,
        Restrictions::blacklist(alloc::vec![addr(3), addr(1), addr(2)])
    );
}

#[test]
fn normalized_round_trip_keeps_canonical_form() {
    let restrictions = Restrictions::blacklist(alloc::vec![addr(2), addr(1), addr(2)]).normalized();

    assert_eq!(
        restrictions,
        Restrictions::blacklist(alloc::vec![addr(2), addr(1)])
    );
}
