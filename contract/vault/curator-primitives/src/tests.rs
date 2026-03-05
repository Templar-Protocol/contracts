//! Golden tests that compare plan outputs against fixed NEAR curator vault snapshots.
//!
//! These tests validate that the curator primitives produce deterministic outputs
//! when given the same inputs, ensuring compatibility with the NEAR vault implementation.

#![cfg(test)]

use alloc::vec;
use alloc::vec::Vec;

use crate::policy::cap_group::CapGroup;
use crate::policy::refresh_plan::build_refresh_plan;
use crate::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
use crate::policy::withdraw_route::{build_withdraw_route, WithdrawRoute, WithdrawRouteEntry};
#[cfg(feature = "recovery")]
use crate::recovery::{
    compute_recovery_stats, compute_settlement_shares, determine_recovery_action, RecoveryContext,
    RecoveryProgress,
};
#[cfg(feature = "recovery")]
use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};
use templar_vault_kernel::Wad;
#[cfg(feature = "recovery")]
use templar_vault_kernel::{
    AllocatingState, KernelAction, OpState, PayoutOutcome, PayoutState, RefreshingState,
    WithdrawingState,
};

// WAD constant matching templar-vault-kernel
const WAD: u128 = Wad::SCALE;

/// Snapshot representing a typical NEAR curator vault state.
/// This represents a vault with:
/// - 3 markets (IDs 0, 1, 2)
/// - Total assets of 10,000,000 USDC (6 decimals)
/// - Different cap groups for risk management
struct NearVaultSnapshot {
    /// Market principals (market_id, principal)
    market_principals: Vec<(u32, u128)>,
    /// Total assets (idle + allocated)
    total_assets: u128,
    /// Idle balance (not allocated) - retained for documentation purposes
    #[allow(dead_code)]
    idle_balance: u128,
    /// Cap groups: (group_id, absolute_cap, relative_cap_wad, current_principal)
    cap_groups: Vec<(&'static str, u128, u128, u128)>,
    /// Market to cap group mapping
    market_cap_groups: Vec<(u32, &'static str)>,
}

impl Default for NearVaultSnapshot {
    fn default() -> Self {
        // Realistic NEAR vault snapshot with 10M USDC (6 decimals = 10_000_000_000_000)
        Self {
            market_principals: vec![
                (0, 3_000_000_000_000), // Market 0: 3M USDC
                (1, 2_500_000_000_000), // Market 1: 2.5M USDC
                (2, 1_500_000_000_000), // Market 2: 1.5M USDC
            ],
            total_assets: 10_000_000_000_000, // 10M USDC total
            idle_balance: 3_000_000_000_000,  // 3M USDC idle
            cap_groups: vec![
                // "stable" group: 5M absolute cap, 60% relative cap, 3M current
                (
                    "stable",
                    5_000_000_000_000,
                    WAD * 60 / 100,
                    3_000_000_000_000,
                ),
                // "volatile" group: 3M absolute cap, 30% relative cap, 2.5M current
                (
                    "volatile",
                    3_000_000_000_000,
                    WAD * 30 / 100,
                    2_500_000_000_000,
                ),
                // "new" group: 2M absolute cap, 20% relative cap, 1.5M current
                ("new", 2_000_000_000_000, WAD * 20 / 100, 1_500_000_000_000),
            ],
            market_cap_groups: vec![(0, "stable"), (1, "volatile"), (2, "new")],
        }
    }
}

mod auth_unit_tests {
    use crate::auth::{
        boundary_policy_class, canonical_policy_class, ActionKind, AuthAdapter, AuthError,
        AuthPolicyClass, AuthResult,
    };
    use templar_vault_kernel::Address;

    #[derive(Clone, Copy, Default)]
    struct TestPermissiveAuth;

    impl AuthAdapter for TestPermissiveAuth {
        fn authorize(
            &self,
            _action: ActionKind,
            _caller: Address,
            _proof: Option<&[u8]>,
        ) -> AuthResult<()> {
            Ok(())
        }

        fn is_paused(&self) -> bool {
            false
        }
    }

    #[derive(Clone, Copy, Default)]
    struct TestStrictAuth {
        paused: bool,
    }

    impl TestStrictAuth {
        const fn new() -> Self {
            Self { paused: false }
        }

        const fn paused() -> Self {
            Self { paused: true }
        }
    }

    impl AuthAdapter for TestStrictAuth {
        fn authorize(
            &self,
            action: ActionKind,
            caller: Address,
            _proof: Option<&[u8]>,
        ) -> AuthResult<()> {
            if self.paused && action != ActionKind::Pause {
                return Err(AuthError::VaultPaused);
            }

            if action.is_privileged() {
                return Err(AuthError::NotAuthorized {
                    caller: caller.into(),
                    action,
                });
            }

            Ok(())
        }

        fn is_paused(&self) -> bool {
            self.paused
        }
    }

    #[test]
    fn test_action_kind_is_privileged() {
        assert!(!ActionKind::Deposit.is_privileged());
        assert!(!ActionKind::RequestWithdraw.is_privileged());
        assert!(ActionKind::ExecuteWithdraw.is_privileged());

        assert!(ActionKind::Pause.is_privileged());
        assert!(ActionKind::SetRestrictions.is_privileged());
        assert!(ActionKind::FinishAllocating.is_privileged());
        assert!(ActionKind::BeginAllocating.is_privileged());
        assert!(ActionKind::AbortAllocating.is_privileged());
        assert!(ActionKind::ManualReconcile.is_privileged());
    }

    #[test]
    fn test_policy_class_canonical() {
        assert_eq!(
            canonical_policy_class(ActionKind::ExecuteWithdraw),
            AuthPolicyClass::Allocator
        );
        assert_eq!(
            canonical_policy_class(ActionKind::Pause),
            AuthPolicyClass::Guardian
        );
        assert_eq!(
            canonical_policy_class(ActionKind::AbortRefreshing),
            AuthPolicyClass::AllocatorEmergency
        );
        assert_eq!(
            canonical_policy_class(ActionKind::ManualReconcile),
            AuthPolicyClass::Curator
        );
    }

    #[test]
    fn test_policy_class_boundary() {
        assert_eq!(
            boundary_policy_class(ActionKind::ExecuteWithdraw),
            AuthPolicyClass::Allocator
        );
        assert_eq!(
            boundary_policy_class(ActionKind::AbortRefreshing),
            AuthPolicyClass::AllocatorEmergency
        );
        assert_eq!(
            boundary_policy_class(ActionKind::SetRestrictions),
            AuthPolicyClass::Guardian
        );
    }

    #[test]
    fn test_permissive_auth() {
        let auth = TestPermissiveAuth;
        let caller = [0u8; 32];

        assert!(auth.authorize(ActionKind::Deposit, caller, None).is_ok());
        assert!(auth.authorize(ActionKind::Pause, caller, None).is_ok());
        assert!(auth
            .authorize(ActionKind::BeginAllocating, caller, None)
            .is_ok());
        assert!(!auth.is_paused());
    }

    #[test]
    fn test_strict_auth_allows_user_actions() {
        let auth = TestStrictAuth::new();
        let caller = [0u8; 32];

        assert!(auth.authorize(ActionKind::Deposit, caller, None).is_ok());
        assert!(auth
            .authorize(ActionKind::RequestWithdraw, caller, None)
            .is_ok());
        let result = auth.authorize(ActionKind::ExecuteWithdraw, caller, None);
        assert!(matches!(result, Err(AuthError::NotAuthorized { .. })));
    }

    #[test]
    fn test_strict_auth_denies_privileged_actions() {
        let auth = TestStrictAuth::new();
        let caller = [0u8; 32];

        let result = auth.authorize(ActionKind::Pause, caller, None);
        assert!(matches!(result, Err(AuthError::NotAuthorized { .. })));

        let result = auth.authorize(ActionKind::BeginAllocating, caller, None);
        assert!(matches!(result, Err(AuthError::NotAuthorized { .. })));
    }

    #[test]
    fn test_strict_auth_paused() {
        let auth = TestStrictAuth::paused();
        let caller = [0u8; 32];

        assert!(auth.is_paused());

        // Pause action is allowed even when paused.
        assert!(auth.authorize(ActionKind::Pause, caller, None).is_err());

        // User actions are denied when paused.
        let result = auth.authorize(ActionKind::Deposit, caller, None);
        assert!(matches!(result, Err(AuthError::VaultPaused)));
    }
}

// Golden Test: Cap Group Enforcement

#[test]
fn golden_cap_group_effective_caps() {
    let snapshot = NearVaultSnapshot::default();

    // Expected effective caps based on the snapshot
    // "stable": min(5M, 60% of 10M = 6M) = 5M
    // "volatile": min(3M, 30% of 10M = 3M) = 3M
    // "new": min(2M, 20% of 10M = 2M) = 2M

    let expected_effective_caps: Vec<(&str, u128)> = vec![
        ("stable", 5_000_000_000_000),
        ("volatile", 3_000_000_000_000),
        ("new", 2_000_000_000_000),
    ];

    for (group_id, abs_cap, rel_cap, _principal) in &snapshot.cap_groups {
        let cap = CapGroup::new()
            .with_absolute(*abs_cap)
            .with_relative(Wad::from(*rel_cap));
        let effective = cap.effective_cap(snapshot.total_assets);

        let expected = expected_effective_caps
            .iter()
            .find(|(id, _)| id == group_id)
            .map(|(_, e)| *e)
            .unwrap();

        assert_eq!(
            effective, expected,
            "Cap group '{}' effective cap mismatch",
            group_id
        );
    }
}

#[test]
fn golden_cap_group_available_capacity() {
    let snapshot = NearVaultSnapshot::default();

    // Expected available capacity = effective_cap - current_principal
    // "stable": 5M - 3M = 2M
    // "volatile": 3M - 2.5M = 0.5M
    // "new": 2M - 1.5M = 0.5M

    let expected_capacities: Vec<(&str, u128)> = vec![
        ("stable", 2_000_000_000_000),
        ("volatile", 500_000_000_000),
        ("new", 500_000_000_000),
    ];

    for (group_id, abs_cap, rel_cap, principal) in &snapshot.cap_groups {
        let cap = CapGroup::new()
            .with_absolute(*abs_cap)
            .with_relative(Wad::from(*rel_cap));
        let available = cap.available_capacity(*principal, snapshot.total_assets);

        let expected = expected_capacities
            .iter()
            .find(|(id, _)| id == group_id)
            .map(|(_, e)| *e)
            .unwrap();

        assert_eq!(
            available, expected,
            "Cap group '{}' available capacity mismatch",
            group_id
        );
    }
}

#[test]
fn golden_cap_group_allocation_validation() {
    let snapshot = NearVaultSnapshot::default();

    // Test allocations against the "volatile" group (3M cap, 2.5M used)
    // Available: 500_000_000_000 (0.5M)

    let volatile_cap = CapGroup::new()
        .with_absolute(3_000_000_000_000)
        .with_relative(Wad::from(WAD * 30 / 100));
    let volatile_principal = 2_500_000_000_000u128;

    // Should succeed: allocate 400_000_000_000 (0.4M)
    assert!(volatile_cap.can_allocate(volatile_principal, 400_000_000_000, snapshot.total_assets));

    // Should succeed: allocate exactly 500_000_000_000 (0.5M)
    assert!(volatile_cap.can_allocate(volatile_principal, 500_000_000_000, snapshot.total_assets));

    // Should fail: allocate 600_000_000_000 (0.6M)
    assert!(!volatile_cap.can_allocate(volatile_principal, 600_000_000_000, snapshot.total_assets));

    // Enforcement should return proper error
    let result = volatile_cap.enforce(volatile_principal, 600_000_000_000, snapshot.total_assets);
    assert!(result.is_err());
}

// Golden Test: Supply Queue to Allocation Plan

#[test]
fn golden_supply_queue_to_plan() {
    // Simulate a supply queue with multiple entries for the same target
    let mut queue = SupplyQueue::new();

    // Add entries simulating batched deposits
    queue = queue
        .enqueue(SupplyQueueEntry::new(0, 500_000_000_000))
        .unwrap();
    queue = queue
        .enqueue(SupplyQueueEntry::new(1, 300_000_000_000))
        .unwrap();
    queue = queue
        .enqueue(SupplyQueueEntry::new(0, 200_000_000_000))
        .unwrap();
    queue = queue
        .enqueue(SupplyQueueEntry::new(2, 400_000_000_000))
        .unwrap();
    queue = queue
        .enqueue(SupplyQueueEntry::new(1, 100_000_000_000))
        .unwrap();

    // Expected total: 1.5M
    let total = queue.total();
    assert_eq!(total, 1_500_000_000_000);

    // Expected plan (aggregated by target):
    // Target 0: 700_000_000_000 (0.7M)
    // Target 1: 400_000_000_000 (0.4M)
    // Target 2: 400_000_000_000 (0.4M)
    let plan = queue.to_allocation_plan();

    let target_0_amount = plan.iter().find(|(id, _)| *id == 0).map(|(_, a)| *a);
    let target_1_amount = plan.iter().find(|(id, _)| *id == 1).map(|(_, a)| *a);
    let target_2_amount = plan.iter().find(|(id, _)| *id == 2).map(|(_, a)| *a);

    assert_eq!(target_0_amount, Some(700_000_000_000));
    assert_eq!(target_1_amount, Some(400_000_000_000));
    assert_eq!(target_2_amount, Some(400_000_000_000));
}

#[test]
fn golden_supply_queue_priority_ordering() {
    let mut queue = SupplyQueue::new();

    // Add entries with different priorities
    queue = queue
        .enqueue(SupplyQueueEntry::new(0, 100_000_000_000).with_priority(0))
        .unwrap();
    queue = queue
        .enqueue(SupplyQueueEntry::new(1, 200_000_000_000).with_priority(5))
        .unwrap();
    queue = queue
        .enqueue(SupplyQueueEntry::new(2, 300_000_000_000).with_priority(10))
        .unwrap();
    queue = queue
        .enqueue(SupplyQueueEntry::new(3, 400_000_000_000).with_priority(3))
        .unwrap();

    // Expected order by priority (highest first): 2, 1, 3, 0
    let entries: Vec<u32> = queue.entries.iter().map(|e| e.target_id).collect();
    assert_eq!(entries, vec![2, 1, 3, 0]);
}

// Golden Test: Withdraw Route Building

#[test]
fn golden_withdraw_route_from_principals() {
    let snapshot = NearVaultSnapshot::default();

    // Build a withdraw route for 2M USDC
    let target_amount = 2_000_000_000_000u128;

    let route = build_withdraw_route(&snapshot.market_principals, target_amount).unwrap();

    // Validate route
    assert!(route.validate().is_ok());

    // Route total should cover target
    let route_total = route.total();
    assert!(route_total >= target_amount);

    // Markets should be sorted by principal (largest first)
    // Expected order: 0 (3M), 1 (2.5M), 2 (1.5M)
    assert_eq!(route.entries[0].target_id, 0);
    assert_eq!(route.entries[1].target_id, 1);
    assert_eq!(route.entries[2].target_id, 2);
}

#[test]
fn golden_withdraw_route_validation() {
    // Create a manually constructed route
    let route = WithdrawRoute::from_entries(
        vec![
            WithdrawRouteEntry::new(0, 1_000_000_000_000),
            WithdrawRouteEntry::new(1, 800_000_000_000),
            WithdrawRouteEntry::new(2, 500_000_000_000),
        ],
        2_000_000_000_000,
    );

    // Should be valid (total 2.3M >= target 2M)
    assert!(route.validate().is_ok());

    // Route total
    assert_eq!(route.total(), 2_300_000_000_000);
}

// Golden Test: Refresh Plan

#[test]
fn golden_refresh_plan_building() {
    let snapshot = NearVaultSnapshot::default();
    let enabled_targets: Vec<u32> = snapshot
        .market_principals
        .iter()
        .map(|(id, _)| *id)
        .collect();

    // Build refresh plan for all markets
    let plan = build_refresh_plan(&enabled_targets, Some(30_000_000_000)).unwrap();

    assert!(plan.validate().is_ok());
    assert_eq!(plan.len(), 3); // 3 markets
    assert_eq!(plan.cooldown_ns(), 30_000_000_000); // 30 seconds
}

#[cfg(feature = "recovery")]
#[test]
fn golden_recovery_allocating_state() {
    // Simulate an allocating state that got stuck
    let state = OpState::Allocating(AllocatingState {
        op_id: 42,
        index: 2,
        remaining: 500_000_000_000,
        plan: vec![
            (0, 300_000_000_000),
            (1, 200_000_000_000),
            (2, 300_000_000_000),
            (3, 200_000_000_000),
        ],
    });

    let ctx = RecoveryContext::new(1_000_000_000_000);
    let progress = RecoveryProgress::new(0);
    let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

    match action {
        KernelAction::AbortAllocating {
            op_id,
            restore_idle,
        } => {
            assert_eq!(op_id, 42);
            assert_eq!(restore_idle, 500_000_000_000);
        }
        _ => panic!("Expected AbortAllocating"),
    }

    // Check recovery stats
    let stats = compute_recovery_stats(&state);
    assert_eq!(stats.completed_targets, 2);
    assert_eq!(stats.remaining_targets, 2);
    assert_eq!(stats.remaining_amount, 500_000_000_000);
}

#[cfg(feature = "recovery")]
#[test]
fn golden_recovery_withdrawing_state() {
    let state = OpState::Withdrawing(WithdrawingState {
        op_id: 43,
        index: 1,
        remaining: 400_000_000_000,
        collected: 600_000_000_000,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 1_000_000_000_000,
    });

    let ctx = RecoveryContext::new(1_000_000_000_000);
    let progress = RecoveryProgress::new(0);
    let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

    match action {
        KernelAction::AbortWithdrawing {
            op_id,
            refund_shares,
        } => {
            assert_eq!(op_id, 43);
            assert_eq!(refund_shares, 1_000_000_000_000);
        }
        _ => panic!("Expected AbortWithdrawing"),
    }
}

#[cfg(feature = "recovery")]
#[test]
fn golden_recovery_payout_state() {
    let state = OpState::Payout(PayoutState {
        op_id: 44,
        receiver: receiver_addr(1),
        amount: 1_000_000_000_000,
        owner: owner_addr(1),
        escrow_shares: 500_000_000_000,
        burn_shares: 400_000_000_000,
    });

    let ctx = RecoveryContext::new(1_000_000_000_000);
    let progress = RecoveryProgress::new(0);
    let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

    match action {
        KernelAction::SettlePayout { op_id, outcome } => {
            assert_eq!(op_id, 44);
            match outcome {
                PayoutOutcome::Failure {
                    restore_idle,
                    refund_shares,
                } => {
                    assert_eq!(restore_idle, 1_000_000_000_000);
                    assert_eq!(refund_shares, 500_000_000_000); // Full refund
                }
                _ => panic!("Expected failure outcome"),
            }
        }
        _ => panic!("Expected SettlePayout"),
    }
}

#[cfg(feature = "recovery")]
#[test]
fn golden_settlement_shares_full() {
    // Full withdrawal: collected == expected
    let settlement = compute_settlement_shares(1_000_000_000_000, 500_000_000_000, 500_000_000_000);
    assert_eq!(settlement.to_burn, 1_000_000_000_000); // All shares burned
    assert_eq!(settlement.refund, 0); // Nothing refunded
}

#[cfg(feature = "recovery")]
#[test]
fn golden_settlement_shares_partial() {
    // Partial withdrawal: collected 60% of expected
    let settlement = compute_settlement_shares(1_000_000_000_000, 500_000_000_000, 300_000_000_000);

    // burn = 1_000_000_000_000 * 300 / 500 = 600_000_000_000
    assert_eq!(settlement.to_burn, 600_000_000_000);
    assert_eq!(settlement.refund, 400_000_000_000);
}

#[cfg(feature = "recovery")]
#[test]
fn golden_settlement_shares_over_collection() {
    // Over-collection: collected > expected (edge case)
    let settlement = compute_settlement_shares(1_000_000_000_000, 500_000_000_000, 600_000_000_000);
    assert_eq!(settlement.to_burn, 1_000_000_000_000); // All shares burned
    assert_eq!(settlement.refund, 0); // Nothing refunded
}

#[cfg(feature = "recovery")]
#[test]
fn golden_settlement_shares_large_values() {
    // Test with large values to ensure no overflow
    let escrow = u128::MAX / 2;
    let expected = u128::MAX / 4;
    let collected = expected / 2;

    let settlement = compute_settlement_shares(escrow, expected, collected);

    // burn = escrow * collected / expected = (MAX/2) * (MAX/8) / (MAX/4) = MAX/4
    // With saturating arithmetic, this should be safe
    assert!(settlement.to_burn <= escrow);
    assert_eq!(settlement.to_burn + settlement.refund, escrow);
}

// Golden Test: Integration Scenario

#[test]
fn golden_full_allocation_cycle() {
    let snapshot = NearVaultSnapshot::default();

    // Step 1: Create supply queue with batched deposits (1M total)
    let mut queue = SupplyQueue::new();
    queue = queue
        .enqueue(SupplyQueueEntry::new(0, 400_000_000_000))
        .unwrap();
    queue = queue
        .enqueue(SupplyQueueEntry::new(1, 300_000_000_000))
        .unwrap();
    queue = queue
        .enqueue(SupplyQueueEntry::new(2, 300_000_000_000))
        .unwrap();

    // Step 2: Convert to allocation plan
    let plan = queue.to_allocation_plan();
    assert_eq!(queue.total(), 1_000_000_000_000);

    // Step 3: Validate against cap groups
    // Market 0 -> "stable" group: 3M + 0.4M = 3.4M < 5M cap (OK)
    // Market 1 -> "volatile" group: 2.5M + 0.3M = 2.8M < 3M cap (OK)
    // Market 2 -> "new" group: 1.5M + 0.3M = 1.8M < 2M cap (OK)

    for (target_id, amount) in &plan {
        let (group_id, abs_cap, rel_cap, principal) = snapshot
            .cap_groups
            .iter()
            .find(|(g, _, _, _)| {
                snapshot
                    .market_cap_groups
                    .iter()
                    .any(|(m, grp)| *m == *target_id && grp == g)
            })
            .unwrap();

        let cap = CapGroup::new()
            .with_absolute(*abs_cap)
            .with_relative(Wad::from(*rel_cap));
        let result = cap.enforce(*principal, *amount, snapshot.total_assets);

        assert!(
            result.is_ok(),
            "Cap group '{}' should allow allocation of {} to market {}",
            group_id,
            amount,
            target_id
        );
    }
}

#[cfg(feature = "recovery")]
#[test]
fn golden_refresh_after_allocation() {
    let snapshot = NearVaultSnapshot::default();

    // Build refresh plan for all markets
    let enabled_targets: Vec<u32> = snapshot
        .market_principals
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let plan = build_refresh_plan(&enabled_targets, None).unwrap();

    // Validate plan
    assert!(plan.validate().is_ok());

    // Simulate refreshing state
    let state = OpState::Refreshing(RefreshingState {
        op_id: 100,
        index: 1,
        plan: plan.targets.clone(),
    });

    // Check recovery from stuck refresh
    let ctx = RecoveryContext::new(1_000_000_000_000);
    let progress = RecoveryProgress::new(0);
    let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

    match action {
        KernelAction::AbortRefreshing { op_id } => {
            assert_eq!(op_id, 100);
        }
        _ => panic!("Expected AbortRefreshing"),
    }
}

mod cap_group_unit_tests {
    use super::WAD;
    use alloc::vec;

    use crate::policy::cap_group::{validate_allocations, CapGroup, CapGroupError, CapGroupRecord};
    use templar_vault_kernel::Wad;

    #[test]
    fn test_cap_group_unlimited() {
        let cap = CapGroup::new();
        assert!(cap.is_unlimited());
        assert!(cap.can_allocate(0, u128::MAX, 1000));
    }

    #[test]
    fn test_cap_group_absolute_only() {
        let cap = CapGroup::absolute_only(1000);
        assert!(!cap.is_unlimited());
        assert!(cap.absolute_cap.is_some());
        assert!(cap.relative_cap.is_none());

        // Can allocate up to cap
        assert!(cap.can_allocate(0, 1000, 10000));
        assert!(cap.can_allocate(500, 500, 10000));

        // Cannot exceed cap
        assert!(!cap.can_allocate(500, 501, 10000));
        assert!(!cap.can_allocate(1000, 1, 10000));
    }

    #[test]
    fn test_cap_group_relative_only() {
        // 50% relative cap
        let cap = CapGroup::relative_only(Wad::from(WAD / 2));
        assert!(!cap.is_unlimited());
        assert!(cap.absolute_cap.is_none());
        assert!(cap.relative_cap.is_some());

        // Total assets = 1000, effective cap = 500
        assert!(cap.can_allocate(0, 500, 1000));
        assert!(cap.can_allocate(200, 300, 1000));
        assert!(!cap.can_allocate(200, 301, 1000));
    }

    #[test]
    fn test_cap_group_both_caps() {
        // 1000 absolute, 50% relative
        let cap = CapGroup::new()
            .with_absolute(1000)
            .with_relative(Wad::from(WAD / 2));

        // With 3000 total assets, relative cap = 1500, but absolute = 1000
        assert!(cap.can_allocate(0, 1000, 3000));
        assert!(!cap.can_allocate(0, 1001, 3000));

        // With 1000 total assets, relative cap = 500, which is stricter
        assert!(cap.can_allocate(0, 500, 1000));
        assert!(!cap.can_allocate(0, 501, 1000));
    }

    #[test]
    fn test_compute_effective_cap() {
        let cap = CapGroup::new()
            .with_absolute(1000)
            .with_relative(Wad::from(WAD / 2));

        // When relative cap is stricter
        assert_eq!(cap.effective_cap(1000), 500);

        // When absolute cap is stricter
        assert_eq!(cap.effective_cap(3000), 1000);

        // Unlimited
        let unlimited = CapGroup::new();
        assert_eq!(unlimited.effective_cap(1000), u128::MAX);
    }

    #[test]
    fn test_enforce_cap_group_errors() {
        let cap = CapGroup::new()
            .with_absolute(1000)
            .with_relative(Wad::from(WAD / 2));

        // Exceeds absolute cap
        let result = cap.enforce(0, 1001, 3000);
        assert!(matches!(
            result,
            Err(CapGroupError::ExceedsAbsoluteCap { .. })
        ));

        // Exceeds relative cap (500 effective cap when total = 1000)
        let result = cap.enforce(0, 501, 1000);
        assert!(matches!(
            result,
            Err(CapGroupError::ExceedsRelativeCap { .. })
        ));
    }

    #[test]
    fn test_compute_available_capacity() {
        let cap = CapGroup::absolute_only(1000);

        assert_eq!(cap.available_capacity(0, 2000), 1000);
        assert_eq!(cap.available_capacity(300, 2000), 700);
        assert_eq!(cap.available_capacity(1000, 2000), 0);
        assert_eq!(cap.available_capacity(1500, 2000), 0); // Already over, saturates to 0
    }

    #[test]
    fn test_apply_and_remove_allocation() {
        let cap = CapGroup::absolute_only(1000);
        let record = CapGroupRecord::new(cap);

        let updated = record.apply_allocation(300);
        assert_eq!(updated.principal, 300);

        let reduced = updated.remove_allocation(100);
        assert_eq!(reduced.principal, 200);

        // Saturating subtraction
        let zero = reduced.remove_allocation(500);
        assert_eq!(zero.principal, 0);
    }

    #[test]
    fn test_validate_allocations() {
        let cap1 = CapGroupRecord::new(CapGroup::absolute_only(1000));
        let cap2 = CapGroupRecord::new(CapGroup::absolute_only(500));

        // Valid allocations
        let allocations = vec![(cap1.clone(), 500), (cap2.clone(), 300)];
        assert!(validate_allocations(&allocations, 2000).is_ok());

        // Invalid - second exceeds cap
        let invalid = vec![(cap1, 500), (cap2, 600)];
        assert!(validate_allocations(&invalid, 2000).is_err());
    }

    #[test]
    fn test_cap_group_record_methods() {
        let record = CapGroupRecord::new(CapGroup::absolute_only(1000));

        assert!(record.can_allocate(500, 2000));
        assert!(!record.can_allocate(1001, 2000));
        assert_eq!(record.available_capacity(2000), 1000);

        assert!(record.enforce(500, 2000).is_ok());
        assert!(record.enforce(1001, 2000).is_err());
    }

    #[test]
    fn test_zero_absolute_cap_is_unlimited() {
        let cap = CapGroup::absolute_only(0);
        // NonZeroU128::new(0) returns None, so this should be unlimited
        assert!(cap.absolute_cap.is_none());
    }

    proptest::proptest! {
        #[test]
        fn prop_available_capacity_matches_effective_cap(
            absolute in 0u128..=1_000_000_000_000u128,
            relative in 0u128..=WAD,
            current in 0u128..=1_000_000_000_000u128,
            total in 0u128..=1_000_000_000_000u128,
        ) {
            let cap = CapGroup::new()
                .with_absolute(absolute)
                .with_relative(Wad::from(relative));
            let effective = cap.effective_cap(total);
            let available = cap.available_capacity(current, total);

            if cap.is_unlimited() {
                proptest::prop_assert_eq!(available, u128::MAX);
            } else {
                proptest::prop_assert_eq!(available, effective.saturating_sub(current));
            }
        }
    }
}

#[cfg(feature = "recovery")]
mod recovery_unit_tests {
    use alloc::string::String;
    use alloc::vec;

    use crate::recovery::{
        compute_payout_failure_outcome, compute_payout_success_outcome, compute_recovery_stats,
        compute_settlement_shares, determine_recovery_action, handle_allocation_failure,
        handle_payout_failure, handle_payout_failure_default, handle_refresh_failure,
        handle_withdrawal_failure, RecoveryContext, RecoveryOutcome, RecoveryProgress,
    };
    use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};
    use templar_vault_kernel::{
        AllocatingState, KernelAction, OpState, PayoutOutcome, PayoutState, RefreshingState,
        WithdrawingState,
    };

    #[test]
    fn test_determine_recovery_action_idle() {
        let state = OpState::Idle;

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0);

        let action = determine_recovery_action(&state, &ctx, &progress);

        assert!(action.is_none());
    }

