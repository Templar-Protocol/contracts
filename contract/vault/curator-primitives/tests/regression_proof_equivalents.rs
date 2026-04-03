use templar_curator_primitives::policy::withdraw_route::{
    build_withdraw_route, build_withdraw_route_with_liquidity,
};

#[cfg(feature = "recovery")]
#[test]
fn settlement_conserves_escrow() {
    use templar_curator_primitives::recovery::{compute_settlement_shares, RecoveryError};

    let cases = [
        (100_u128, 100_u128, 100_u128, Ok((100_u128, 0_u128))),
        (100_u128, 200_u128, 50_u128, Ok((25_u128, 75_u128))),
        (0_u128, 100_u128, 10_u128, Ok((0_u128, 0_u128))),
        (
            100_u128,
            0_u128,
            0_u128,
            Err(RecoveryError::ExpectedAmountZero {
                escrow_shares: 100_u128,
                collected_amount: 0_u128,
            }),
        ),
    ];

    for (escrow, expected, collected, expected_result) in cases {
        let settlement = compute_settlement_shares(escrow, expected, collected);
        match (settlement, expected_result) {
            (Ok(settlement), Ok((expected_burn, expected_refund))) => {
                assert_eq!(settlement.to_burn, expected_burn);
                assert_eq!(settlement.refund, expected_refund);
                assert_eq!(settlement.to_burn.saturating_add(settlement.refund), escrow);
                assert!(settlement.to_burn <= escrow);
                assert!(settlement.refund <= escrow);
            }
            (Err(actual_error), Err(expected_error)) => assert_eq!(actual_error, expected_error),
            (actual, expected) => {
                panic!("unexpected settlement result: actual={actual:?} expected={expected:?}")
            }
        }
    }
}

#[test]
fn withdraw_route_ordering_tie_breaker() {
    let principals = vec![(2_u32, 100_u128), (1_u32, 100_u128)];
    let route = build_withdraw_route(&principals, 1).unwrap();
    assert_eq!(route.entries()[0].target_id(), 1);
    assert_eq!(route.entries()[1].target_id(), 2);

    let market_data = vec![(2_u32, 100_u128, 50_u128), (1_u32, 10_u128, 50_u128)];
    let route = build_withdraw_route_with_liquidity(&market_data, 1).unwrap();
    assert_eq!(route.entries()[0].target_id(), 2);
    assert_eq!(route.entries()[1].target_id(), 1);
}
