use super::*;
use alloc::string::String;

#[test]
fn external_assets_sums_principals() {
    let mut state = PolicyState::new();
    state.set_principal(1, 100);
    state.set_principal(2, 250);
    state.set_principal(3, 50);

    assert_eq!(state.external_assets(), 400);
}

#[test]
fn cap_group_totals_aggregate_by_group() {
    let mut state = PolicyState::new();
    let group_a = CapGroupId::new("group-a");
    let group_b = CapGroupId::new("group-b");

    state.set_market_config(1, MarketConfig::new(true, Some(group_a.clone())));
    state.set_market_config(2, MarketConfig::new(true, Some(group_a.clone())));
    state.set_market_config(3, MarketConfig::new(true, Some(group_b.clone())));

    state.set_principal(1, 10);
    state.set_principal(2, 20);
    state.set_principal(3, 40);

    let totals = state.compute_cap_group_totals();
    assert_eq!(totals.get(&group_a).copied().unwrap_or(0), 30);
    assert_eq!(totals.get(&group_b).copied().unwrap_or(0), 40);
}

#[test]
fn refresh_cap_group_principals_updates_records() {
    let mut state = PolicyState::new();
    let group = CapGroupId::new(String::from("group"));
    state
        .cap_groups
        .insert(group.clone(), CapGroupRecord::default());
    state.set_market_config(1, MarketConfig::new(true, Some(group.clone())));
    state.set_principal(1, 123);

    state.refresh_cap_group_principals();

    let record = state.cap_groups.get(&group).expect("cap group");
    assert_eq!(record.principal, 123);
}
