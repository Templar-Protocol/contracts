use super::{CapGroupId, CapGroupUpdate, CapGroupUpdateKey};

#[test]
fn cap_group_update_uses_canonical_set_cap_shape() {
    let update = CapGroupUpdate::SetCap {
        cap_group_id: CapGroupId::from("group-a"),
        new_cap: 123,
    };

    assert_eq!(
        update,
        CapGroupUpdate::SetCap {
            cap_group_id: CapGroupId::from("group-a"),
            new_cap: 123,
        }
    );
}

#[test]
fn cap_group_update_uses_canonical_set_relative_cap_shape() {
    let update = CapGroupUpdate::SetRelativeCap {
        cap_group_id: CapGroupId::from("group-b"),
        new_relative_cap_wad: 999,
    };

    assert_eq!(
        update,
        CapGroupUpdate::SetRelativeCap {
            cap_group_id: CapGroupId::from("group-b"),
            new_relative_cap_wad: 999,
        }
    );
}

#[test]
fn cap_group_update_uses_canonical_membership_shape() {
    let update = CapGroupUpdate::SetMembership {
        market_id: 77,
        cap_group_id: Some(CapGroupId::from("group-c")),
    };

    assert_eq!(
        update,
        CapGroupUpdate::SetMembership {
            market_id: 77,
            cap_group_id: Some(CapGroupId::from("group-c")),
        }
    );
}

#[test]
fn cap_group_update_key_uses_canonical_shape() {
    let key = CapGroupUpdateKey::SetRelativeCap {
        cap_group_id: CapGroupId::from("group-key"),
    };
    assert_eq!(
        key,
        CapGroupUpdateKey::SetRelativeCap {
            cap_group_id: CapGroupId::from("group-key"),
        }
    );
}