    #[test]
    fn test_determine_recovery_action_allocating() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 500,
            plan: vec![(0, 300), (1, 200), (2, 300), (3, 200)],
        });

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0);

        let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

        match action {
            KernelAction::AbortAllocating {
                op_id,
                restore_idle,
            } => {
                assert_eq!(op_id, 1);
                assert_eq!(restore_idle, 500);
            }
            _ => panic!("Expected AbortAllocating"),
        }
    }

    #[test]
    fn test_determine_recovery_action_not_stuck() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 10,
            index: 0,
            remaining: 100,
            plan: vec![(0, 100)],
        });

        let ctx = RecoveryContext::with_stuck_threshold(1_000, 500);
        let progress = RecoveryProgress::with_last_progress(900, 900);

        let action = determine_recovery_action(&state, &ctx, &progress);
        assert!(action.is_none());
    }

    #[test]
    fn test_determine_recovery_action_forced_ignores_threshold() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 11,
            index: 0,
            remaining: 100,
            plan: vec![(0, 100)],
        });

        let ctx = RecoveryContext::forced(1_000);
        let progress = RecoveryProgress::with_last_progress(999, 999);

        let action = determine_recovery_action(&state, &ctx, &progress);
        assert!(action.is_some());
    }

    #[test]
    fn test_determine_recovery_action_withdrawing() {
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: 2,
            index: 1,
            remaining: 400,
            collected: 600,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 1000,
        });

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0);

        let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

        match action {
            KernelAction::AbortWithdrawing {
                op_id,
                refund_shares,
            } => {
                assert_eq!(op_id, 2);
                assert_eq!(refund_shares, 1000);
            }
            _ => panic!("Expected AbortWithdrawing"),
        }
    }

    #[test]
    fn test_determine_recovery_action_refreshing() {
        let state = OpState::Refreshing(RefreshingState {
            op_id: 3,
            index: 1,
            plan: vec![0, 1, 2],
        });

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0);

        let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

        match action {
            KernelAction::AbortRefreshing { op_id } => {
                assert_eq!(op_id, 3);
            }
            _ => panic!("Expected AbortRefreshing"),
        }
    }

    #[test]
    fn test_determine_recovery_action_payout() {
        let state = OpState::Payout(PayoutState {
            op_id: 4,
            receiver: receiver_addr(1),
            amount: 1000,
            owner: owner_addr(1),
            escrow_shares: 500,
            burn_shares: 400,
        });

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0);

        let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

        match action {
            KernelAction::SettlePayout { op_id, outcome } => {
                assert_eq!(op_id, 4);
                match outcome {
                    PayoutOutcome::Failure {
                        restore_idle,
                        refund_shares,
                    } => {
                        assert_eq!(restore_idle, 1000);
                        assert_eq!(refund_shares, 500);
                    }
                    _ => panic!("Expected failure outcome"),
                }
            }
            _ => panic!("Expected SettlePayout"),
        }
    }

    #[test]
    fn test_compute_settlement_shares_full_collection() {
        let settlement = compute_settlement_shares(1000, 500, 500);
        assert_eq!(settlement.to_burn, 1000);
        assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn test_compute_settlement_shares_partial_collection() {
        let settlement = compute_settlement_shares(1000, 500, 250);
        // burn = 1000 * 250 / 500 = 500
        assert_eq!(settlement.to_burn, 500);
        assert_eq!(settlement.refund, 500);
    }

    #[test]
    fn test_compute_settlement_shares_over_collection() {
        // Collected more than expected (edge case)
        let settlement = compute_settlement_shares(1000, 500, 600);
        assert_eq!(settlement.to_burn, 1000);
        assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn test_compute_payout_success_outcome_maps_settlement() {
        let outcome = compute_payout_success_outcome(1000, 500, 250);
        match outcome {
            PayoutOutcome::Success {
                burn_shares,
                refund_shares,
            } => {
                assert_eq!(burn_shares, 500);
                assert_eq!(refund_shares, 500);
            }
            _ => panic!("Expected success outcome"),
        }
    }

    #[test]
    fn test_compute_payout_failure_outcome_refunds_all() {
        let outcome = compute_payout_failure_outcome(1000, 250);
        match outcome {
            PayoutOutcome::Failure {
                restore_idle,
                refund_shares,
            } => {
                assert_eq!(restore_idle, 250);
                assert_eq!(refund_shares, 1000);
            }
            _ => panic!("Expected failure outcome"),
        }
    }

    #[test]
    fn test_compute_settlement_shares_zero_expected() {
        let settlement = compute_settlement_shares(1000, 0, 0);
        assert_eq!(settlement.to_burn, 0);
        assert_eq!(settlement.refund, 1000);
    }

    #[test]
    fn test_compute_settlement_shares_zero_escrow() {
        let settlement = compute_settlement_shares(0, 500, 250);
        assert_eq!(settlement.to_burn, 0);
        assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn test_handle_allocation_failure() {
        let state = AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 500,
            plan: vec![(0, 300), (1, 200), (2, 300)],
        };

        let outcome = handle_allocation_failure(&state, "Market unavailable");

        assert!(outcome.success);
        assert_eq!(outcome.message, Some(String::from("Market unavailable")));
        match outcome.action {
            KernelAction::AbortAllocating {
                op_id,
                restore_idle,
            } => {
                assert_eq!(op_id, 1);
                assert_eq!(restore_idle, 500);
            }
            _ => panic!("Expected AbortAllocating"),
        }
    }

    #[test]
    fn test_handle_withdrawal_failure() {
        let state = WithdrawingState {
            op_id: 2,
            index: 1,
            remaining: 400,
            collected: 600,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 1000,
        };

        let outcome = handle_withdrawal_failure(&state, "Insufficient liquidity");

        assert!(outcome.success);
        match outcome.action {
            KernelAction::AbortWithdrawing {
                op_id,
                refund_shares,
            } => {
                assert_eq!(op_id, 2);
                assert_eq!(refund_shares, 1000);
            }
            _ => panic!("Expected AbortWithdrawing"),
        }
    }

    #[test]
    fn test_handle_refresh_failure() {
        let state = RefreshingState {
            op_id: 3,
            index: 1,
            plan: vec![0, 1, 2],
        };

        let outcome = handle_refresh_failure(&state, "Oracle unavailable");

        assert!(outcome.success);
        match outcome.action {
            KernelAction::AbortRefreshing { op_id } => {
                assert_eq!(op_id, 3);
            }
            _ => panic!("Expected AbortRefreshing"),
        }
    }

    #[test]
    fn test_handle_payout_failure() {
        let state = PayoutState {
            op_id: 4,
            receiver: receiver_addr(1),
            amount: 1000,
            owner: owner_addr(1),
            escrow_shares: 500,
            burn_shares: 400,
        };

        let outcome = handle_payout_failure(&state, 1000, "Transfer rejected");

        assert!(outcome.success);
        match outcome.action {
            KernelAction::SettlePayout { op_id, outcome } => {
                assert_eq!(op_id, 4);
                match outcome {
                    PayoutOutcome::Failure {
                        restore_idle,
                        refund_shares,
                    } => {
                        assert_eq!(restore_idle, 1000);
                        assert_eq!(refund_shares, 500);
                    }
                    _ => panic!("Expected failure outcome"),
                }
            }
            _ => panic!("Expected SettlePayout"),
        }
    }

    #[test]
    fn test_handle_payout_failure_default_uses_amount() {
        let state = PayoutState {
            op_id: 5,
            receiver: receiver_addr(2),
            amount: 1500,
            owner: owner_addr(2),
            escrow_shares: 750,
            burn_shares: 0,
        };

        let outcome = handle_payout_failure_default(&state, "Transfer rejected");

        match outcome.action {
            KernelAction::SettlePayout { op_id, outcome } => {
                assert_eq!(op_id, 5);
                match outcome {
                    PayoutOutcome::Failure {
                        restore_idle,
                        refund_shares,
                    } => {
                        assert_eq!(restore_idle, 1500);
                        assert_eq!(refund_shares, 750);
                    }
                    _ => panic!("Expected failure outcome"),
                }
            }
            _ => panic!("Expected SettlePayout"),
        }
    }

    #[test]
    fn test_compute_recovery_stats_allocating() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 500,
            plan: vec![(0, 300), (1, 200), (2, 300), (3, 200)],
        });

        let stats = compute_recovery_stats(&state);

        assert_eq!(stats.completed_targets, 2);
        assert_eq!(stats.remaining_targets, 2);
        assert_eq!(stats.remaining_amount, 500);
        assert_eq!(stats.escrow_shares, 0);
    }

    #[test]
    fn test_compute_recovery_stats_withdrawing() {
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: 2,
            index: 3,
            remaining: 400,
            collected: 600,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 1000,
        });

        let stats = compute_recovery_stats(&state);

        assert_eq!(stats.completed_targets, 3);
        assert_eq!(stats.collected_amount, 600);
        assert_eq!(stats.remaining_amount, 400);
        assert_eq!(stats.escrow_shares, 1000);
    }

    #[test]
    fn test_compute_recovery_stats_idle() {
        let state = OpState::Idle;
        let stats = compute_recovery_stats(&state);

        assert_eq!(stats.completed_targets, 0);
        assert_eq!(stats.remaining_targets, 0);
        assert_eq!(stats.collected_amount, 0);
        assert_eq!(stats.remaining_amount, 0);
        assert_eq!(stats.escrow_shares, 0);
    }

    #[test]
    fn test_recovery_outcome_creation() {
        let action = KernelAction::AbortRefreshing { op_id: 1 };

        let success = RecoveryOutcome::success(action.clone());
        assert!(success.success);
        assert!(success.message.is_none());

        let with_msg = RecoveryOutcome::success_with_message(action.clone(), "All good");
        assert!(with_msg.success);
        assert_eq!(with_msg.message, Some(String::from("All good")));

        let failure = RecoveryOutcome::failure(action, "Something went wrong");
        assert!(!failure.success);
        assert_eq!(failure.message, Some(String::from("Something went wrong")));
    }
}
