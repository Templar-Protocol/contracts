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
    use templar_curator_primitives::policy::cap_group::{
        validate_allocations, CapGroup, CapGroupError, CapGroupId, CapGroupRecord,
    };
    use templar_curator_primitives::policy::cooldown::Cooldown;
    use templar_curator_primitives::policy::market_lock::{MarketLock, MarketLockSet};
    use templar_curator_primitives::policy::refresh_plan::{
        build_refresh_plan, build_targeted_refresh_plan, RefreshPlan, RefreshPlanError,
    };
    use templar_curator_primitives::policy::state::{MarketConfig, PolicyState};
    use templar_curator_primitives::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
    use templar_curator_primitives::policy::withdraw_route::{
        build_withdraw_route, build_withdraw_route_with_liquidity,
    };
    use templar_curator_primitives::recovery::compute_settlement_shares;
    use templar_vault_kernel::Wad;

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

    #[kani::proof]
    fn kani_cooldown_unlimited_ready() {
        let now: u64 = kani::any();
        let cooldown = Cooldown::unlimited();
        assert!(cooldown.is_ready(now));
        assert!(cooldown.ready_at().is_none());
    }

    #[kani::proof]
    fn kani_cooldown_first_operation_ready() {
        let interval: u64 = kani::any();
        let now: u64 = kani::any();

        let cooldown = Cooldown::new(interval);
        assert!(cooldown.is_ready(now));
    }

    #[kani::proof]
    fn kani_cooldown_ready_at_and_remaining() {
        let interval: u64 = kani::any();
        let last: u64 = kani::any();
        let now: u64 = kani::any();

        assume(interval > 0);

        let cooldown = Cooldown::with_last_event(interval, last);
        let ready_at = cooldown.ready_at().unwrap();
        assert_eq!(ready_at, last.saturating_add(interval));

        let remaining = cooldown.remaining(now);
        if cooldown.is_ready(now) {
            assert_eq!(remaining, 0);
        } else {
            assert_eq!(remaining, ready_at.saturating_sub(now));
        }
    }

    #[kani::proof]
    fn kani_cap_group_unlimited_effective_cap() {
        let total_assets: u128 = kani::any();
        let cap = CapGroup::new();
        assert!(cap.is_unlimited());
        assert_eq!(cap.effective_cap(total_assets), u128::MAX);
        assert_eq!(cap.available_capacity(0, total_assets), u128::MAX);
    }

    #[kani::proof]
    fn kani_cap_group_available_capacity_matches_effective_cap() {
        let absolute: u128 = kani::any();
        let relative_raw: u128 = kani::any();
        let current: u128 = kani::any();
        let total: u128 = kani::any();

        assume(relative_raw <= Wad::SCALE);

        let cap = CapGroup::new()
            .with_absolute(absolute)
            .with_relative(Wad::from(relative_raw));
        let effective = cap.effective_cap(total);
        let available = cap.available_capacity(current, total);

        if cap.is_unlimited() {
            assert_eq!(available, u128::MAX);
        } else {
            assert_eq!(available, effective.saturating_sub(current));
        }
    }

    #[kani::proof]
    fn kani_cap_group_can_allocate_matches_effective_cap() {
        let absolute: u128 = kani::any();
        let relative_raw: u128 = kani::any();
        let current: u128 = kani::any();
        let amount: u128 = kani::any();
        let total: u128 = kani::any();

        assume(relative_raw <= Wad::SCALE);

        let cap = CapGroup::new()
            .with_absolute(absolute)
            .with_relative(Wad::from(relative_raw));
        let effective = cap.effective_cap(total);
        let new_principal = current.saturating_add(amount);
        let expected = if cap.is_unlimited() {
            true
        } else {
            new_principal <= effective
        };
        assert_eq!(cap.can_allocate(current, amount, total), expected);
    }

    #[kani::proof]
    fn kani_cap_group_enforce_absolute_cap() {
        let absolute: u128 = kani::any();
        let current: u128 = kani::any();
        let amount: u128 = kani::any();

        assume(absolute > 0);

        let cap = CapGroup::absolute_only(absolute);
        let result = cap.enforce(current, amount, 1000);
        let new_principal = current.saturating_add(amount);

        if new_principal > absolute {
            assert!(matches!(
                result,
                Err(CapGroupError::ExceedsAbsoluteCap { .. })
            ));
        } else {
            assert!(result.is_ok());
        }
    }

    #[kani::proof]
    fn kani_cap_group_record_apply_remove_allocation() {
        let cap = CapGroup::absolute_only(1000);
        let record = CapGroupRecord::with_principal(cap, 200);
        let updated = record.apply_allocation(100);
        assert_eq!(updated.principal, 300);

        let reduced = updated.remove_allocation(500);
        assert_eq!(reduced.principal, 0);
    }

    #[kani::proof]
    fn kani_validate_allocations_respects_caps() {
        let cap = CapGroup::absolute_only(500);
        let record = CapGroupRecord::with_principal(cap, 100);
        let allocations = vec![(record.clone(), 200u128)];
        assert!(validate_allocations(&allocations, 1000).is_ok());

        let bad = vec![(record, 1000u128)];
        assert!(validate_allocations(&bad, 1000).is_err());
    }

    #[kani::proof]
    fn kani_supply_queue_total_matches_sum() {
        let queue = SupplyQueue::new();
        let queue = queue.enqueue(SupplyQueueEntry::new(1, 10)).unwrap();
        let queue = queue.enqueue(SupplyQueueEntry::new(2, 20)).unwrap();
        assert_eq!(queue.total(), 30);
    }

    #[kani::proof]
    fn kani_supply_queue_priority_ordering() {
        let queue = SupplyQueue::new();
        let low = SupplyQueueEntry::new(1, 10).with_priority(1);
        let high = SupplyQueueEntry::new(2, 10).with_priority(9);
        let queue = queue.enqueue(low.clone()).unwrap();
        let queue = queue.enqueue(high.clone()).unwrap();

        assert_eq!(queue.entries[0].target_id, high.target_id);
        assert_eq!(queue.entries[1].target_id, low.target_id);
    }

    #[kani::proof]
    fn kani_supply_queue_fifo_within_priority() {
        let queue = SupplyQueue::new();
        let first = SupplyQueueEntry::new(1, 10).with_priority(5);
        let second = SupplyQueueEntry::new(2, 10).with_priority(5);
        let queue = queue.enqueue(first.clone()).unwrap();
        let queue = queue.enqueue(second.clone()).unwrap();

        assert_eq!(queue.entries[0].target_id, first.target_id);
        assert_eq!(queue.entries[1].target_id, second.target_id);
    }

    #[kani::proof]
    fn kani_supply_queue_dequeue_decreases_len() {
        let queue = SupplyQueue::new();
        let queue = queue.enqueue(SupplyQueueEntry::new(1, 10)).unwrap();
        let len_before = queue.len();
        let (queue, _) = queue.dequeue().unwrap();
        assert_eq!(queue.len(), len_before - 1);
    }

    #[kani::proof]
    fn kani_supply_queue_totals_by_target() {
        let queue = SupplyQueue::new();
        let queue = queue.enqueue(SupplyQueueEntry::new(1, 10)).unwrap();
        let queue = queue.enqueue(SupplyQueueEntry::new(1, 20)).unwrap();
        let queue = queue.enqueue(SupplyQueueEntry::new(2, 5)).unwrap();

        let totals = queue.totals_by_target();
        let mut total_1 = 0u128;
        let mut total_2 = 0u128;
        for (id, amount) in totals {
            if id == 1 {
                total_1 = amount;
            } else if id == 2 {
                total_2 = amount;
            }
        }
        assert_eq!(total_1, 30);
        assert_eq!(total_2, 5);
    }

    #[kani::proof]
    fn kani_supply_queue_remove_target() {
        let queue = SupplyQueue::new();
        let queue = queue.enqueue(SupplyQueueEntry::new(1, 10)).unwrap();
        let queue = queue.enqueue(SupplyQueueEntry::new(2, 10)).unwrap();
        let removed = queue.remove_target(1);
        assert!(!removed.entries.iter().any(|e| e.target_id == 1));
    }

    #[kani::proof]
    fn kani_market_lock_expiry_clears_active() {
        let now: u64 = kani::any();
        let expiry: u64 = kani::any();

        assume(expiry <= now);

        let lock = MarketLock::new(1, 0).with_expiry(expiry);
        assert!(lock.is_expired(now));

        let set = MarketLockSet { locks: vec![lock] };
        assert!(!set.is_locked(1, now));
    }

    #[kani::proof]
    fn kani_market_lock_acquire_conflict() {
        let now: u64 = kani::any();
        let lock = MarketLock::new(1, now);
        let set = MarketLockSet::new().acquire(lock.clone(), now).unwrap();
        let result = set.acquire(lock, now);
        assert!(result.is_err());
    }

    #[kani::proof]
    fn kani_market_lock_release_removes() {
        let now: u64 = kani::any();
        let lock = MarketLock::new(1, now);
        let set = MarketLockSet::new().acquire(lock, now).unwrap();
        let released = set.release(1);
        assert!(!released.is_locked(1, now));
    }

    #[kani::proof]
    fn kani_market_lock_active_count_le_len() {
        let now: u64 = kani::any();
        let lock1 = MarketLock::new(1, now);
        let lock2 = MarketLock::new(2, now);
        let set = MarketLockSet {
            locks: vec![lock1, lock2],
        };
        assert!(set.active_count(now) <= set.len());
    }

    #[kani::proof]
    fn kani_refresh_plan_validate_empty_rejects() {
        let plan = RefreshPlan::empty();
        assert!(matches!(plan.validate(), Err(RefreshPlanError::EmptyPlan)));
    }

    #[kani::proof]
    fn kani_refresh_plan_validate_duplicate_rejects() {
        let plan = RefreshPlan::new(vec![1u32, 1u32]);
        assert!(matches!(
            plan.validate(),
            Err(RefreshPlanError::DuplicateTarget { .. })
        ));
    }

    #[kani::proof]
    fn kani_build_refresh_plan_empty_rejects() {
        let result = build_refresh_plan(&[], None);
        assert!(matches!(result, Err(RefreshPlanError::EmptyPlan)));
    }

    #[kani::proof]
    fn kani_build_targeted_refresh_plan_invalid_target_rejects() {
        let result = build_targeted_refresh_plan(&[2u32], &[1u32]);
        assert!(matches!(
            result,
            Err(RefreshPlanError::TargetNotFound { target_id: 2 })
        ));
    }

    #[kani::proof]
    fn kani_refresh_plan_is_ready_matches_cooldown() {
        let cooldown_ns: u64 = kani::any();
        let last_refresh: u64 = kani::any();
        let now: u64 = kani::any();

        let plan = RefreshPlan::new(vec![1u32])
            .with_cooldown(cooldown_ns)
            .with_last_refresh(last_refresh);
        assert_eq!(plan.is_ready(now), plan.cooldown.is_ready(now));
    }

    #[kani::proof]
    fn kani_policy_state_external_assets_sums_principals() {
        let a: u128 = kani::any();
        let b: u128 = kani::any();

        assume(a <= u128::MAX - b);

        let mut state = PolicyState::new();
        state.set_principal(1, a);
        state.set_principal(2, b);
        assert_eq!(state.external_assets(), a + b);
    }

    #[kani::proof]
    fn kani_policy_state_cap_group_totals() {
        let group_a = CapGroupId::new("group-a");
        let group_b = CapGroupId::new("group-b");

        let mut state = PolicyState::new();
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

    #[kani::proof]
    fn kani_policy_state_refresh_cap_group_principals_updates() {
        let group = CapGroupId::new("group");
        let mut state = PolicyState::new();
        state
            .cap_groups
            .insert(group.clone(), CapGroupRecord::default());
        state.set_market_config(1, MarketConfig::new(true, Some(group.clone())));
        state.set_principal(1, 123);

        state.refresh_cap_group_principals();
        let record = state.cap_groups.get(&group).expect("cap group");
        assert_eq!(record.principal, 123);
    }

    #[kani::proof]
    fn kani_build_withdraw_route_is_valid() {
        let principals = vec![(1u32, 100u128), (2u32, 50u128)];
        let route = build_withdraw_route(&principals, 100).unwrap();
        assert!(route.validate().is_ok());
    }

    #[kani::proof]
    fn kani_build_withdraw_route_with_liquidity_is_valid() {
        let market_data = vec![(1u32, 100u128, 80u128), (2u32, 50u128, 40u128)];
        let route = build_withdraw_route_with_liquidity(&market_data, 80).unwrap();
        assert!(route.validate().is_ok());
    }
}
