use super::*;

fn addr(tag: u8) -> Address {
    Address([tag; 32])
}

#[test]
fn test_paused_blocks_everyone() {
    let r = Restrictions::Paused;
    let actor = addr(1);
    let self_id = addr(2);
    assert_eq!(
        r.is_restricted(&actor, &self_id),
        Some(RestrictionKind::Paused)
    );
    assert_eq!(
        r.is_restricted(&self_id, &self_id),
        Some(RestrictionKind::Paused)
    );
}

#[test]
fn test_blacklist_blocks_listed() {
    let r = Restrictions::Blacklist(alloc::vec![addr(1)]);

    let self_id = addr(2);
    assert_eq!(
        r.is_restricted(&addr(1), &self_id),
        Some(RestrictionKind::Blacklisted)
    );
    assert_eq!(r.is_restricted(&addr(3), &self_id), None);
}

#[test]
fn test_whitelist_allows_listed_and_self() {
    let r = Restrictions::Whitelist(alloc::vec![addr(1)]);

    let self_id = addr(2);
    assert_eq!(r.is_restricted(&addr(1), &self_id), None);
    assert_eq!(r.is_restricted(&self_id, &self_id), None);
    assert_eq!(
        r.is_restricted(&addr(3), &self_id),
        Some(RestrictionKind::NotWhitelisted)
    );
}

#[test]
fn normalized_restriction_lists_sort_and_dedup() {
    let restrictions =
        Restrictions::Blacklist(alloc::vec![addr(3), addr(1), addr(3), addr(2)]).normalized();

    assert_eq!(
        restrictions,
        Restrictions::Blacklist(alloc::vec![addr(1), addr(2), addr(3)])
    );
}

#[test]
fn test_normalized_restrictions_sort_and_dedup() {
    let r = Restrictions::Blacklist(alloc::vec![addr(2), addr(1), addr(2)]).normalized();

    assert_eq!(r, Restrictions::Blacklist(alloc::vec![addr(1), addr(2)]));
}
