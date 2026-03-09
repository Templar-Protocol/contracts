use proptest::prelude::*;

use templar_curator_primitives::policy::withdraw_route::{
    build_withdraw_route, build_withdraw_route_with_liquidity,
};
#[cfg(feature = "recovery")]
use templar_curator_primitives::recovery::compute_settlement_shares;

fn assert_route_order_by_principal(route: &[(u32, u128)]) {
    for window in route.windows(2) {
        let a = window[0];
        let b = window[1];
        if a.1 == b.1 {
            assert!(a.0 <= b.0);
        } else {
            assert!(a.1 >= b.1);
        }
    }
}

#[cfg(feature = "recovery")]
proptest! {
    #[test]
    fn prop_compute_settlement_shares_conserves_escrow(
        escrow in 0u64..1_000_000_000,
        expected in 0u64..1_000_000_000,
        collected in 0u64..1_000_000_000,
    ) {
        let settlement = compute_settlement_shares(
            escrow as u128,
            expected as u128,
            collected as u128,
        );

        prop_assert_eq!(
            settlement.to_burn.saturating_add(settlement.refund),
            escrow as u128
        );
        prop_assert!(settlement.to_burn <= escrow as u128);
        prop_assert!(settlement.refund <= escrow as u128);

        if expected == 0 || escrow == 0 {
            prop_assert_eq!(settlement.to_burn, 0);
            prop_assert_eq!(settlement.refund, escrow as u128);
        }

        if collected >= expected && expected > 0 {
            prop_assert_eq!(settlement.to_burn, escrow as u128);
            prop_assert_eq!(settlement.refund, 0);
        }
    }
}

proptest! {
    #[test]
    fn prop_build_withdraw_route_is_valid(
        data in prop::collection::vec((1u32..200, 1u64..1_000_000), 1..20),
        target_amount in 1u64..1_000_000,
    ) {
        let mut principals: Vec<(u32, u128)> = Vec::new();
        for (id, principal) in data {
            if principals.iter().all(|(existing, _)| *existing != id) {
                principals.push((id, principal as u128));
            }
        }

        prop_assume!(!principals.is_empty());
        let total: u128 = principals.iter().fold(0u128, |acc, (_, p)| acc.saturating_add(*p));
        prop_assume!(total >= target_amount as u128);

        let route = build_withdraw_route(&principals, target_amount as u128).unwrap();
        prop_assert!(route.validate().is_ok());

        let ordered: Vec<(u32, u128)> = route
            .entries
            .iter()
            .map(|e| (e.target_id, e.max_amount))
            .collect();
        assert_route_order_by_principal(&ordered);
    }

    #[test]
    fn prop_build_withdraw_route_with_liquidity_is_valid(
        data in prop::collection::vec((1u32..200, 1u64..1_000_000, 1u64..1_000_000), 1..20),
        target_amount in 1u64..1_000_000,
    ) {
        let mut market_data: Vec<(u32, u128, u128)> = Vec::new();
        for (id, principal, liquidity) in data {
            if market_data.iter().all(|(existing, _, _)| *existing != id) {
                market_data.push((id, principal as u128, liquidity as u128));
            }
        }

        prop_assume!(!market_data.is_empty());
        let total: u128 = market_data
            .iter()
            .fold(0u128, |acc, (_, p, l)| acc.saturating_add((*p).min(*l)));
        prop_assume!(total >= target_amount as u128);

        let route = build_withdraw_route_with_liquidity(&market_data, target_amount as u128)
            .unwrap();
        prop_assert!(route.validate().is_ok());

        for window in route.entries.windows(2) {
            let a = &window[0];
            let b = &window[1];
            let a_liq = a.available_liquidity.expect("route entry should carry liquidity metadata");
            let b_liq = b.available_liquidity.expect("route entry should carry liquidity metadata");
            if a_liq == b_liq {
                prop_assert!(a.target_id <= b.target_id);
            } else {
                prop_assert!(a_liq >= b_liq);
            }
        }
    }
}
