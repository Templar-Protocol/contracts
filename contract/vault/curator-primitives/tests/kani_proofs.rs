//! Kani proofs for curator-primitives invariants.
//!
//! Run with:
//!   cargo kani --tests -p templar-curator-primitives

#[cfg(all(test, not(kani)))]
mod test_equivalents {
    use templar_curator_primitives::policy::withdraw_route::{
        build_withdraw_route, build_withdraw_route_with_liquidity,
    };
    use templar_curator_primitives::recovery::compute_settlement_shares;

    #[test]
    fn settlement_conserves_escrow() {
        let cases = [
            (100u128, 100u128, 100u128),
            (100u128, 200u128, 50u128),
            (0u128, 100u128, 10u128),
            (100u128, 0u128, 0u128),
        ];

        for (escrow, expected, collected) in cases {
            let settlement = compute_settlement_shares(escrow, expected, collected);
            assert_eq!(settlement.to_burn.saturating_add(settlement.refund), escrow);
            assert!(settlement.to_burn <= escrow);
            assert!(settlement.refund <= escrow);
        }
    }

    #[test]
    fn withdraw_route_ordering_tie_breaker() {
        let principals = vec![(2u32, 100), (1u32, 100)];
        let route = build_withdraw_route(&principals, 1).unwrap();
        assert_eq!(route.entries[0].target_id, 1);
        assert_eq!(route.entries[1].target_id, 2);

        let market_data = vec![(2u32, 100, 50), (1u32, 10, 50)];
        let route = build_withdraw_route_with_liquidity(&market_data, 1).unwrap();
        assert_eq!(route.entries[0].target_id, 1);
        assert_eq!(route.entries[1].target_id, 2);
    }
}

#[cfg(kani)]
mod proofs {
    use kani::assume;
    use templar_curator_primitives::policy::withdraw_route::{
        build_withdraw_route, build_withdraw_route_with_liquidity,
    };
    use templar_curator_primitives::recovery::compute_settlement_shares;

    #[kani::proof]
    fn kani_settlement_conserves_escrow() {
        let escrow: u128 = kani::any();
        let expected: u128 = kani::any();
        let collected: u128 = kani::any();

        let settlement = compute_settlement_shares(escrow, expected, collected);

        assert!(settlement.to_burn.saturating_add(settlement.refund) == escrow);
        assert!(settlement.to_burn <= escrow);
        assert!(settlement.refund <= escrow);
    }

    #[kani::proof]
    fn kani_withdraw_route_ordering_principal_then_id() {
        let id_a: u32 = kani::any();
        let id_b: u32 = kani::any();
        assume(id_a != id_b);

        let p_a: u128 = kani::any();
        let p_b: u128 = kani::any();
        assume(p_a > 0);
        assume(p_b > 0);

        let principals = vec![(id_a, p_a), (id_b, p_b)];
        let route = build_withdraw_route(&principals, 1).unwrap();

        let first = &route.entries[0];
        let second = &route.entries[1];

        if p_a == p_b {
            let min_id = if id_a < id_b { id_a } else { id_b };
            assert!(first.target_id == min_id);
        } else {
            assert!(first.max_amount >= second.max_amount);
        }
    }

    #[kani::proof]
    fn kani_withdraw_route_ordering_liquidity_then_id() {
        let id_a: u32 = kani::any();
        let id_b: u32 = kani::any();
        assume(id_a != id_b);

        let p_a: u128 = kani::any();
        let p_b: u128 = kani::any();
        let l_a: u128 = kani::any();
        let l_b: u128 = kani::any();
        assume(p_a > 0);
        assume(p_b > 0);
        assume(l_a > 0);
        assume(l_b > 0);

        let market_data = vec![(id_a, p_a, l_a), (id_b, p_b, l_b)];
        let route = build_withdraw_route_with_liquidity(&market_data, 1).unwrap();

        let first = &route.entries[0];
        let second = &route.entries[1];
        let first_liq = first.available_liquidity.unwrap_or(0);
        let second_liq = second.available_liquidity.unwrap_or(0);

        if l_a == l_b {
            let min_id = if id_a < id_b { id_a } else { id_b };
            assert!(first.target_id == min_id);
        } else {
            assert!(first_liq >= second_liq);
        }
    }
}
