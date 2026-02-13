use super::*;
use alloc::vec;

#[test]
fn test_new_route() {
    let route = WithdrawRoute::new(1000);
    assert!(route.is_empty());
    assert_eq!(route.target_amount, 1000);
}

#[test]
fn test_builder_pattern() {
    let route = WithdrawRoute::new(1000)
        .with_entry(WithdrawRouteEntry::new(1, 500))
        .with_entry(WithdrawRouteEntry::new(2, 600));

    assert_eq!(route.len(), 2);
    assert_eq!(route.total(), 1100);
}

#[test]
fn test_entry_builder() {
    let entry = WithdrawRouteEntry::new(1, 500).with_liquidity(400);

    assert_eq!(entry.target_id, 1);
    assert_eq!(entry.max_amount, 500);
    assert_eq!(entry.available_liquidity, Some(400));
}

#[test]
fn test_compute_route_total() {
    let route = WithdrawRoute::from_entries(
        vec![
            WithdrawRouteEntry::new(1, 500),
            WithdrawRouteEntry::new(2, 300),
            WithdrawRouteEntry::new(3, 200),
        ],
        1000,
    );

    assert_eq!(route.total(), 1000);
}

#[test]
fn test_validate_withdraw_route_success() {
    let route = WithdrawRoute::from_entries(
        vec![
            WithdrawRouteEntry::new(1, 500),
            WithdrawRouteEntry::new(2, 600),
        ],
        1000,
    );

    assert!(route.validate().is_ok());
}

#[test]
fn test_validate_withdraw_route_zero_target() {
    let route = WithdrawRoute::from_entries(vec![WithdrawRouteEntry::new(1, 500)], 0);

    assert!(matches!(
        route.validate(),
        Err(WithdrawRouteError::ZeroTargetAmount)
    ));
}

#[test]
fn test_validate_withdraw_route_empty() {
    let route = WithdrawRoute::new(1000);

    assert!(matches!(
        route.validate(),
        Err(WithdrawRouteError::EmptyRoute)
    ));
}

#[test]
fn test_validate_withdraw_route_insufficient() {
    let route = WithdrawRoute::from_entries(
        vec![WithdrawRouteEntry::new(1, 500)],
        1000, // target > route total
    );

    assert!(matches!(
        route.validate(),
        Err(WithdrawRouteError::InsufficientRouteTotal { .. })
    ));
}

#[test]
fn test_validate_withdraw_route_duplicate() {
    let route = WithdrawRoute::from_entries(
        vec![
            WithdrawRouteEntry::new(1, 500),
            WithdrawRouteEntry::new(1, 600), // duplicate target
        ],
        1000,
    );

    assert!(matches!(
        route.validate(),
        Err(WithdrawRouteError::DuplicateTarget { target_id: 1 })
    ));
}

#[test]
fn test_validate_withdraw_route_zero_max() {
    let route = WithdrawRoute::from_entries(
        vec![
            WithdrawRouteEntry::new(1, 500),
            WithdrawRouteEntry::new(2, 0), // zero max
        ],
        500,
    );

    assert!(matches!(
        route.validate(),
        Err(WithdrawRouteError::ZeroMaxAmount { target_id: 2 })
    ));
}

#[test]
fn test_build_withdraw_route() {
    let principals = vec![(1, 1000), (2, 500), (3, 300)];

    let route = build_withdraw_route(&principals, 800).unwrap();

    // Should be sorted by principal (largest first)
    assert_eq!(route.entries[0].target_id, 1);
    assert_eq!(route.entries[1].target_id, 2);
    assert_eq!(route.entries[2].target_id, 3);
    assert_eq!(route.target_amount, 800);
}

#[test]
fn test_build_withdraw_route_tie_breaker() {
    let principals = vec![(2, 1000), (1, 1000), (3, 500)];

    let route = build_withdraw_route(&principals, 100).unwrap();

    // Equal principals should be ordered by target_id asc
    assert_eq!(route.entries[0].target_id, 1);
    assert_eq!(route.entries[1].target_id, 2);
    assert_eq!(route.entries[2].target_id, 3);
}

#[test]
fn test_build_withdraw_route_insufficient() {
    let principals = vec![(1, 100), (2, 50)];

    let result = build_withdraw_route(&principals, 200);

    assert!(matches!(
        result,
        Err(WithdrawRouteError::InsufficientRouteTotal { .. })
    ));
}

#[test]
fn test_build_withdraw_route_with_liquidity() {
    let market_data = vec![
        (1, 1000, 800), // principal 1000, liquidity 800
        (2, 500, 500),  // principal 500, liquidity 500
        (3, 300, 100),  // principal 300, liquidity 100
    ];

    let route = build_withdraw_route_with_liquidity(&market_data, 500).unwrap();

    // Should be sorted by liquidity (highest first)
    assert_eq!(route.entries[0].target_id, 1);
    assert_eq!(route.entries[0].max_amount, 800); // min(1000, 800)
    assert_eq!(route.entries[0].available_liquidity, Some(800));
}

#[test]
fn test_build_withdraw_route_with_liquidity_tie_breaker() {
    let market_data = vec![(2, 1000, 500), (1, 200, 500), (3, 300, 400)];

    let route = build_withdraw_route_with_liquidity(&market_data, 100).unwrap();

    // Equal liquidity should be ordered by target_id asc
    assert_eq!(route.entries[0].target_id, 1);
    assert_eq!(route.entries[1].target_id, 2);
    assert_eq!(route.entries[2].target_id, 3);
}

#[test]
fn test_compute_available_liquidity() {
    let route = WithdrawRoute::from_entries(
        vec![
            WithdrawRouteEntry::new(1, 500).with_liquidity(400),
            WithdrawRouteEntry::new(2, 300), // no liquidity info
            WithdrawRouteEntry::new(3, 200).with_liquidity(200),
        ],
        1000,
    );

    assert_eq!(route.available_liquidity(), 600);
}

#[test]
fn test_to_withdrawal_plan() {
    let route = WithdrawRoute::from_entries(
        vec![
            WithdrawRouteEntry::new(1, 500),
            WithdrawRouteEntry::new(2, 300),
        ],
        800,
    );

    let plan = route.to_withdrawal_plan();

    assert_eq!(plan, vec![(1, 500), (2, 300)]);
}

#[test]
fn test_can_satisfy() {
    let route = WithdrawRoute::from_entries(vec![WithdrawRouteEntry::new(1, 500)], 1000);
    assert!(!route.can_satisfy());

    let route = WithdrawRoute::from_entries(vec![WithdrawRouteEntry::new(1, 1000)], 1000);
    assert!(route.can_satisfy());
}

#[test]
fn test_get_entry_and_has_target() {
    let route = WithdrawRoute::from_entries(
        vec![
            WithdrawRouteEntry::new(1, 500),
            WithdrawRouteEntry::new(2, 300),
        ],
        800,
    );

    assert!(route.has_target(1));
    assert!(route.has_target(2));
    assert!(!route.has_target(3));

    let entry = route.get_entry(1);
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().max_amount, 500);

    assert!(route.get_entry(3).is_none());
}
