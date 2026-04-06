//! Golden tests that compare plan outputs against fixed NEAR curator vault snapshots.
//!
//! These tests validate that the curator primitives produce deterministic outputs
//! when given the same inputs, ensuring compatibility with the NEAR vault implementation.

use alloc::vec;
use alloc::vec::Vec;

use crate::policy::cap_group::CapGroup;
use crate::policy::refresh_plan::build_refresh_plan;
use crate::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
use crate::policy::withdraw_route::{build_withdraw_route, WithdrawRoute, WithdrawRouteEntry};
#[cfg(feature = "recovery")]
use crate::recovery::{
    compute_recovery_stats, compute_settlement_shares, determine_recovery_action,
    PayoutRecoveryEvidence, RecoveryContext, RecoveryProgress,
};
#[cfg(feature = "recovery")]
use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};
use templar_vault_kernel::Wad;
#[cfg(feature = "recovery")]
use templar_vault_kernel::{
    state::op_state::AllocationPlanEntry, AllocatingState, KernelAction, OpState, PayoutOutcome,
    PayoutState, RefreshingState, WithdrawingState,
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

#[rstest::fixture]
fn near_snapshot() -> NearVaultSnapshot {
    NearVaultSnapshot::default()
}

mod auth_unit_tests {
    use crate::auth::{
        allowed_while_paused, boundary_policy_class, canonical_policy_class, ActionKind,
        AuthAdapter, AuthError, AuthPolicyClass, AuthResult, Caller,
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
            _caller: Address,
            _proof: Option<&[u8]>,
        ) -> AuthResult<()> {
            if self.paused && action != ActionKind::Pause {
                return Err(AuthError::VaultPaused);
            }

            if action.is_privileged() {
                return Err(AuthError::NotAuthorized {
                    caller: Caller::User,
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
            AuthPolicyClass::Sentinel
        );
        assert_eq!(
            canonical_policy_class(ActionKind::SetRestrictions),
            AuthPolicyClass::Sentinel
        );
        assert_eq!(
            canonical_policy_class(ActionKind::AbortRefreshing),
            AuthPolicyClass::AllocatorEmergency
        );
        assert_eq!(
            canonical_policy_class(ActionKind::ManualReconcile),
            AuthPolicyClass::Curator
        );
        assert_eq!(
            canonical_policy_class(ActionKind::PolicyAdmin),
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
            AuthPolicyClass::Sentinel
        );
    }

    #[test]
    fn test_allowed_while_paused_whitelist() {
        for action in [
            ActionKind::Pause,
            ActionKind::SetRestrictions,
            ActionKind::AbortAllocating,
            ActionKind::AbortWithdrawing,
            ActionKind::AbortRefreshing,
            ActionKind::ManualReconcile,
            ActionKind::EmergencyReset,
        ] {
            assert!(allowed_while_paused(action));
        }

        for action in [
            ActionKind::Deposit,
            ActionKind::RequestWithdraw,
            ActionKind::ExecuteWithdraw,
            ActionKind::BeginAllocating,
            ActionKind::FinishAllocating,
            ActionKind::SyncExternalAssets,
            ActionKind::RebalanceWithdraw,
            ActionKind::BeginRefreshing,
            ActionKind::FinishRefreshing,
            ActionKind::SettlePayout,
            ActionKind::RefreshFees,
            ActionKind::PolicyAdmin,
            ActionKind::AtomicWithdraw,
        ] {
            assert!(!allowed_while_paused(action));
        }
    }

    #[test]
    fn test_permissive_auth() {
        let auth = TestPermissiveAuth;
        let caller = Address([0u8; 32]);

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
        let caller = Address([0u8; 32]);

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
        let caller = Address([0u8; 32]);

        let result = auth.authorize(ActionKind::Pause, caller, None);
        assert!(matches!(result, Err(AuthError::NotAuthorized { .. })));

        let result = auth.authorize(ActionKind::BeginAllocating, caller, None);
        assert!(matches!(result, Err(AuthError::NotAuthorized { .. })));
    }

    #[test]
    fn test_strict_auth_paused() {
        let auth = TestStrictAuth::paused();
        let caller = Address([0u8; 32]);

        assert!(auth.is_paused());

        // Pause action is allowed even when paused.
        assert!(auth.authorize(ActionKind::Pause, caller, None).is_err());

        // User actions are denied when paused.
        let result = auth.authorize(ActionKind::Deposit, caller, None);
        assert!(matches!(result, Err(AuthError::VaultPaused)));
    }
}

// Golden Test: Cap Group Enforcement

#[rstest::rstest]
fn golden_cap_group_effective_caps(near_snapshot: NearVaultSnapshot) {
    let snapshot = near_snapshot;

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
        let cap = CapGroup::builder()
            .absolute_cap(*abs_cap)
            .relative_cap(Wad::from(*rel_cap))
            .build();
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

#[rstest::rstest]
fn golden_cap_group_available_capacity(near_snapshot: NearVaultSnapshot) {
    let snapshot = near_snapshot;

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
        let cap = CapGroup::builder()
            .absolute_cap(*abs_cap)
            .relative_cap(Wad::from(*rel_cap))
            .build();
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

#[rstest::rstest]
fn golden_cap_group_allocation_validation(near_snapshot: NearVaultSnapshot) {
    let snapshot = near_snapshot;

    // Test allocations against the "volatile" group (3M cap, 2.5M used)
    // Available: 500_000_000_000 (0.5M)

    let volatile_cap = CapGroup::builder()
        .absolute_cap(3_000_000_000_000)
        .relative_cap(Wad::from(WAD * 30 / 100))
        .build();
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
    let mut queue = SupplyQueue::default();

    queue
        .enqueue(SupplyQueueEntry::new(0, 500_000_000_000).unwrap())
        .unwrap();
    queue
        .enqueue(SupplyQueueEntry::new(1, 300_000_000_000).unwrap())
        .unwrap();
    queue
        .enqueue(SupplyQueueEntry::new(0, 200_000_000_000).unwrap())
        .unwrap();
    queue
        .enqueue(SupplyQueueEntry::new(2, 400_000_000_000).unwrap())
        .unwrap();
    queue
        .enqueue(SupplyQueueEntry::new(1, 100_000_000_000).unwrap())
        .unwrap();

    let total = queue.total().unwrap();
    assert_eq!(total, 1_500_000_000_000);

    let plan = queue.to_allocation_plan().unwrap();

    let target_0_amount = plan.iter().find(|(id, _)| *id == 0).map(|(_, a)| *a);
    let target_1_amount = plan.iter().find(|(id, _)| *id == 1).map(|(_, a)| *a);
    let target_2_amount = plan.iter().find(|(id, _)| *id == 2).map(|(_, a)| *a);

    assert_eq!(target_0_amount, Some(700_000_000_000));
    assert_eq!(target_1_amount, Some(400_000_000_000));
    assert_eq!(target_2_amount, Some(400_000_000_000));
}

#[test]
fn golden_supply_queue_priority_ordering() {
    let mut queue = SupplyQueue::default();

    // Add entries with different priorities
    queue
        .enqueue(SupplyQueueEntry::new_with_priority(0, 100_000_000_000_u128, 0).unwrap())
        .unwrap();
    queue
        .enqueue(SupplyQueueEntry::new_with_priority(1, 200_000_000_000_u128, 5).unwrap())
        .unwrap();
    queue
        .enqueue(SupplyQueueEntry::new_with_priority(2, 300_000_000_000_u128, 10).unwrap())
        .unwrap();
    queue
        .enqueue(SupplyQueueEntry::new_with_priority(3, 400_000_000_000_u128, 3).unwrap())
        .unwrap();

    let entries: Vec<u32> = queue
        .entries()
        .iter()
        .map(|entry| entry.target_id)
        .collect();
    assert_eq!(entries, vec![2, 1, 3, 0]);
}

// Golden Test: Withdraw Route Building

#[rstest::rstest]
fn golden_withdraw_route_from_principals(near_snapshot: NearVaultSnapshot) {
    let snapshot = near_snapshot;

    // Build a withdraw route for 2M USDC
    let target_amount = 2_000_000_000_000u128;

    let route = build_withdraw_route(&snapshot.market_principals, target_amount).unwrap();

    // Validate route
    assert!(route.validate().is_ok());

    // Route total should cover target
    let route_total = route.checked_total().unwrap();
    assert!(route_total >= target_amount);

    // Markets should be sorted by principal (largest first)
    // Expected order: 0 (3M), 1 (2.5M), 2 (1.5M)
    assert_eq!(route.entries()[0].target_id, 0);
    assert_eq!(route.entries()[1].target_id, 1);
    assert_eq!(route.entries()[2].target_id, 2);
}

#[test]
fn golden_withdraw_route_validation() {
    // Create a manually constructed route
    let route = WithdrawRoute::new(
        vec![
            WithdrawRouteEntry::new(0, 1_000_000_000_000).unwrap(),
            WithdrawRouteEntry::new(1, 800_000_000_000).unwrap(),
            WithdrawRouteEntry::new(2, 500_000_000_000).unwrap(),
        ],
        2_000_000_000_000,
    )
    .unwrap();

    // Should be valid (total 2.3M >= target 2M)
    assert!(route.validate().is_ok());

    // Route total
    assert_eq!(route.checked_total().unwrap(), 2_300_000_000_000);
}

// Golden Test: Refresh Plan

#[rstest::rstest]
fn golden_refresh_plan_building(near_snapshot: NearVaultSnapshot) {
    let snapshot = near_snapshot;
    let enabled_targets: Vec<u32> = snapshot
        .market_principals
        .iter()
        .map(|(id, _)| *id)
        .collect();

    let plan = build_refresh_plan(&enabled_targets).unwrap();

    assert_eq!(plan.len(), 3);
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
            AllocationPlanEntry::new(0, 300_000_000_000),
            AllocationPlanEntry::new(1, 200_000_000_000),
            AllocationPlanEntry::new(2, 300_000_000_000),
            AllocationPlanEntry::new(3, 200_000_000_000),
        ],
    });

    let ctx = RecoveryContext::after_inactivity(1_000_000_000_000, 1);
    let progress = RecoveryProgress::new(42, 0);
    let action = determine_recovery_action(&state, &ctx, &progress, None)
        .expect("expected action")
        .expect("expected action");

    match action {
        KernelAction::AbortAllocating { op_id } => {
            assert_eq!(op_id, 42);
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
        request_id: 43,
        index: 1,
        remaining: 400_000_000_000,
        collected: 600_000_000_000,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 1_000_000_000_000,
    });

    let ctx = RecoveryContext::after_inactivity(1_000_000_000_000, 1);
    let progress = RecoveryProgress::new(43, 0);
    let action = determine_recovery_action(&state, &ctx, &progress, None)
        .expect("expected action")
        .expect("expected action");

    match action {
        KernelAction::AbortWithdrawing { op_id } => {
            assert_eq!(op_id, 43);
        }
        _ => panic!("Expected AbortWithdrawing"),
    }
}

#[cfg(feature = "recovery")]
#[test]
fn golden_recovery_payout_state() {
    let state = OpState::Payout(PayoutState {
        op_id: 44,
        request_id: 44,
        receiver: receiver_addr(1),
        amount: 1_000_000_000_000,
        owner: owner_addr(1),
        escrow_shares: 500_000_000_000,
        burn_shares: 400_000_000_000,
    });

    let ctx = RecoveryContext::after_inactivity(1_000_000_000_000, 1);
    let progress = RecoveryProgress::new(44, 0);
    let action = determine_recovery_action(
        &state,
        &ctx,
        &progress,
        Some(PayoutRecoveryEvidence::Failure {
            restore_idle: 1_000_000_000_000,
        }),
    )
    .expect("expected action")
    .expect("expected action");

    match action {
        KernelAction::SettlePayout { op_id, outcome } => {
            assert_eq!(op_id, 44);
            assert_eq!(outcome, PayoutOutcome::Failure);
        }
        _ => panic!("Expected SettlePayout"),
    }
}

#[cfg(feature = "recovery")]
#[rstest::rstest]
#[case(
    1_000_000_000_000,
    500_000_000_000,
    500_000_000_000,
    1_000_000_000_000,
    0
)] // full
#[case(
    1_000_000_000_000,
    500_000_000_000,
    300_000_000_000,
    600_000_000_000,
    400_000_000_000
)] // partial
#[case(
    1_000_000_000_000,
    500_000_000_000,
    600_000_000_000,
    1_000_000_000_000,
    0
)] // over
fn golden_settlement_shares_cases(
    #[case] escrow: u128,
    #[case] expected: u128,
    #[case] collected: u128,
    #[case] expected_burn: u128,
    #[case] expected_refund: u128,
) {
    let settlement = compute_settlement_shares(escrow, expected, collected)
        .expect("golden settlement inputs should be valid");
    assert_eq!(settlement.to_burn, expected_burn);
    assert_eq!(settlement.refund, expected_refund);
}

#[cfg(feature = "recovery")]
#[test]
fn golden_settlement_shares_large_values() {
    let escrow = u128::MAX / 2;
    let expected = u128::MAX / 4;
    let collected = expected / 2;

    let settlement = compute_settlement_shares(escrow, expected, collected)
        .expect("golden settlement inputs should be valid");

    assert!(settlement.to_burn <= escrow);
    assert_eq!(settlement.to_burn + settlement.refund, escrow);
}

// Golden Test: Integration Scenario

#[rstest::rstest]
fn golden_full_allocation_cycle(near_snapshot: NearVaultSnapshot) {
    let snapshot = near_snapshot;

    let mut queue = SupplyQueue::default();
    queue
        .enqueue(SupplyQueueEntry::new(0, 400_000_000_000).unwrap())
        .unwrap();
    queue
        .enqueue(SupplyQueueEntry::new(1, 300_000_000_000).unwrap())
        .unwrap();
    queue
        .enqueue(SupplyQueueEntry::new(2, 300_000_000_000).unwrap())
        .unwrap();

    let plan = queue.to_allocation_plan().unwrap();
    assert_eq!(queue.total().unwrap(), 1_000_000_000_000);

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

        let cap = CapGroup::builder()
            .absolute_cap(*abs_cap)
            .relative_cap(Wad::from(*rel_cap))
            .build();
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
#[rstest::rstest]
fn golden_refresh_after_allocation(near_snapshot: NearVaultSnapshot) {
    let snapshot = near_snapshot;

    let enabled_targets: Vec<u32> = snapshot
        .market_principals
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let plan = build_refresh_plan(&enabled_targets).unwrap();

    let state = OpState::Refreshing(RefreshingState {
        op_id: 100,
        index: 1,
        plan: plan.targets().to_vec(),
    });

    // Check recovery from stuck refresh
    let ctx = RecoveryContext::after_inactivity(1_000_000_000_000, 1);
    let progress = RecoveryProgress::new(100, 0);
    let action = determine_recovery_action(&state, &ctx, &progress, None)
        .expect("expected action")
        .expect("expected action");

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

    use crate::policy::cap_group::{
        validate_allocations, CapGroup, CapGroupError, CapGroupId, CapGroupRecord,
    };
    use templar_vault_kernel::Wad;

    #[test]
    fn test_cap_group_unlimited() {
        let cap = CapGroup::default();
        assert!(cap.is_unlimited());
        assert!(cap.can_allocate(0, u128::MAX, 1000));
        assert!(!cap.can_allocate(u128::MAX, 1, 1000));
        assert!(matches!(
            cap.enforce(u128::MAX, 1, 1000),
            Err(CapGroupError::Overflow { .. })
        ));
    }

    #[test]
    fn test_cap_group_absolute_only() {
        let cap = CapGroup::builder().absolute_cap(1000).build();
        assert!(!cap.is_unlimited());
        assert!(cap.absolute_cap().is_some());
        assert!(cap.relative_cap().is_none());

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
        let cap = CapGroup::builder().relative_cap(Wad::from(WAD / 2)).build();
        assert!(!cap.is_unlimited());
        assert!(cap.absolute_cap().is_none());
        assert!(cap.relative_cap().is_some());

        // Total assets = 1000, effective cap = 500
        assert!(cap.can_allocate(0, 500, 1000));
        assert!(cap.can_allocate(200, 300, 1000));
        assert!(!cap.can_allocate(200, 301, 1000));
    }

    #[test]
    fn test_cap_group_both_caps() {
        // 1000 absolute, 50% relative
        let cap = CapGroup::builder()
            .absolute_cap(1000)
            .relative_cap(Wad::from(WAD / 2))
            .build();

        // With 3000 total assets, relative cap = 1500, but absolute = 1000
        assert!(cap.can_allocate(0, 1000, 3000));
        assert!(!cap.can_allocate(0, 1001, 3000));

        // With 1000 total assets, relative cap = 500, which is stricter
        assert!(cap.can_allocate(0, 500, 1000));
        assert!(!cap.can_allocate(0, 501, 1000));
    }

    #[test]
    fn test_compute_effective_cap() {
        let cap = CapGroup::builder()
            .absolute_cap(1000)
            .relative_cap(Wad::from(WAD / 2))
            .build();

        // When relative cap is stricter
        assert_eq!(cap.effective_cap(1000), 500);

        // When absolute cap is stricter
        assert_eq!(cap.effective_cap(3000), 1000);

        // Unlimited
        let unlimited = CapGroup::default();
        assert_eq!(unlimited.effective_cap(1000), u128::MAX);
    }

    #[test]
    fn test_enforce_cap_group_errors() {
        let cap = CapGroup::builder()
            .absolute_cap(1000)
            .relative_cap(Wad::from(WAD / 2))
            .build();

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
        let cap = CapGroup::builder().absolute_cap(1000).build();
        let unlimited = CapGroup::default();

        assert_eq!(cap.available_capacity(0, 2000), 1000);
        assert_eq!(cap.available_capacity(300, 2000), 700);
        assert_eq!(cap.available_capacity(1000, 2000), 0);
        assert_eq!(cap.available_capacity(1500, 2000), 0); // Already over, saturates to 0
        assert_eq!(unlimited.available_capacity(u128::MAX, 2000), 0);
    }

    #[test]
    fn test_apply_and_remove_allocation() {
        let cap = CapGroup::builder().absolute_cap(1000).build();
        let record = CapGroupRecord { cap, principal: 0 };

        let updated = record.apply_allocation(300).unwrap();
        assert_eq!(updated.principal, 300);

        let reduced = updated.remove_allocation(100).unwrap();
        assert_eq!(reduced.principal, 200);
    }

    #[test]
    fn test_remove_allocation_underflow_returns_error() {
        let cap = CapGroup::builder().absolute_cap(1000).build();
        let record = CapGroupRecord {
            cap,
            principal: 200,
        };

        assert!(matches!(
            record.remove_allocation(500),
            Err(CapGroupError::Underflow { .. })
        ));
    }

    #[test]
    fn test_apply_allocation_overflow_returns_error() {
        let record = CapGroupRecord {
            cap: CapGroup::default(),
            principal: u128::MAX,
        };

        assert!(matches!(
            record.apply_allocation(1),
            Err(CapGroupError::Overflow { .. })
        ));
    }

    #[test]
    fn test_validate_allocations() {
        let group1 = CapGroupId::try_from("group1").unwrap();
        let group2 = CapGroupId::try_from("group2").unwrap();

        let cap1 = CapGroupRecord {
            cap: CapGroup::builder().absolute_cap(1000).build(),
            principal: 0,
        };
        let cap2 = CapGroupRecord {
            cap: CapGroup::builder().absolute_cap(500).build(),
            principal: 0,
        };

        // Valid allocations to different groups
        let allocations = vec![(&group1, &cap1, 500u128), (&group2, &cap2, 300u128)];
        assert!(validate_allocations(&allocations, 2000).is_ok());

        // Invalid - second exceeds cap
        let invalid = vec![(&group1, &cap1, 500u128), (&group2, &cap2, 600u128)];
        assert!(validate_allocations(&invalid, 2000).is_err());
    }

    #[test]
    fn test_validate_allocations_cumulative_breach() {
        // CR-069: Test that multiple allocations to the same group are tracked cumulatively
        let group1 = CapGroupId::try_from("group1").unwrap();

        let cap1 = CapGroupRecord {
            cap: CapGroup::builder().absolute_cap(1000).build(),
            principal: 0,
        };

        // Two allocations of 600 each - individually valid (600 < 1000),
        // but together they exceed the cap (1200 > 1000)
        let allocations = vec![(&group1, &cap1, 600u128), (&group1, &cap1, 600u128)];
        assert!(
            validate_allocations(&allocations, 2000).is_err(),
            "cumulative allocations exceeding cap should fail"
        );

        // Two allocations that together stay within cap should succeed
        let valid_cumulative = vec![(&group1, &cap1, 400u128), (&group1, &cap1, 400u128)];
        assert!(
            validate_allocations(&valid_cumulative, 2000).is_ok(),
            "cumulative allocations within cap should succeed"
        );
    }

    #[test]
    fn test_cap_group_record_methods() {
        let record = CapGroupRecord {
            cap: CapGroup::builder().absolute_cap(1000).build(),
            principal: 0,
        };

        assert!(record.can_allocate(500, 2000));
        assert!(!record.can_allocate(1001, 2000));
        assert_eq!(record.available_capacity(2000), 1000);

        assert!(record.enforce(500, 2000).is_ok());
        assert!(record.enforce(1001, 2000).is_err());
    }

    #[test]
    fn test_zero_absolute_cap_is_preserved() {
        let cap = CapGroup::builder().absolute_cap(0).build();

        assert_eq!(cap.absolute_cap(), Some(0));
        assert!(!cap.is_unlimited());
    }

    #[test]
    fn test_zero_relative_cap_is_zero_cap() {
        let cap = CapGroup::builder().relative_cap(Wad::zero()).build();

        assert_eq!(cap.relative_cap(), Some(Wad::zero()));
        assert_eq!(cap.effective_cap(1_000), 0);
        assert!(!cap.can_allocate(0, 1, 1_000));
    }

    #[test]
    fn test_validate_allocations_rejects_inconsistent_records() {
        let group = CapGroupId::try_from("group1").unwrap();
        let canonical = CapGroupRecord {
            cap: CapGroup::builder().absolute_cap(1000).build(),
            principal: 90,
        };
        let stale = CapGroupRecord {
            cap: CapGroup::builder().absolute_cap(1000).build(),
            principal: 0,
        };

        let allocations = vec![(&group, &canonical, 5u128), (&group, &stale, 95u128)];

        assert!(matches!(
            validate_allocations(&allocations, 2_000),
            Err(CapGroupError::InconsistentRecord { .. })
        ));
    }

    proptest::proptest! {
        #[test]
        fn prop_available_capacity_matches_effective_cap(
            absolute in 0u128..=1_000_000_000_000u128,
            relative in 0u128..=WAD,
            current in 0u128..=1_000_000_000_000u128,
            total in 0u128..=1_000_000_000_000u128,
        ) {
            let cap = CapGroup::builder()
                .absolute_cap(absolute)
                .relative_cap(Wad::from(relative))
                .build();
            let effective = cap.effective_cap(total);
            let available = cap.available_capacity(current, total);

            proptest::prop_assert_eq!(available, effective.saturating_sub(current));
        }
    }
}

#[cfg(feature = "recovery")]
mod recovery_unit_tests {
    use alloc::string::String;
    use alloc::vec;

    use crate::recovery::{
        compute_payout_failure_outcome, compute_payout_success_outcome, compute_recovery_stats,
        compute_settlement_shares, determine_recovery_action, plan_allocation_recovery,
        plan_payout_recovery, plan_refresh_recovery, plan_withdrawal_recovery,
        PayoutRecoveryEvidence, RecoveryContext, RecoveryError, RecoveryOutcome, RecoveryProgress,
    };
    use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};
    use templar_vault_kernel::{
        state::op_state::AllocationPlanEntry, AllocatingState, KernelAction, OpState,
        PayoutOutcome, PayoutState, RefreshingState, WithdrawingState,
    };

    #[test]
    fn test_determine_recovery_action_idle() {
        let state = OpState::Idle;

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0, 0);

        let action = determine_recovery_action(&state, &ctx, &progress, None)
            .expect("idle should not error");

        assert!(action.is_none());
    }

    #[test]
    fn test_determine_recovery_action_allocating() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 500,
            plan: vec![
                AllocationPlanEntry::new(0, 300),
                AllocationPlanEntry::new(1, 200),
                AllocationPlanEntry::new(2, 300),
                AllocationPlanEntry::new(3, 200),
            ],
        });

        let ctx = RecoveryContext::after_inactivity(1000, 500);
        let progress = RecoveryProgress::new(1, 0);

        let action = determine_recovery_action(&state, &ctx, &progress, None)
            .expect("expected action")
            .expect("expected action");

        match action {
            KernelAction::AbortAllocating { op_id } => {
                assert_eq!(op_id, 1);
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
            plan: vec![AllocationPlanEntry::new(0, 100)],
        });

        let ctx = RecoveryContext::after_inactivity(1_000, 500);
        let progress = RecoveryProgress::with_last_progress(10, 900, 900);

        let action = determine_recovery_action(&state, &ctx, &progress, None)
            .expect("progress should be valid");
        assert!(action.is_none());
    }

    #[test]
    fn test_determine_recovery_action_forced_ignores_threshold() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 11,
            index: 0,
            remaining: 100,
            plan: vec![AllocationPlanEntry::new(0, 100)],
        });

        let ctx = RecoveryContext::forced(1_000);
        let progress = RecoveryProgress::with_last_progress(11, 999, 999);

        let action = determine_recovery_action(&state, &ctx, &progress, None);
        assert!(action
            .expect("recovery evaluation should succeed")
            .is_some());
    }

    #[test]
    fn test_determine_recovery_action_withdrawing() {
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: 2,
            request_id: 2,
            index: 1,
            remaining: 400,
            collected: 600,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 1000,
        });

        let ctx = RecoveryContext::after_inactivity(1000, 500);
        let progress = RecoveryProgress::new(2, 0);

        let action = determine_recovery_action(&state, &ctx, &progress, None)
            .expect("expected action")
            .expect("expected action");

        match action {
            KernelAction::AbortWithdrawing { op_id } => {
                assert_eq!(op_id, 2);
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

        let ctx = RecoveryContext::after_inactivity(1000, 500);
        let progress = RecoveryProgress::new(3, 0);

        let action = determine_recovery_action(&state, &ctx, &progress, None)
            .expect("expected action")
            .expect("expected action");

        match action {
            KernelAction::AbortRefreshing { op_id } => {
                assert_eq!(op_id, 3);
            }
            _ => panic!("Expected AbortRefreshing"),
        }
    }

    #[test]
    fn test_determine_recovery_action_payout_requires_evidence() {
        let state = OpState::Payout(PayoutState {
            op_id: 4,
            request_id: 4,
            receiver: receiver_addr(1),
            amount: 1000,
            owner: owner_addr(1),
            escrow_shares: 500,
            burn_shares: 400,
        });

        let ctx = RecoveryContext::after_inactivity(1000, 500);
        let progress = RecoveryProgress::new(4, 0);

        let action = determine_recovery_action(&state, &ctx, &progress, None);

        assert_eq!(action, Err(RecoveryError::UnknownPayoutState { op_id: 4 }));
    }

    #[test]
    fn test_determine_recovery_action_payout_with_failure_evidence() {
        let state = OpState::Payout(PayoutState {
            op_id: 4,
            request_id: 4,
            receiver: receiver_addr(1),
            amount: 1000,
            owner: owner_addr(1),
            escrow_shares: 500,
            burn_shares: 400,
        });

        let ctx = RecoveryContext::after_inactivity(1000, 500);
        let progress = RecoveryProgress::new(4, 0);

        let action = determine_recovery_action(
            &state,
            &ctx,
            &progress,
            Some(PayoutRecoveryEvidence::Failure { restore_idle: 1000 }),
        )
        .expect("expected action")
        .expect("expected action");

        match action {
            KernelAction::SettlePayout { op_id, outcome } => {
                assert_eq!(op_id, 4);
                assert_eq!(outcome, PayoutOutcome::Failure);
            }
            _ => panic!("Expected SettlePayout"),
        }
    }

    #[rstest::rstest]
    #[case(1000, 500, 500, Ok((1000, 0)))]
    #[case(1000, 500, 250, Ok((500, 500)))]
    #[case(1000, 500, 600, Err(RecoveryError::CollectedExceedsExpected { expected_amount: 500, collected_amount: 600 }))]
    #[case(1000, 0, 0, Err(RecoveryError::ExpectedAmountZero { escrow_shares: 1000, collected_amount: 0 }))]
    #[case(0, 500, 250, Ok((0, 0)))]
    fn test_compute_settlement_shares_cases(
        #[case] escrow: u128,
        #[case] expected: u128,
        #[case] collected: u128,
        #[case] expected_result: Result<(u128, u128), RecoveryError>,
    ) {
        let settlement = compute_settlement_shares(escrow, expected, collected);
        match (settlement, expected_result) {
            (Ok(settlement), Ok((expected_burn, expected_refund))) => {
                assert_eq!(settlement.to_burn, expected_burn);
                assert_eq!(settlement.refund, expected_refund);
            }
            (Err(actual_error), Err(expected_error)) => assert_eq!(actual_error, expected_error),
            (actual, expected) => {
                panic!("unexpected settlement result: actual={actual:?} expected={expected:?}")
            }
        }
    }

    #[test]
    fn test_compute_payout_success_outcome_maps_settlement() {
        let err = compute_payout_success_outcome(1000, 500, 250).unwrap_err();
        assert_eq!(err, RecoveryError::InvalidPayoutEvidence);
    }

    #[test]
    fn test_compute_payout_failure_outcome_refunds_all() {
        let outcome = compute_payout_failure_outcome(1000, 250, 250)
            .expect("full restore should remain representable");
        assert_eq!(outcome, PayoutOutcome::Failure);
    }

    #[test]
    fn test_compute_payout_failure_outcome_rejects_partial_restore() {
        let err = compute_payout_failure_outcome(1000, 500, 250).unwrap_err();
        assert_eq!(err, RecoveryError::InvalidPayoutEvidence);
    }

    #[test]
    fn test_determine_recovery_action_rejects_invalid_progress_timestamps() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 77,
            index: 0,
            remaining: 100,
            plan: vec![AllocationPlanEntry::new(0, 100)],
        });

        let ctx = RecoveryContext::forced(1_000);
        let progress = RecoveryProgress::with_last_progress(77, 900, 1_100);

        let action = determine_recovery_action(&state, &ctx, &progress, None);

        assert_eq!(
            action,
            Err(RecoveryError::InvalidProgressTimestamps {
                started_at_ns: 900,
                last_progress_ns: 1_100,
                current_ns: 1_000,
            })
        );
    }

    #[test]
    fn test_plan_allocation_recovery() {
        let state = AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 500,
            plan: vec![
                AllocationPlanEntry::new(0, 300),
                AllocationPlanEntry::new(1, 200),
                AllocationPlanEntry::new(2, 300),
            ],
        };

        let outcome = plan_allocation_recovery(&state, "Market unavailable");

        assert!(outcome.planned);
        assert_eq!(outcome.message, Some(String::from("Market unavailable")));
        match outcome.action {
            KernelAction::AbortAllocating { op_id } => {
                assert_eq!(op_id, 1);
            }
            _ => panic!("Expected AbortAllocating"),
        }
    }

    #[test]
    fn test_plan_withdrawal_recovery() {
        let state = WithdrawingState {
            op_id: 2,
            request_id: 2,
            index: 1,
            remaining: 400,
            collected: 600,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 1000,
        };

        let outcome = plan_withdrawal_recovery(&state, "Insufficient liquidity");

        assert!(outcome.planned);
        match outcome.action {
            KernelAction::AbortWithdrawing { op_id } => {
                assert_eq!(op_id, 2);
            }
            _ => panic!("Expected AbortWithdrawing"),
        }
    }

    #[test]
    fn test_plan_refresh_recovery() {
        let state = RefreshingState {
            op_id: 3,
            index: 1,
            plan: vec![0, 1, 2],
        };

        let outcome = plan_refresh_recovery(&state, "Oracle unavailable");

        assert!(outcome.planned);
        match outcome.action {
            KernelAction::AbortRefreshing { op_id } => {
                assert_eq!(op_id, 3);
            }
            _ => panic!("Expected AbortRefreshing"),
        }
    }

    #[test]
    fn test_plan_payout_recovery_failure() {
        let state = PayoutState {
            op_id: 4,
            request_id: 4,
            receiver: receiver_addr(1),
            amount: 1000,
            owner: owner_addr(1),
            escrow_shares: 500,
            burn_shares: 400,
        };

        let outcome = plan_payout_recovery(
            &state,
            PayoutRecoveryEvidence::Failure { restore_idle: 1000 },
            "Transfer rejected",
        )
        .expect("payout failure planning should succeed");

        assert!(outcome.planned);
        match outcome.action {
            KernelAction::SettlePayout { op_id, outcome } => {
                assert_eq!(op_id, 4);
                assert_eq!(outcome, PayoutOutcome::Failure);
            }
            _ => panic!("Expected SettlePayout"),
        }
    }

    #[test]
    fn test_plan_payout_recovery_success() {
        let state = PayoutState {
            op_id: 5,
            request_id: 5,
            receiver: receiver_addr(2),
            amount: 1500,
            owner: owner_addr(2),
            escrow_shares: 750,
            burn_shares: 0,
        };

        let outcome = plan_payout_recovery(
            &state,
            PayoutRecoveryEvidence::Success {
                collected_amount: 1500,
            },
            "Transfer completed",
        )
        .expect("payout success planning should succeed");

        match outcome.action {
            KernelAction::SettlePayout { op_id, outcome } => {
                assert_eq!(op_id, 5);
                assert_eq!(outcome, PayoutOutcome::Success);
            }
            _ => panic!("Expected SettlePayout"),
        }
    }

    #[test]
    fn test_determine_recovery_action_rejects_progress_for_wrong_op() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 99,
            index: 0,
            remaining: 100,
            plan: vec![AllocationPlanEntry::new(0, 100)],
        });

        let ctx = RecoveryContext::forced(1000);
        let progress = RecoveryProgress::new(100, 900);

        let action = determine_recovery_action(&state, &ctx, &progress, None);

        assert_eq!(
            action,
            Err(RecoveryError::ProgressOpMismatch {
                expected_op_id: 99,
                progress_op_id: 100,
            })
        );
    }

    #[test]
    fn test_determine_recovery_action_uses_max_total_age() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 12,
            index: 0,
            remaining: 100,
            plan: vec![AllocationPlanEntry::new(0, 100)],
        });

        let ctx = RecoveryContext::after_inactivity_with_max_age(1_000, 500, 900);
        let progress = RecoveryProgress::with_last_progress(12, 0, 950);

        let action = determine_recovery_action(&state, &ctx, &progress, None)
            .expect("progress should be valid");

        assert!(action.is_some());
    }

    #[test]
    fn test_compute_recovery_stats_allocating() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 500,
            plan: vec![
                AllocationPlanEntry::new(0, 300),
                AllocationPlanEntry::new(1, 200),
                AllocationPlanEntry::new(2, 300),
                AllocationPlanEntry::new(3, 200),
            ],
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
            request_id: 2,
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
    fn test_compute_recovery_stats_clamps_completed_targets_to_plan_len() {
        let state = OpState::Refreshing(RefreshingState {
            op_id: 3,
            index: 5,
            plan: vec![1, 2, 3],
        });

        let stats = compute_recovery_stats(&state);

        assert_eq!(stats.completed_targets, 3);
        assert_eq!(stats.remaining_targets, 0);
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

        let planned = RecoveryOutcome::planned(action.clone());
        assert!(planned.planned);
        assert!(planned.message.is_none());

        let with_msg = RecoveryOutcome::planned_with_message(action, "All good");
        assert!(with_msg.planned);
        assert_eq!(with_msg.message, Some(String::from("All good")));
    }
}

mod governance_module_tests {
    pub use crate::governance::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use templar_vault_kernel::{DurationNs, TimestampNs, Wad};

    fn identity_key<'a>(value: &&'a str) -> &'a str {
        value
    }

    #[test]
    fn pending_value_maturity_is_time_based() {
        let pending = PendingValue {
            value: "ok",
            ready_at_ns: TimestampNs(1_000),
        };

        assert!(!pending.is_mature(TimestampNs(999)));
        assert!(pending.is_mature(TimestampNs(1_000)));
        assert!(pending.is_mature(TimestampNs(1_001)));
    }

    #[test]
    fn queue_take_by_key_enforces_timelock() {
        let mut queue = PendingActions::from_restored_entries(vec![PendingValue {
            value: "change",
            ready_at_ns: TimestampNs(1_000),
        }]);

        let not_ready = queue.take_by_key(TimestampNs(999), &"change", identity_key);
        assert_eq!(
            not_ready,
            TakePending::Pending {
                ready_at_ns: TimestampNs(1_000)
            }
        );
        assert_eq!(queue.len(), 1);

        let ready = queue.take_by_key(TimestampNs(1_000), &"change", identity_key);
        assert_eq!(ready, TakePending::Ready("change"));
        assert!(queue.is_empty());
    }

    #[test]
    fn queue_take_by_key_reports_missing() {
        let mut queue = PendingActions::default();
        queue.schedule("change", TimestampNs(1_000), DurationNs(10));

        assert_eq!(
            queue.take_by_key(TimestampNs(2_000), &"other", identity_key),
            TakePending::Missing
        );
    }

    #[test]
    fn queue_schedule_uses_delay_to_compute_maturity() {
        let mut queue = PendingActions::default();

        queue.schedule("change", TimestampNs(1_000), DurationNs(50));

        assert_eq!(
            queue.back().map(|entry| entry.ready_at_ns),
            Some(TimestampNs(1_050))
        );
    }

    #[test]
    fn queue_schedule_zero_delay_is_ready_at_current_time() {
        let mut queue = PendingActions::default();

        queue.schedule("change", TimestampNs(1_000), DurationNs::ZERO);

        let entry = queue.back().expect("scheduled entry must exist");
        assert_eq!(entry.ready_at_ns, TimestampNs(1_000));
        assert!(entry.is_mature(TimestampNs(1_000)));
    }

    #[test]
    fn queue_schedule_overflow_saturates_ready_time() {
        let mut queue = PendingActions::default();

        queue.schedule("change", TimestampNs(u64::MAX - 5), DurationNs(10));

        assert_eq!(
            queue.back().map(|entry| entry.ready_at_ns),
            Some(TimestampNs(u64::MAX))
        );
    }

    #[test]
    fn queue_schedule_replacing_supersedes_existing_key() {
        let mut queue = PendingActions::default();
        queue.schedule("old", TimestampNs(1_000), DurationNs(100));

        let scheduled = queue.schedule_replacing(
            &"old",
            identity_key,
            "old",
            TimestampNs(1_050),
            DurationNs(200),
        );

        assert_eq!(scheduled.replaced, alloc::vec!["old"]);
        assert_eq!(scheduled.ready_at_ns, TimestampNs(1_250));
        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue.back().map(|entry| entry.ready_at_ns),
            Some(TimestampNs(1_250))
        );
    }

    #[test]
    fn timelock_config_decision_treats_reductions_as_timelocked() {
        let decision = timelock_config_decision(
            DurationNs(100),
            DurationNs(50),
            DurationNs(10),
            DurationNs(200),
        );

        assert_eq!(decision, Ok(TimelockDecision::Timelocked));
    }

    #[test]
    fn timelock_config_decision_rejects_out_of_bounds_values() {
        let decision = timelock_config_decision(
            DurationNs(100),
            DurationNs(250),
            DurationNs(10),
            DurationNs(200),
        );

        assert_eq!(decision, Err(TimelockConfigError::OutOfBounds));
    }

    #[test]
    fn fee_change_decision_marks_recipient_change_as_timelocked() {
        let current = FeeConfig {
            performance_fee: Wad::from(10_u128),
            management_fee: Wad::from(20_u128),
            performance_recipient: &"alice",
            management_recipient: &"bob",
            max_rate: Some(Wad::from(30_u128)),
        };
        let proposed = FeeConfig {
            performance_fee: Wad::from(10_u128),
            management_fee: Wad::from(20_u128),
            performance_recipient: &"carol",
            management_recipient: &"bob",
            max_rate: Some(Wad::from(30_u128)),
        };

        assert_eq!(
            FeeConfig::evaluate_change(&current, &proposed),
            Ok(FeeChangeDecision {
                timelocked: true,
                fee_increase: false,
                recipient_changed: true,
                max_rate_relaxed: false,
            })
        );
    }

    #[test]
    fn cap_change_decision_market_new_cap_is_timelocked() {
        let decision = TimelockDecision::from_cap_change(None, 100);
        assert_eq!(decision, Ok(TimelockDecision::Timelocked));
    }

    #[test]
    fn cap_group_cap_change_decision_unlimited_to_finite_is_immediate() {
        let from_none = TimelockDecision::from_cap_group_cap_change(None, Some(100));
        assert_eq!(from_none, Ok(TimelockDecision::Immediate));

        let from_zero = TimelockDecision::from_cap_group_cap_change(Some(0), Some(100));
        assert_eq!(from_zero, Ok(TimelockDecision::Timelocked));
    }

    #[test]
    fn cap_group_cap_change_decision_finite_to_unlimited_is_timelocked() {
        let decision = TimelockDecision::from_cap_group_cap_change(Some(100), None);
        assert_eq!(decision, Ok(TimelockDecision::Timelocked));
    }

    #[test]
    fn cap_group_cap_change_decision_finite_decrease_is_immediate() {
        let decision = TimelockDecision::from_cap_group_cap_change(Some(100), Some(50));
        assert_eq!(decision, Ok(TimelockDecision::Immediate));
    }

    #[test]
    fn relative_cap_change_decision_is_directional() {
        assert_eq!(
            TimelockDecision::from_relative_cap_change(
                Some(Wad::from(10_u128)),
                Some(Wad::from(20_u128))
            ),
            Ok(TimelockDecision::Timelocked)
        );
        assert_eq!(
            TimelockDecision::from_relative_cap_change(
                Some(Wad::from(20_u128)),
                Some(Wad::from(10_u128))
            ),
            Ok(TimelockDecision::Immediate)
        );
    }

    #[test]
    fn membership_change_kind_is_directional() {
        assert_eq!(
            TimelockDecision::membership_change_kind::<u32>(None, Some(&1)),
            Some(MembershipChangeKind::Added)
        );
        assert_eq!(
            TimelockDecision::membership_change_kind(Some(&1), None::<&u32>),
            Some(MembershipChangeKind::Removed)
        );
        assert_eq!(
            TimelockDecision::membership_change_kind(Some(&1), Some(&2)),
            Some(MembershipChangeKind::Reassigned)
        );
        assert_eq!(
            TimelockDecision::membership_change_kind(Some(&1), Some(&1)),
            None
        );
    }

    #[test]
    fn membership_assignment_change_requires_actual_difference() {
        assert_eq!(
            TimelockDecision::from_membership_assignment_change(Some(&1_u32), Some(&1_u32)),
            Err(MembershipChangeError::NoChange)
        );
        assert_eq!(
            TimelockDecision::from_membership_assignment_change(Some(&1_u32), Some(&2_u32)),
            Ok(TimelockDecision::Timelocked)
        );
    }

    #[test]
    fn determine_relaxed_paused_to_empty_whitelist_is_not_relaxing() {
        let current = Some(Restrictions::<&str>::Paused);
        let next = Some(Restrictions::Whitelist(Vec::new()));

        assert!(!Restrictions::determine_relaxed(&current, &next));
    }

    #[test]
    fn determine_relaxed_paused_to_nonempty_whitelist_is_relaxing() {
        let current = Some(Restrictions::<&str>::Paused);
        let next = Some(Restrictions::Whitelist(vec!["alice"]));

        assert!(Restrictions::determine_relaxed(&current, &next));
    }
}

mod rbac_module_tests {
    pub use crate::rbac::*;
    use crate::{ActionKind, AuthAdapter, AuthError, AuthPolicyClass, AuthResult};
    use alloc::vec;
    use templar_vault_kernel::Address;

    #[rstest::fixture]
    fn curator_addr() -> Address {
        Address([1u8; 32])
    }

    #[rstest::fixture]
    fn guardian_addr() -> Address {
        Address([2u8; 32])
    }

    #[rstest::fixture]
    fn allocator_addr() -> Address {
        Address([3u8; 32])
    }

    #[rstest::fixture]
    fn user_addr() -> Address {
        Address([4u8; 32])
    }

    #[rstest::fixture]
    fn sentinel_addr() -> Address {
        Address([5u8; 32])
    }

    #[rstest::fixture]
    fn rbac_auth(
        curator_addr: Address,
        allocator_addr: Address,
        sentinel_addr: Address,
    ) -> RbacAuth {
        let mut config = RbacConfig::with_curator(curator_addr);
        config.add_role(allocator_addr, Role::Allocator);
        config.add_role(sentinel_addr, Role::Sentinel);
        RbacAuth::new(config)
    }

    #[rstest::rstest]
    fn test_role_assignment(curator_addr: Address, user_addr: Address) {
        let config = RbacConfig::with_curator(curator_addr);

        assert!(config.has_role(&curator_addr, Role::Curator));
        assert!(!config.has_role(&user_addr, Role::Curator));
    }

    #[rstest::rstest]
    fn test_add_remove_role(curator_addr: Address, sentinel_addr: Address) {
        let mut config = RbacConfig::with_curator(curator_addr);

        assert!(config.add_role(sentinel_addr, Role::Sentinel));
        assert!(config.has_role(&sentinel_addr, Role::Sentinel));
        assert!(!config.add_role(sentinel_addr, Role::Sentinel));

        assert!(config.remove_role(&sentinel_addr, Role::Sentinel));
        assert!(!config.has_role(&sentinel_addr, Role::Sentinel));
        assert!(!config.remove_role(&sentinel_addr, Role::Sentinel));
    }

    #[rstest::rstest]
    fn test_get_roles(curator_addr: Address) {
        let mut config = RbacConfig::with_curator(curator_addr);
        config.add_role(curator_addr, Role::Sentinel); // Curator also sentinel

        let roles = config.get_roles(&curator_addr);
        assert_eq!(roles.len(), 2);
        assert!(roles.contains(&Role::Curator));
        assert!(roles.contains(&Role::Sentinel));
    }

    #[rstest::rstest]
    fn test_sentinel_role(
        curator_addr: Address,
        sentinel_addr: Address,
        user_addr: Address,
        guardian_addr: Address,
    ) {
        let mut config = RbacConfig::with_curator(curator_addr);
        config.add_role(sentinel_addr, Role::Sentinel);

        assert!(config.has_role(&sentinel_addr, Role::Sentinel));
        assert!(!config.has_role(&user_addr, Role::Sentinel));
        assert!(!config.has_role(&guardian_addr, Role::Sentinel));

        assert_eq!(Role::Sentinel.as_str(), "sentinel");

        let roles = config.get_roles(&sentinel_addr);
        assert_eq!(roles.len(), 1);
        assert!(roles.contains(&Role::Sentinel));
    }

    #[rstest::rstest]
    fn test_sentinel_add_remove(curator_addr: Address, sentinel_addr: Address) {
        let mut config = RbacConfig::with_curator(curator_addr);

        assert!(config.add_role(sentinel_addr, Role::Sentinel));
        assert!(config.has_role(&sentinel_addr, Role::Sentinel));

        assert!(config.remove_role(&sentinel_addr, Role::Sentinel));
        assert!(!config.has_role(&sentinel_addr, Role::Sentinel));
    }

    #[rstest::rstest]
    fn test_role_assignments_snapshot(curator_addr: Address, sentinel_addr: Address) {
        let mut config = RbacConfig::with_curator(curator_addr);
        config.add_role(sentinel_addr, Role::Sentinel);

        let assignments = config.role_assignments();
        assert_eq!(assignments.len(), 2);
        assert!(assignments.contains(&RoleAssignment {
            address: curator_addr,
            role: Role::Curator,
        }));
        assert!(assignments.contains(&RoleAssignment {
            address: sentinel_addr,
            role: Role::Sentinel,
        }));
    }

    fn assert_missing_role(
        result: AuthResult<()>,
        action: ActionKind,
        policy_class: AuthPolicyClass,
    ) {
        assert!(matches!(
            result,
            Err(AuthError::MissingRole {
                action: actual_action,
                policy_class: actual_policy_class,
            }) if actual_action == action && actual_policy_class == policy_class
        ));
    }

    #[rstest::rstest]
    fn test_cannot_remove_last_curator(curator_addr: Address) {
        let mut config = RbacConfig::with_curator(curator_addr);

        assert!(!config.remove_role(&curator_addr, Role::Curator));
        assert!(config.has_role(&curator_addr, Role::Curator));
    }

    #[rstest::rstest]
    fn test_user_actions_allowed(rbac_auth: RbacAuth, user_addr: Address) {
        let auth = rbac_auth;

        assert!(auth.authorize(ActionKind::Deposit, user_addr, None).is_ok());
        assert!(auth
            .authorize(ActionKind::RequestWithdraw, user_addr, None)
            .is_ok());
    }

    #[rstest::rstest]
    fn test_execute_withdraw_allocator_only(
        rbac_auth: RbacAuth,
        allocator_addr: Address,
        user_addr: Address,
        curator_addr: Address,
    ) {
        let auth = rbac_auth;

        assert!(auth
            .authorize(ActionKind::ExecuteWithdraw, allocator_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::ExecuteWithdraw, curator_addr, None)
            .is_ok());

        let result = auth.authorize(ActionKind::ExecuteWithdraw, user_addr, None);
        assert_missing_role(
            result,
            ActionKind::ExecuteWithdraw,
            AuthPolicyClass::Allocator,
        );
    }

    #[rstest::rstest]
    fn test_abort_actions_allow_allocator_or_sentinel(
        rbac_auth: RbacAuth,
        allocator_addr: Address,
        sentinel_addr: Address,
        user_addr: Address,
    ) {
        let auth = rbac_auth;

        assert!(auth
            .authorize(ActionKind::AbortAllocating, allocator_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::AbortAllocating, sentinel_addr, None)
            .is_ok());

        let result = auth.authorize(ActionKind::AbortAllocating, user_addr, None);
        assert_missing_role(
            result,
            ActionKind::AbortAllocating,
            AuthPolicyClass::AllocatorEmergency,
        );
    }

    #[rstest::rstest]
    fn test_sentinel_can_pause(
        rbac_auth: RbacAuth,
        sentinel_addr: Address,
        guardian_addr: Address,
        user_addr: Address,
    ) {
        let auth = rbac_auth;

        assert!(auth
            .authorize(ActionKind::Pause, sentinel_addr, None)
            .is_ok());

        let result = auth.authorize(ActionKind::Pause, guardian_addr, None);
        assert_missing_role(result, ActionKind::Pause, AuthPolicyClass::Sentinel);

        let result = auth.authorize(ActionKind::Pause, user_addr, None);
        assert_missing_role(result, ActionKind::Pause, AuthPolicyClass::Sentinel);
    }

    #[rstest::rstest]
    fn test_allocator_actions(rbac_auth: RbacAuth, allocator_addr: Address, user_addr: Address) {
        let auth = rbac_auth;

        assert!(auth
            .authorize(ActionKind::BeginAllocating, allocator_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::FinishAllocating, allocator_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::SyncExternalAssets, allocator_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::BeginRefreshing, allocator_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::FinishRefreshing, allocator_addr, None)
            .is_ok());

        let result = auth.authorize(ActionKind::BeginAllocating, user_addr, None);
        assert_missing_role(
            result,
            ActionKind::BeginAllocating,
            AuthPolicyClass::Allocator,
        );
    }

    #[rstest::rstest]
    fn test_curator_scoped_actions_with_allocator_bypass(
        rbac_auth: RbacAuth,
        curator_addr: Address,
        sentinel_addr: Address,
    ) {
        let auth = rbac_auth;

        assert!(auth
            .authorize(ActionKind::ManualReconcile, curator_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::Deposit, curator_addr, None)
            .is_ok());

        let result = auth.authorize(ActionKind::Pause, curator_addr, None);
        assert_missing_role(result, ActionKind::Pause, AuthPolicyClass::Sentinel);

        assert!(auth
            .authorize(ActionKind::BeginAllocating, curator_addr, None)
            .is_ok());

        assert!(auth
            .authorize(ActionKind::Pause, sentinel_addr, None)
            .is_ok());
    }

    #[rstest::rstest]
    fn test_manual_reconcile_curator_only(
        rbac_auth: RbacAuth,
        curator_addr: Address,
        allocator_addr: Address,
        guardian_addr: Address,
    ) {
        let auth = rbac_auth;

        assert!(auth
            .authorize(ActionKind::ManualReconcile, curator_addr, None)
            .is_ok());

        let result = auth.authorize(ActionKind::ManualReconcile, allocator_addr, None);
        assert_missing_role(
            result,
            ActionKind::ManualReconcile,
            AuthPolicyClass::Curator,
        );

        let result = auth.authorize(ActionKind::ManualReconcile, guardian_addr, None);
        assert_missing_role(
            result,
            ActionKind::ManualReconcile,
            AuthPolicyClass::Curator,
        );
    }

    #[rstest::rstest]
    fn test_paused_blocks_user_actions(
        rbac_auth: RbacAuth,
        user_addr: Address,
        allocator_addr: Address,
    ) {
        let mut auth = rbac_auth;
        auth.set_paused(true);

        let result = auth.authorize(ActionKind::Deposit, user_addr, None);
        assert!(matches!(result, Err(AuthError::VaultPaused)));

        let result = auth.authorize(ActionKind::BeginAllocating, allocator_addr, None);
        assert!(matches!(result, Err(AuthError::VaultPaused)));
    }

    #[rstest::rstest]
    fn test_paused_allows_pause_action(rbac_auth: RbacAuth, sentinel_addr: Address) {
        let mut auth = rbac_auth;
        auth.set_paused(true);

        assert!(auth
            .authorize(ActionKind::Pause, sentinel_addr, None)
            .is_ok());
    }

    #[rstest::rstest]
    fn test_paused_allows_emergency_actions(
        rbac_auth: RbacAuth,
        sentinel_addr: Address,
        curator_addr: Address,
    ) {
        let mut auth = rbac_auth;
        auth.set_paused(true);

        assert!(auth
            .authorize(ActionKind::AbortAllocating, sentinel_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::ManualReconcile, curator_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::AbortWithdrawing, sentinel_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::AbortRefreshing, sentinel_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::SetRestrictions, sentinel_addr, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::EmergencyReset, curator_addr, None)
            .is_ok());
    }

    #[rstest::rstest]
    fn test_is_paused(rbac_auth: RbacAuth) {
        let mut auth = rbac_auth;

        assert!(!auth.is_paused());

        auth.set_paused(true);
        assert!(auth.is_paused());
    }

    #[test]
    fn test_allowed_roles_for_action_matches_emergency_policy() {
        let roles = allowed_roles_for_action(ActionKind::AbortRefreshing);

        assert_eq!(roles.len(), 3);
        assert!(roles.contains(&Role::Allocator));
        assert!(roles.contains(&Role::Sentinel));
        assert!(roles.contains(&Role::Curator));
    }

    #[test]
    fn test_allowed_roles_for_action_curator_policy() {
        let roles = allowed_roles_for_action(ActionKind::PolicyAdmin);

        assert_eq!(roles, vec![Role::Curator]);
    }

    #[test]
    fn test_role_as_str() {
        assert_eq!(Role::Curator.as_str(), "curator");
        assert_eq!(Role::Sentinel.as_str(), "sentinel");
        assert_eq!(Role::Allocator.as_str(), "allocator");
    }
}

mod utils_module_tests {
    use crate::utils::{nonnegative_i128_to_u128, seconds_to_nanoseconds, u128_to_i128_checked};

    #[test]
    fn converts_seconds_to_nanoseconds() {
        assert_eq!(seconds_to_nanoseconds(1), Some(1_000_000_000));
        assert_eq!(seconds_to_nanoseconds(42), Some(42_000_000_000));
    }

    #[test]
    fn returns_none_on_overflow() {
        assert_eq!(seconds_to_nanoseconds(u64::MAX), None);
    }

    #[test]
    fn converts_u128_to_i128_when_in_range() {
        assert_eq!(u128_to_i128_checked(0), Some(0));
        assert_eq!(u128_to_i128_checked(i128::MAX as u128), Some(i128::MAX));
    }

    #[test]
    fn rejects_u128_to_i128_when_out_of_range() {
        assert_eq!(u128_to_i128_checked((i128::MAX as u128) + 1), None);
    }

    #[test]
    fn converts_nonnegative_i128_to_u128() {
        assert_eq!(nonnegative_i128_to_u128(0), Some(0));
        assert_eq!(nonnegative_i128_to_u128(42), Some(42));
    }

    #[test]
    fn rejects_negative_i128_to_u128() {
        assert_eq!(nonnegative_i128_to_u128(-1), None);
    }
}

mod policy_cap_group_update_tests {
    use crate::policy::cap_group::{CapGroupId, CapGroupUpdate, CapGroupUpdateKey};
    use templar_vault_kernel::Wad;

    #[test]
    fn cap_group_update_uses_canonical_set_cap_shape() {
        let update = CapGroupUpdate::SetCap {
            cap_group_id: CapGroupId::try_from("group-a").unwrap(),
            new_cap: Some(123),
        };

        assert_eq!(
            update,
            CapGroupUpdate::SetCap {
                cap_group_id: CapGroupId::try_from("group-a").unwrap(),
                new_cap: Some(123),
            }
        );
    }

    #[test]
    fn cap_group_update_uses_canonical_set_relative_cap_shape() {
        let update = CapGroupUpdate::SetRelativeCap {
            cap_group_id: CapGroupId::try_from("group-b").unwrap(),
            new_relative_cap: Some(Wad::from(999u128)),
        };

        assert_eq!(
            update,
            CapGroupUpdate::SetRelativeCap {
                cap_group_id: CapGroupId::try_from("group-b").unwrap(),
                new_relative_cap: Some(Wad::from(999u128)),
            }
        );
    }

    #[test]
    fn cap_group_update_uses_canonical_membership_shape() {
        let update = CapGroupUpdate::SetMembership {
            market_id: 77,
            cap_group_id: Some(CapGroupId::try_from("group-c").unwrap()),
        };

        assert_eq!(
            update,
            CapGroupUpdate::SetMembership {
                market_id: 77,
                cap_group_id: Some(CapGroupId::try_from("group-c").unwrap()),
            }
        );
    }

    #[test]
    fn cap_group_update_key_uses_canonical_shape() {
        let key = CapGroupUpdateKey::SetRelativeCap {
            cap_group_id: CapGroupId::try_from("group-key").unwrap(),
        };
        assert_eq!(
            key,
            CapGroupUpdateKey::SetRelativeCap {
                cap_group_id: CapGroupId::try_from("group-key").unwrap(),
            }
        );
    }
}

mod policy_cap_group_adapter_tests {
    pub use crate::policy::cap_group_adapter::*;
    use crate::{CapGroup, CapGroupRecord};
    pub use templar_vault_kernel::Wad;

    const WAD: u128 = Wad::SCALE;

    #[test]
    fn builds_cap_group_and_record_from_fields() {
        let cap = CapGroup::builder()
            .absolute_cap(1_000)
            .relative_cap(Wad::from(WAD / 2))
            .build();
        assert_eq!(cap.absolute_cap(), Some(1_000));
        assert_eq!(cap.relative_cap(), Some(Wad::from(WAD / 2)));

        let record = CapGroupRecord {
            cap,
            principal: 300,
        };
        assert_eq!(record.principal, 300);
        assert_eq!(record.cap.absolute_cap(), Some(1_000));
    }

    #[test]
    fn alloc_helpers_match_cap_group_behavior() {
        let cap = CapGroup::builder()
            .absolute_cap(1_000)
            .relative_cap(Wad::one())
            .build();

        assert!(cap.can_allocate(300, 500, 2_000));
        assert!(!cap.can_allocate(300, 800, 2_000));

        assert!(cap.enforce(300, 500, 2_000).is_ok());
        assert!(cap.enforce(300, 800, 2_000).is_err());
    }

    #[test]
    fn computes_effective_and_available_from_fields() {
        let cap = CapGroup::builder()
            .absolute_cap(1_000)
            .relative_cap(Wad::one())
            .build();

        assert_eq!(cap.effective_cap(500), 500);
        assert_eq!(cap.available_capacity(300, 500), 200);
    }

    #[test]
    fn record_field_helpers_preserve_unlimited_defaults_and_principal() {
        let mut record = CapGroupRecord {
            cap: CapGroup::builder()
                .absolute_cap(0)
                .relative_cap(Wad::one())
                .build(),
            principal: 123,
        };

        assert_eq!(cap_group_record_absolute_cap(&record), 0);
        assert_eq!(cap_group_record_relative_cap(&record), Wad::one());
        assert_eq!(record.principal, 123);

        set_cap_group_record_absolute_cap(&mut record, 7_500);
        assert_eq!(cap_group_record_absolute_cap(&record), 7_500);
        assert_eq!(cap_group_record_relative_cap(&record), Wad::one());
        assert_eq!(record.principal, 123);

        let three_quarters = Wad::from(WAD * 3 / 4);
        set_cap_group_record_relative_cap(&mut record, three_quarters);
        assert_eq!(cap_group_record_absolute_cap(&record), 7_500);
        assert_eq!(cap_group_record_relative_cap(&record), three_quarters);
        assert_eq!(record.principal, 123);
    }
}

mod policy_cooldown_tests {
    pub use crate::policy::cooldown::*;
    use core::num::NonZeroU64;

    #[test]
    fn test_unlimited_cooldown() {
        let cooldown = Cooldown::unlimited();
        assert!(cooldown.is_unlimited());
        assert!(cooldown.is_ready(0));
        assert!(cooldown.is_ready(u64::MAX));
    }

    #[test]
    fn test_first_operation_always_ready() {
        let cooldown = Cooldown::new(NonZeroU64::new(1000).expect("non-zero interval"));
        assert!(cooldown.is_ready(0));
        assert!(cooldown.is_ready(500));
    }

    #[test]
    fn test_cooldown_enforced() {
        let cooldown = Cooldown::new(NonZeroU64::new(1000).expect("non-zero interval"));
        let cooldown = cooldown.recorded_at(100);

        assert!(!cooldown.is_ready(100));
        assert!(!cooldown.is_ready(500));
        assert!(!cooldown.is_ready(1099));

        assert!(cooldown.is_ready(1100));
        assert!(cooldown.is_ready(2000));
    }

    #[test]
    fn test_check_returns_error() {
        let cooldown =
            Cooldown::new(NonZeroU64::new(1000).expect("non-zero interval")).recorded_at(100);

        let result = cooldown.check(500);
        assert!(matches!(
            result,
            Err(CooldownError::OnCooldown {
                ready_at_ns: 1100,
                remaining_ns: 600,
            })
        ));

        let result = cooldown.check(1100);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ready_at() {
        let cooldown = Cooldown::new(NonZeroU64::new(1000).expect("non-zero interval"));
        assert_eq!(cooldown.ready_at(), None);

        let cooldown = cooldown.recorded_at(100);
        assert_eq!(cooldown.ready_at(), Some(1100));

        let unlimited = Cooldown::unlimited();
        assert_eq!(unlimited.ready_at(), None);
    }

    #[test]
    fn test_remaining() {
        let cooldown =
            Cooldown::new(NonZeroU64::new(1000).expect("non-zero interval")).recorded_at(100);

        assert_eq!(cooldown.remaining(100), 1000);
        assert_eq!(cooldown.remaining(500), 600);
        assert_eq!(cooldown.remaining(1100), 0);
        assert_eq!(cooldown.remaining(2000), 0);
    }

    #[test]
    fn test_record_updates_last_event() {
        let cooldown = Cooldown::new(NonZeroU64::new(1000).expect("non-zero interval"));
        assert_eq!(cooldown.last_event_ns(), None);

        let cooldown = cooldown.recorded_at(500);
        assert_eq!(cooldown.last_event_ns(), Some(500));

        let cooldown = cooldown.recorded_at(1500);
        assert_eq!(cooldown.last_event_ns(), Some(1500));
    }

    #[test]
    fn test_unlimited_state_retains_last_event_after_recording() {
        let unlimited = Cooldown::unlimited();
        let recorded = unlimited.recorded_at(123);

        assert_eq!(recorded.interval_ns(), None);
        assert_eq!(recorded.last_event_ns(), Some(123));
    }

    #[test]
    fn test_interval_ns_reports_finite_or_unlimited_honestly() {
        let unlimited = Cooldown::unlimited();
        assert_eq!(unlimited.interval_ns(), None);

        let cooldown = Cooldown::new(NonZeroU64::new(1000).expect("non-zero interval"));
        assert_eq!(cooldown.interval_ns().map(NonZeroU64::get), Some(1000));
    }
}

mod policy_lock_filter_tests {
    use alloc::vec;

    use crate::policy::market_lock::{LeaseDurationNs, LeaseOwner, MarketLeaseRegistry};
    use crate::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
    use crate::policy::withdraw_route::{WithdrawRoute, WithdrawRouteEntry, WithdrawRouteError};
    use templar_vault_kernel::{TargetId, TimestampNs};

    fn lease_registry_with_target(target_id: TargetId) -> MarketLeaseRegistry {
        let (registry, _) = MarketLeaseRegistry::default()
            .try_acquire(
                target_id,
                LeaseOwner(u64::from(target_id)),
                Some(u64::from(target_id)),
                TimestampNs(1_000),
                LeaseDurationNs(1_000),
            )
            .expect("lease should be acquirable");
        registry
    }

    #[rstest::fixture]
    fn lease_registry_target_1() -> MarketLeaseRegistry {
        lease_registry_with_target(1)
    }

    #[rstest::fixture]
    fn lease_registry_target_2() -> MarketLeaseRegistry {
        lease_registry_with_target(2)
    }

    #[rstest::rstest]
    fn filters_targets(lease_registry_target_2: MarketLeaseRegistry) {
        let lease_registry = lease_registry_target_2;
        let targets = vec![1, 2, 3];
        assert_eq!(
            lease_registry.excluding_leased_targets(&targets, TimestampNs(1_500)),
            vec![1, 3]
        );
    }

    #[rstest::rstest]
    fn excludes_locked_supply_queue_entries_and_preserves_max_length(
        lease_registry_target_2: MarketLeaseRegistry,
    ) {
        let lease_registry = lease_registry_target_2;
        let queue = SupplyQueue::try_from_entries(
            vec![
                SupplyQueueEntry::new(1, 10).unwrap(),
                SupplyQueueEntry::new(2, 20).unwrap(),
                SupplyQueueEntry::new(3, 30).unwrap(),
            ],
            core::num::NonZeroU32::new(16),
        )
        .unwrap();

        let filtered = queue.excluding_leased(&lease_registry, TimestampNs(1_500));

        assert_eq!(
            filtered.max_length().map(core::num::NonZeroU32::get),
            Some(16)
        );
        assert_eq!(filtered.entries().len(), 2);
        assert_eq!(filtered.entries()[0].target_id, 1);
        assert_eq!(filtered.entries()[1].target_id, 3);
    }

    #[rstest::rstest]
    fn excluding_leased_targets_can_invalidate_withdraw_route(
        lease_registry_target_1: MarketLeaseRegistry,
    ) {
        let lease_registry = lease_registry_target_1;
        let route = WithdrawRoute::new(
            vec![
                WithdrawRouteEntry::new(1, 100).expect("valid route entry"),
                WithdrawRouteEntry::new(2, 200).expect("valid route entry"),
            ],
            250,
        )
        .expect("valid route");

        let filtered = route.excluding_leased(&lease_registry, TimestampNs(1_500));

        assert!(matches!(
            filtered,
            Err(WithdrawRouteError::LockedTargetsExcluded { source })
                if matches!(*source, WithdrawRouteError::InsufficientRouteTotal {
                    route_total: 200,
                    target_amount: 250,
                })
        ));
    }

    #[rstest::rstest]
    fn builds_allocation_plan_excluding_leased_targets(
        lease_registry_target_2: MarketLeaseRegistry,
    ) {
        let lease_registry = lease_registry_target_2;
        let queue = SupplyQueue::try_from_entries(
            vec![
                SupplyQueueEntry::new(1, 10).unwrap(),
                SupplyQueueEntry::new(2, 20).unwrap(),
                SupplyQueueEntry::new(3, 30).unwrap(),
            ],
            core::num::NonZeroU32::new(16),
        )
        .unwrap();

        assert_eq!(
            queue
                .to_allocation_plan_excluding_leased(&lease_registry, TimestampNs(1_500))
                .unwrap(),
            vec![(1, 10), (3, 30)]
        );
    }

    #[rstest::rstest]
    fn builds_withdrawal_plan_excluding_leased_targets(
        lease_registry_target_1: MarketLeaseRegistry,
    ) {
        let lease_registry = lease_registry_target_1;
        let route = WithdrawRoute::new(
            vec![
                WithdrawRouteEntry::new(1, 100).expect("valid route entry"),
                WithdrawRouteEntry::new(2, 200).expect("valid route entry"),
                WithdrawRouteEntry::new(3, 300).expect("valid route entry"),
            ],
            450,
        )
        .expect("valid route");

        assert_eq!(
            route
                .to_target_amount_pairs_excluding_leased(&lease_registry, TimestampNs(1_500))
                .expect("filtered route remains satisfiable"),
            vec![(2, 200), (3, 300)]
        );
    }

    #[test]
    fn filtered_withdrawal_plan_errors_when_locks_break_route() {
        let lease_registry = lease_registry_with_target(1);
        let route = WithdrawRoute::new(
            vec![
                WithdrawRouteEntry::new(1, 100).expect("valid route entry"),
                WithdrawRouteEntry::new(2, 200).expect("valid route entry"),
            ],
            250,
        )
        .expect("valid route");

        let result =
            route.to_target_amount_pairs_excluding_leased(&lease_registry, TimestampNs(1_500));

        assert!(matches!(
            result,
            Err(WithdrawRouteError::LockedTargetsExcluded { source })
                if matches!(*source, WithdrawRouteError::InsufficientRouteTotal {
                    route_total: 200,
                    target_amount: 250,
                })
        ));
    }

    #[test]
    fn excluding_leased_preserves_original_route_validation_errors() {
        let invalid_route = WithdrawRoute::new(
            vec![
                WithdrawRouteEntry::new(1, 100).expect("valid route entry"),
                WithdrawRouteEntry::new(1, 200).expect("valid route entry"),
            ],
            250,
        );

        assert!(matches!(
            invalid_route,
            Err(WithdrawRouteError::DuplicateTarget { target_id: 1 })
        ));
    }

    #[rstest::rstest]
    fn filters_refresh_targets(lease_registry_target_2: MarketLeaseRegistry) {
        let lease_registry = lease_registry_target_2;
        let targets = vec![1, 2, 3, 4];

        assert_eq!(
            lease_registry.excluding_leased_targets(&targets, TimestampNs(1_500)),
            vec![1, 3, 4]
        );
    }

    #[rstest::rstest]
    fn reports_unleased_targets(lease_registry_target_2: MarketLeaseRegistry) {
        let lease_registry = lease_registry_target_2;

        assert!(lease_registry.is_unleased(1, TimestampNs(1_500)));
        assert!(!lease_registry.is_unleased(2, TimestampNs(1_500)));
        assert!(lease_registry.is_unleased(3, TimestampNs(1_500)));
    }
}

mod policy_market_lease_tests {
    pub use crate::policy::market_lock::*;

    use templar_vault_kernel::TimestampNs;

    #[rstest::fixture]
    fn empty_registry() -> MarketLeaseRegistry {
        MarketLeaseRegistry::default()
    }

    #[rstest::rstest]
    fn test_new_registry_is_empty(empty_registry: MarketLeaseRegistry) {
        let registry = empty_registry;
        assert!(registry.is_empty());
        assert_eq!(registry.stored_len(), 0);
        assert_eq!(registry.active_len(TimestampNs(0)), 0);
    }

    #[rstest::rstest]
    fn test_acquire_lease_assigns_token(empty_registry: MarketLeaseRegistry) {
        let (registry, lease) = empty_registry
            .try_acquire(
                1,
                LeaseOwner(10),
                Some(10),
                TimestampNs(1_000),
                LeaseDurationNs(500),
            )
            .unwrap();

        assert_eq!(registry.stored_len(), 1);
        assert!(registry.is_leased(1, TimestampNs(1_200)));
        assert_eq!(lease.fencing_token, FencingToken(1));
    }

    #[rstest::rstest]
    fn test_acquire_lease_conflicts_for_different_owner(empty_registry: MarketLeaseRegistry) {
        let (registry, _) = empty_registry
            .try_acquire(
                1,
                LeaseOwner(10),
                Some(10),
                TimestampNs(1_000),
                LeaseDurationNs(500),
            )
            .unwrap();

        let result = registry.try_acquire(
            1,
            LeaseOwner(20),
            Some(20),
            TimestampNs(1_100),
            LeaseDurationNs(500),
        );

        assert!(matches!(
            result,
            Err(AcquireLeaseError::AlreadyLeased { .. })
        ));
    }

    #[rstest::rstest]
    fn test_same_owner_reacquire_refreshes_and_increments_token(
        empty_registry: MarketLeaseRegistry,
    ) {
        let (registry, first_lease) = empty_registry
            .try_acquire(
                1,
                LeaseOwner(10),
                Some(10),
                TimestampNs(1_000),
                LeaseDurationNs(500),
            )
            .unwrap();
        let (registry, second_lease) = registry
            .try_acquire(
                1,
                LeaseOwner(10),
                Some(10),
                TimestampNs(1_100),
                LeaseDurationNs(700),
            )
            .unwrap();

        assert_eq!(first_lease.fencing_token, FencingToken(1));
        assert_eq!(second_lease.fencing_token, FencingToken(2));
        assert!(registry
            .assert_token_current(1, second_lease.fencing_token, TimestampNs(1_200))
            .is_ok());
        assert!(matches!(
            registry.assert_token_current(1, first_lease.fencing_token, TimestampNs(1_200)),
            Err(FencingError::NotCurrent { .. })
        ));
    }

    #[rstest::rstest]
    fn test_expired_lease_can_be_reacquired_by_new_owner(empty_registry: MarketLeaseRegistry) {
        let (registry, _) = empty_registry
            .try_acquire(
                1,
                LeaseOwner(10),
                Some(10),
                TimestampNs(1_000),
                LeaseDurationNs(500),
            )
            .unwrap();

        let (registry, lease) = registry
            .try_acquire(
                1,
                LeaseOwner(20),
                Some(20),
                TimestampNs(1_500),
                LeaseDurationNs(300),
            )
            .unwrap();

        assert!(registry.is_leased_by_owner(1, &LeaseOwner(20), TimestampNs(1_600)));
        assert_eq!(lease.fencing_token, FencingToken(2));
    }

    #[test]
    fn test_zero_ttl_is_rejected() {
        let result = MarketLeaseRegistry::default().try_acquire(
            1,
            LeaseOwner(1),
            Some(1),
            TimestampNs(100),
            LeaseDurationNs(0),
        );

        assert_eq!(result, Err(AcquireLeaseError::ZeroTtl));
    }

    #[test]
    fn test_expiry_overflow_is_rejected() {
        let result = MarketLeaseRegistry::default().try_acquire(
            1,
            LeaseOwner(1),
            Some(1),
            TimestampNs(u64::MAX - 5),
            LeaseDurationNs(10),
        );

        assert_eq!(result, Err(AcquireLeaseError::ExpiryOverflow));
    }

    #[rstest::rstest]
    fn test_owner_checked_release(empty_registry: MarketLeaseRegistry) {
        let (registry, _) = empty_registry
            .try_acquire(
                1,
                LeaseOwner(10),
                Some(10),
                TimestampNs(1_000),
                LeaseDurationNs(500),
            )
            .unwrap();

        let error = registry.release_if_owned(1, &LeaseOwner(20)).unwrap_err();
        assert!(matches!(error, ReleaseLeaseError::OwnerMismatch { .. }));

        let released = registry.release_if_owned(1, &LeaseOwner(10)).unwrap();
        assert!(!released.is_leased(1, TimestampNs(1_100)));

        let token_error = registry
            .release_if_owned_with_token(1, &LeaseOwner(10), FencingToken(999))
            .unwrap_err();
        assert!(matches!(
            token_error,
            ReleaseLeaseError::TokenMismatch { .. }
        ));

        let (registry, lease) = empty_registry
            .try_acquire(
                1,
                LeaseOwner(10),
                Some(10),
                TimestampNs(1_000),
                LeaseDurationNs(500),
            )
            .unwrap();
        let released = registry
            .release_if_owned_with_token(1, &LeaseOwner(10), lease.fencing_token)
            .unwrap();
        assert!(!released.is_leased(1, TimestampNs(1_100)));
    }

    #[test]
    fn test_force_release_by_op() {
        let (registry, _) = MarketLeaseRegistry::default()
            .try_acquire(
                1,
                LeaseOwner(10),
                Some(10),
                TimestampNs(1_000),
                LeaseDurationNs(500),
            )
            .unwrap();
        let (registry, _) = registry
            .try_acquire(
                2,
                LeaseOwner(20),
                Some(20),
                TimestampNs(1_000),
                LeaseDurationNs(500),
            )
            .unwrap();

        let cleaned = registry.force_release_by_op(10);
        assert!(!cleaned.is_leased(1, TimestampNs(1_100)));
        assert!(cleaned.is_leased(2, TimestampNs(1_100)));
    }

    #[test]
    fn test_cleanup_expired_leases() {
        let (registry, _) = MarketLeaseRegistry::default()
            .try_acquire(
                1,
                LeaseOwner(10),
                Some(10),
                TimestampNs(1_000),
                LeaseDurationNs(500),
            )
            .unwrap();
        let (registry, _) = registry
            .try_acquire(
                2,
                LeaseOwner(20),
                Some(20),
                TimestampNs(1_000),
                LeaseDurationNs(2_000),
            )
            .unwrap();

        let cleaned = registry.cleanup_expired(TimestampNs(1_600));
        assert!(!cleaned.is_leased(1, TimestampNs(1_600)));
        assert!(cleaned.is_leased(2, TimestampNs(1_600)));
    }

    #[test]
    fn test_clear_registry() {
        let (registry, _) = MarketLeaseRegistry::default()
            .try_acquire(
                1,
                LeaseOwner(10),
                Some(10),
                TimestampNs(1_000),
                LeaseDurationNs(500),
            )
            .unwrap();

        assert!(registry.clear().is_empty());
    }
}

mod policy_refresh_plan_tests {
    pub use crate::policy::refresh_plan::*;

    use crate::policy::target_set::find_first_duplicate;
    use alloc::vec;
    use alloc::vec::Vec;
    use templar_vault_kernel::{DurationNs, TargetId, TimestampNs};

    #[test]
    fn test_new_plan() {
        let plan = RefreshPlan::new(vec![1, 2, 3]).unwrap();
        assert_eq!(plan.len(), 3);
        assert_eq!(plan.targets(), [1, 2, 3]);
    }

    #[test]
    fn test_new_plan_rejects_empty_targets() {
        let result = RefreshPlan::new(vec![]);
        assert!(matches!(result, Err(RefreshPlanError::EmptyPlan)));
    }

    #[test]
    fn test_new_plan_rejects_duplicate_targets() {
        let plan = RefreshPlan::new(vec![1, 2, 1]);
        assert!(matches!(
            plan,
            Err(RefreshPlanError::DuplicateTarget { target_id: 1 })
        ));
    }

    #[test]
    fn test_check_refresh_cooldown_no_cooldown() {
        let throttle = RefreshThrottle::new(DurationNs::ZERO, None);
        assert!(throttle.check(TimestampNs(1000)).is_ok());
        assert!(throttle.is_ready(TimestampNs(1000)));
    }

    #[test]
    fn test_check_refresh_cooldown_first_refresh() {
        let throttle = RefreshThrottle::new(DurationNs(1000), None);
        assert!(throttle.check(TimestampNs(100)).is_ok());
        assert!(throttle.is_ready(TimestampNs(100)));
    }

    #[test]
    fn test_check_refresh_cooldown_on_cooldown() {
        let throttle = RefreshThrottle::new(DurationNs(1000), Some(TimestampNs(100)));
        let result = throttle.check(TimestampNs(600));
        assert!(matches!(result, Err(RefreshPlanError::OnCooldown { .. })));
        assert!(!throttle.is_ready(TimestampNs(600)));
    }

    #[test]
    fn test_check_refresh_cooldown_after_cooldown() {
        let throttle = RefreshThrottle::new(DurationNs(1000), Some(TimestampNs(100)));
        assert!(throttle.check(TimestampNs(1200)).is_ok());
        assert!(throttle.is_ready(TimestampNs(1200)));
    }

    #[test]
    fn test_with_cooldown_preserves_last_refresh_timestamp() {
        let throttle = RefreshThrottle::new(DurationNs(200), Some(TimestampNs(50)));

        assert_eq!(throttle.last_refresh_at(), Some(TimestampNs(50)));
        assert_eq!(throttle.cooldown_duration(), DurationNs(200));
    }

    #[test]
    fn test_zero_cooldown_maps_to_unlimited() {
        let throttle = RefreshThrottle::new(DurationNs::ZERO, None);

        assert!(throttle.cooldown().is_unlimited());
        assert_eq!(throttle.cooldown_duration(), DurationNs::ZERO);
        assert_eq!(throttle.last_refresh_at(), None);
    }

    #[test]
    fn test_build_refresh_plan() {
        let enabled = vec![1, 2, 3];
        let plan = build_refresh_plan(&enabled).unwrap();

        assert_eq!(plan.targets(), [1, 2, 3]);
    }

    #[test]
    fn test_build_refresh_plan_empty() {
        let enabled: Vec<TargetId> = vec![];
        let result = build_refresh_plan(&enabled);

        assert!(matches!(result, Err(RefreshPlanError::EmptyPlan)));
    }

    #[test]
    fn test_build_targeted_refresh_plan() {
        let enabled = vec![1, 2, 3, 4];
        let targets = vec![2, 4];

        let plan = build_targeted_refresh_plan(&targets, &enabled).unwrap();

        assert_eq!(plan.targets(), [2, 4]);
    }

    #[test]
    fn test_build_targeted_refresh_plan_invalid_target() {
        let enabled = vec![1, 2, 3];
        let targets = vec![2, 5]; // 5 is not enabled

        let result = build_targeted_refresh_plan(&targets, &enabled);

        assert!(matches!(
            result,
            Err(RefreshPlanError::TargetNotFound { target_id: 5 })
        ));
    }

    #[test]
    fn test_build_targeted_refresh_plan_duplicate() {
        let enabled = vec![1, 2, 3];
        let targets = vec![1, 2, 1]; // duplicate 1

        let result = build_targeted_refresh_plan(&targets, &enabled);

        assert!(matches!(
            result,
            Err(RefreshPlanError::DuplicateTarget { target_id: 1 })
        ));
    }

    #[test]
    fn test_build_targeted_refresh_plan_duplicate_precedes_missing_target() {
        let enabled = vec![1, 2, 3];
        let targets = vec![1, 4, 1];

        let result = build_targeted_refresh_plan(&targets, &enabled);

        assert!(matches!(
            result,
            Err(RefreshPlanError::DuplicateTarget { target_id: 1 })
        ));
    }

    #[test]
    fn test_record_refresh_completion() {
        let throttle = RefreshThrottle::new(DurationNs(1000), None);
        let updated = throttle.record_completion(TimestampNs(5000));

        assert_eq!(updated.last_refresh_at(), Some(TimestampNs(5000)));
        assert_eq!(updated.cooldown_duration(), DurationNs(1000));
    }

    #[test]
    fn test_filter_stale_targets() {
        let targets = vec![
            RefreshTargetStatus::new(1, Some(TimestampNs(1000))),
            RefreshTargetStatus::new(2, Some(TimestampNs(500))),
            RefreshTargetStatus::new(3, Some(TimestampNs(2000))),
        ];

        let stale = filter_stale_targets(&targets, DurationNs(1500), TimestampNs(3000)).unwrap();

        assert_eq!(stale.len(), 2);
        assert!(stale.contains(&1));
        assert!(stale.contains(&2));
        assert!(!stale.contains(&3));
    }

    #[test]
    fn test_filter_stale_targets_includes_never_refreshed() {
        let targets = vec![
            RefreshTargetStatus::new(1, None),
            RefreshTargetStatus::new(2, Some(TimestampNs(2_000))),
        ];

        let stale = filter_stale_targets(&targets, DurationNs(1_500), TimestampNs(3_000)).unwrap();

        assert_eq!(stale, vec![1]);
    }

    #[test]
    fn test_filter_stale_targets_rejects_future_timestamp() {
        let targets = vec![RefreshTargetStatus::new(7, Some(TimestampNs(4_000)))];

        let result = filter_stale_targets(&targets, DurationNs(1_500), TimestampNs(3_000));

        assert!(matches!(
            result,
            Err(RefreshPlanError::FutureRefreshTimestamp {
                target_id: 7,
                last_refresh_at: TimestampNs(4_000),
                current_time: TimestampNs(3_000),
            })
        ));
    }

    #[test]
    fn test_build_stale_refresh_plan_returns_none_when_nothing_is_stale() {
        let targets = vec![
            RefreshTargetStatus::new(1, Some(TimestampNs(2_000))),
            RefreshTargetStatus::new(2, Some(TimestampNs(2_500))),
        ];

        let plan =
            build_stale_refresh_plan(&targets, DurationNs(1_500), TimestampNs(3_000), &[1, 2])
                .unwrap();

        assert!(plan.is_none());
    }

    #[test]
    fn test_into_targets() {
        let plan = RefreshPlan::new(vec![5, 3, 1]).unwrap();
        let list = plan.into_targets();
        assert_eq!(list, vec![5, 3, 1]);
    }

    #[test]
    fn test_find_first_duplicate_shared_helper() {
        assert_eq!(find_first_duplicate(&[1, 2, 3]), None);
        assert_eq!(find_first_duplicate(&[1, 2, 1]), Some(1));
        assert_eq!(find_first_duplicate(&[1, 2, 2, 3]), Some(2));
        assert_eq!(find_first_duplicate::<i32>(&[]), None);
    }
}

mod policy_state_tests {
    pub use crate::policy::state::*;

    use crate::policy::cap_group::CapGroupId;
    use crate::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
    use alloc::string::String;
    use alloc::vec;

    #[test]
    fn external_assets_sums_principals() {
        let mut state = PolicyState::default();
        state.set_market_config(1, MarketConfig::default()).unwrap();
        state.set_market_config(2, MarketConfig::default()).unwrap();
        state.set_market_config(3, MarketConfig::default()).unwrap();
        state.set_principal(1, 100).unwrap();
        state.set_principal(2, 250).unwrap();
        state.set_principal(3, 50).unwrap();

        assert_eq!(state.external_assets().unwrap(), 400);
    }

    #[test]
    fn cap_group_totals_aggregate_by_group() {
        let mut state = PolicyState::default();
        let group_a = CapGroupId::try_from("group-a").unwrap();
        let group_b = CapGroupId::try_from("group-b").unwrap();

        state.ensure_cap_group(group_a.clone());
        state.ensure_cap_group(group_b.clone());
        state
            .set_market_config(1, MarketConfig::new(true, 0, Some(group_a.clone())))
            .unwrap();
        state
            .set_market_config(2, MarketConfig::new(true, 0, Some(group_a.clone())))
            .unwrap();
        state
            .set_market_config(3, MarketConfig::new(true, 0, Some(group_b.clone())))
            .unwrap();

        state.set_principal(1, 10).unwrap();
        state.set_principal(2, 20).unwrap();
        state.set_principal(3, 40).unwrap();

        let totals = state.compute_cap_group_totals().unwrap();
        let total_for = |group_id: &CapGroupId| {
            totals
                .iter()
                .find(|(candidate, _)| candidate == group_id)
                .map(|(_, total)| *total)
                .unwrap_or(0)
        };
        assert_eq!(total_for(&group_a), 30);
        assert_eq!(total_for(&group_b), 40);
    }

    #[test]
    fn recompute_cap_group_principals_updates_records() {
        let mut state = PolicyState::default();
        let group = CapGroupId::try_from(String::from("group")).unwrap();
        state.ensure_cap_group(group.clone());
        state
            .set_market_config(1, MarketConfig::new(true, 0, Some(group.clone())))
            .unwrap();
        state.set_principal(1, 123).unwrap();

        state.recompute_cap_group_principals().unwrap();

        let record = state.cap_groups().get(&group).expect("cap group");
        assert_eq!(record.principal, 123);
    }

    #[test]
    fn set_principal_requires_known_market() {
        let mut state = PolicyState::default();

        let err = state.set_principal(7, 1).unwrap_err();

        assert_eq!(err, PolicyStateError::UnknownMarket { target_id: 7 });
    }

    #[test]
    fn set_market_config_rejects_unknown_cap_group() {
        let mut state = PolicyState::default();
        let missing_group = CapGroupId::try_from("missing").unwrap();

        let err = state
            .set_market_config(1, MarketConfig::new(true, 0, Some(missing_group.clone())))
            .unwrap_err();

        assert_eq!(err, PolicyStateError::UnknownCapGroup { id: missing_group });
    }

    #[test]
    fn remove_market_prunes_its_principal_and_group_total() {
        let mut state = PolicyState::default();
        let group = CapGroupId::try_from("group").unwrap();
        state.ensure_cap_group(group.clone());
        state
            .set_market_config(1, MarketConfig::new(true, 100, Some(group.clone())))
            .unwrap();
        state.set_principal(1, 25).unwrap();
        state
            .replace_supply_queue(
                SupplyQueue::try_from_entries(vec![SupplyQueueEntry::new(1, 1).unwrap()], None)
                    .unwrap(),
            )
            .unwrap();

        let removed = state.remove_market(1).unwrap();

        assert!(removed.is_some());
        assert_eq!(state.principal_for(1), None);
        assert!(state.market_config(1).is_none());
        assert!(state.supply_queue().is_empty());
        assert_eq!(state.cap_group(&group).expect("cap group").principal, 0);
    }

    #[test]
    fn prune_zero_principals_removes_zero_entries_for_known_markets() {
        let mut state = PolicyState::default();
        state.set_market_config(1, MarketConfig::default()).unwrap();
        state.set_market_config(2, MarketConfig::default()).unwrap();
        state.set_principal(1, 10).unwrap();

        state.prune_zero_principals();

        assert_eq!(state.principal_for(1), Some(10));
        assert_eq!(state.principal_for(2), None);
    }

    #[test]
    fn prune_unused_cap_groups_removes_unreferenced_groups() {
        let mut state = PolicyState::default();
        let used_group = CapGroupId::try_from("used").unwrap();
        let unused_group = CapGroupId::try_from("unused").unwrap();
        state.ensure_cap_group(used_group.clone());
        state.ensure_cap_group(unused_group.clone());
        state
            .set_market_config(1, MarketConfig::new(true, 0, Some(used_group.clone())))
            .unwrap();

        state.prune_unused_cap_groups();

        assert!(state.cap_group(&used_group).is_some());
        assert!(state.cap_group(&unused_group).is_none());
    }

    #[test]
    fn remove_cap_group_rejects_groups_still_in_use() {
        let mut state = PolicyState::default();
        let group = CapGroupId::try_from("group").unwrap();
        state.ensure_cap_group(group.clone());
        state
            .set_market_config(1, MarketConfig::new(true, 0, Some(group.clone())))
            .unwrap();

        let err = state.remove_cap_group(&group).unwrap_err();

        assert_eq!(err, PolicyStateError::CapGroupInUse { id: group });
    }

    #[test]
    fn replace_supply_queue_rejects_unknown_market_targets() {
        let mut state = PolicyState::default();
        state.set_market_config(1, MarketConfig::default()).unwrap();

        let queue = SupplyQueue::try_from_entries(vec![SupplyQueueEntry::new(9, 1).unwrap()], None)
            .unwrap();

        let err = state.replace_supply_queue(queue).unwrap_err();

        assert_eq!(
            err,
            PolicyStateError::SupplyQueueUnknownMarket { target_id: 9 }
        );
    }

    #[test]
    fn replace_supply_queue_rejects_disabled_market_targets() {
        let mut state = PolicyState::default();
        state.set_market_config(1, MarketConfig::default()).unwrap();
        state.set_market_enabled(1, false).unwrap();

        let queue = SupplyQueue::try_from_entries(vec![SupplyQueueEntry::new(1, 1).unwrap()], None)
            .unwrap();

        let err = state.replace_supply_queue(queue).unwrap_err();

        assert_eq!(
            err,
            PolicyStateError::SupplyQueueDisabledMarket { target_id: 1 }
        );
    }

    #[test]
    fn replace_supply_queue_rejects_zero_cap_market_targets() {
        let mut state = PolicyState::default();
        state.set_market_config(1, MarketConfig::default()).unwrap();

        let queue = SupplyQueue::try_from_entries(vec![SupplyQueueEntry::new(1, 1).unwrap()], None)
            .unwrap();

        let err = state.replace_supply_queue(queue).unwrap_err();

        assert_eq!(
            err,
            PolicyStateError::SupplyQueueUnauthorizedMarket { target_id: 1 }
        );
    }

    #[test]
    fn from_parts_rejects_invalid_supply_queue_targets() {
        let markets = OrderedMap::from_iter([(1, MarketConfig::new(true, 5, None))]);
        let principals = OrderedMap::default();
        let cap_groups = OrderedMap::default();
        let leases = crate::MarketLeaseRegistry::default();
        let supply_queue =
            SupplyQueue::try_from_entries(vec![SupplyQueueEntry::new(2, 1).unwrap()], None)
                .unwrap();

        let err = PolicyState::from_parts(markets, principals, cap_groups, leases, supply_queue)
            .unwrap_err();

        assert_eq!(
            err,
            PolicyStateError::SupplyQueueUnknownMarket { target_id: 2 }
        );
    }

    #[test]
    fn disabling_market_prunes_supply_queue_target() {
        let mut state = PolicyState::default();
        state
            .set_market_config(1, MarketConfig::new(true, 5, None))
            .unwrap();
        state
            .replace_supply_queue(
                SupplyQueue::try_from_entries(vec![SupplyQueueEntry::new(1, 1).unwrap()], None)
                    .unwrap(),
            )
            .unwrap();

        state.set_market_enabled(1, false).unwrap();

        assert!(state.supply_queue().is_empty());
    }

    #[test]
    fn zero_cap_prunes_supply_queue_target() {
        let mut state = PolicyState::default();
        state
            .set_market_config(1, MarketConfig::new(true, 5, None))
            .unwrap();
        state
            .replace_supply_queue(
                SupplyQueue::try_from_entries(vec![SupplyQueueEntry::new(1, 1).unwrap()], None)
                    .unwrap(),
            )
            .unwrap();

        state.set_market_cap(1, 0).unwrap();

        assert!(state.supply_queue().is_empty());
    }
}

mod policy_supply_queue_tests {
    pub use crate::policy::supply_queue::*;
    use core::num::NonZeroU32;

    #[rstest::fixture]
    fn empty_queue() -> SupplyQueue {
        SupplyQueue::default()
    }

    #[rstest::fixture]
    fn queue_two_entries(mut empty_queue: SupplyQueue) -> SupplyQueue {
        empty_queue
            .enqueue(SupplyQueueEntry::new(1, 100).unwrap())
            .unwrap();
        empty_queue
            .enqueue(SupplyQueueEntry::new(2, 200).unwrap())
            .unwrap();
        empty_queue
    }

    #[rstest::fixture]
    fn queue_with_repeated_target(mut empty_queue: SupplyQueue) -> SupplyQueue {
        empty_queue
            .enqueue(SupplyQueueEntry::new(1, 100).unwrap())
            .unwrap();
        empty_queue
            .enqueue(SupplyQueueEntry::new(2, 200).unwrap())
            .unwrap();
        empty_queue
            .enqueue(SupplyQueueEntry::new(1, 50).unwrap())
            .unwrap();
        empty_queue
    }

    #[rstest::rstest]
    fn test_new_queue_is_empty(empty_queue: SupplyQueue) {
        let queue = empty_queue;
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert!(!queue.is_full());
    }

    #[rstest::rstest]
    fn test_enqueue_supply(mut empty_queue: SupplyQueue) {
        let entry = SupplyQueueEntry::new(1, 100).unwrap();

        empty_queue.enqueue(entry.clone()).unwrap();

        assert_eq!(empty_queue.len(), 1);
        assert_eq!(*empty_queue.entries()[0], entry);
    }

    #[rstest::rstest]
    fn test_enqueue_zero_amount_error() {
        let result = SupplyQueueEntry::new(1, 0);

        assert!(matches!(result, Err(SupplyQueueError::ZeroAmount)));
    }

    #[test]
    fn test_enqueue_full_queue_error() {
        let mut queue = SupplyQueue::bounded(NonZeroU32::new(2).unwrap());
        let entry1 = SupplyQueueEntry::new(1, 100).unwrap();
        let entry2 = SupplyQueueEntry::new(2, 200).unwrap();
        let entry3 = SupplyQueueEntry::new(3, 300).unwrap();

        queue.enqueue(entry1).unwrap();
        queue.enqueue(entry2).unwrap();
        let result = queue.enqueue(entry3);

        assert!(matches!(
            result,
            Err(SupplyQueueError::QueueFull { max_length: 2 })
        ));
    }

    #[rstest::rstest]
    fn test_enqueue_with_priority(mut empty_queue: SupplyQueue) {
        let low = SupplyQueueEntry::new_with_priority(1, 100, 0).unwrap();
        let high = SupplyQueueEntry::new_with_priority(2, 200, 10).unwrap();
        let medium = SupplyQueueEntry::new_with_priority(3, 300, 5).unwrap();

        empty_queue.enqueue(low).unwrap();
        empty_queue.enqueue(high).unwrap();
        empty_queue.enqueue(medium).unwrap();

        let entries = empty_queue.entries();
        assert_eq!(entries[0].target_id, 2);
        assert_eq!(entries[1].target_id, 3);
        assert_eq!(entries[2].target_id, 1);
    }

    #[rstest::rstest]
    fn test_dequeue_supply(mut queue_two_entries: SupplyQueue) {
        let dequeued = queue_two_entries.dequeue().unwrap();

        assert_eq!(dequeued.target_id, 1);
        assert_eq!(dequeued.amount, 100);
        assert_eq!(queue_two_entries.len(), 1);
    }

    #[rstest::rstest]
    fn test_dequeue_empty_error(mut empty_queue: SupplyQueue) {
        let result = empty_queue.dequeue();

        assert!(matches!(result, Err(SupplyQueueError::QueueEmpty)));
    }

    #[rstest::rstest]
    fn test_peek(mut empty_queue: SupplyQueue) {
        assert!(empty_queue.peek().is_none());

        let entry = SupplyQueueEntry::new(1, 100).unwrap();
        empty_queue.enqueue(entry.clone()).unwrap();

        assert_eq!(empty_queue.peek(), Some(&entry));
        assert_eq!(empty_queue.len(), 1);
    }

    #[rstest::rstest]
    fn test_compute_queue_total(queue_with_repeated_target: SupplyQueue) {
        let queue = queue_with_repeated_target;
        assert_eq!(queue.total().unwrap(), 350);
    }

    #[rstest::rstest]
    fn test_compute_queue_totals_by_target(queue_with_repeated_target: SupplyQueue) {
        let queue = queue_with_repeated_target;
        let totals = queue.totals_by_target().unwrap();

        assert_eq!(totals.len(), 2);
        let total_for = |target_id| {
            totals
                .iter()
                .find(|(candidate, _)| *candidate == target_id)
                .map(|(_, total)| *total)
        };
        assert_eq!(total_for(1), Some(150));
        assert_eq!(total_for(2), Some(200));
    }

    #[rstest::rstest]
    fn test_remove_target_entries(mut queue_with_repeated_target: SupplyQueue) {
        queue_with_repeated_target.remove_target(1);

        assert_eq!(queue_with_repeated_target.len(), 1);
        assert_eq!(queue_with_repeated_target.entries()[0].target_id, 2);
    }

    #[rstest::rstest]
    fn test_drain_queue(mut queue_two_entries: SupplyQueue) {
        let entries = queue_two_entries.drain();

        assert!(queue_two_entries.is_empty());
        assert_eq!(entries.len(), 2);
    }

    #[rstest::rstest]
    fn test_to_allocation_plan(queue_with_repeated_target: SupplyQueue) {
        let queue = queue_with_repeated_target;
        let plan = queue.to_allocation_plan().unwrap();

        assert_eq!(plan.len(), 2);
        assert_eq!(plan, alloc::vec![(1, 150), (2, 200)]);
    }

    #[rstest::rstest]
    fn test_total_for_target(queue_with_repeated_target: SupplyQueue) {
        let queue = queue_with_repeated_target;
        assert_eq!(queue.total_for_target(1).unwrap(), 150);
        assert_eq!(queue.total_for_target(2).unwrap(), 200);
        assert_eq!(queue.total_for_target(3).unwrap(), 0);
    }

    #[rstest::rstest]
    fn test_has_target(mut empty_queue: SupplyQueue) {
        let entry = SupplyQueueEntry::new(1, 100).unwrap();
        empty_queue.enqueue(entry).unwrap();

        assert!(empty_queue.has_target(1));
        assert!(!empty_queue.has_target(2));
    }

    #[test]
    fn test_entry_construction_with_priority() {
        let entry = SupplyQueueEntry::new_with_priority(1, 100, 5).unwrap();

        assert_eq!(entry.target_id, 1);
        assert_eq!(entry.amount, 100);
        assert_eq!(entry.priority, 5);
    }
}

mod policy_target_set_tests {
    use crate::policy::{
        market_lock::{LeaseDurationNs, LeaseOwner, MarketLeaseRegistry},
        refresh_plan::RefreshTiming,
        target_set::{
            build_refresh_plan_from_targets, build_withdraw_capacity_pairs_from_target_principals,
        },
        target_set::{find_first_duplicate, has_unique_items},
    };

    use alloc::vec;
    use templar_vault_kernel::{DurationNs, TimestampNs};

    #[test]
    fn finds_first_duplicate() {
        assert_eq!(find_first_duplicate(&[1u32, 2, 3]), None);
        assert_eq!(find_first_duplicate(&[1u32, 2, 1]), Some(1));
        assert_eq!(find_first_duplicate(&[1u32, 2, 2, 3]), Some(2));
    }

    #[test]
    fn validates_uniqueness() {
        assert!(has_unique_items(&[1u32, 2, 3]));
        assert!(!has_unique_items(&[1u32, 2, 1]));
    }

    #[test]
    fn reports_duplicate_target_ids() {
        assert_eq!(find_first_duplicate(&[1, 2, 3]), None);
        assert_eq!(find_first_duplicate(&[1, 2, 1]), Some(1));
    }

    #[test]
    fn builds_withdraw_capacity_pairs_from_target_principals() {
        let principals = vec![(1, 100), (2, 200), (3, 300)];
        let plan = build_withdraw_capacity_pairs_from_target_principals(&principals, 250).unwrap();

        assert_eq!(plan, vec![(3, 300), (2, 200), (1, 100)]);
    }

    #[test]
    fn lease_registry_queries_stay_on_registry() {
        let owner = LeaseOwner(7);
        let (registry, _) = MarketLeaseRegistry::default()
            .try_acquire(2, owner, None, 1_000.into(), LeaseDurationNs(500))
            .unwrap();

        let targets = vec![1, 2, 3];
        assert_eq!(
            registry.find_leased_targets(&targets, 1_250.into()),
            vec![2]
        );
        assert!(registry.is_leased(2, 1_250.into()));
        assert!(!registry.is_leased(1, 1_250.into()));
        assert_eq!(registry.leased_targets(1_250.into()), vec![2]);
    }

    #[test]
    fn builds_refresh_plan_from_targets() {
        let (plan, throttle) =
            build_refresh_plan_from_targets(&[1, 2, 3], DurationNs(100), Some(TimestampNs(50)))
                .unwrap();
        assert_eq!(plan.targets(), [1, 2, 3]);
        assert_eq!(throttle.cooldown_duration(), DurationNs(100));
        assert_eq!(throttle.last_refresh_at(), Some(TimestampNs(50)));
    }

    #[test]
    fn builds_named_refresh_execution_plan() {
        let refresh_execution_plan = crate::policy::target_set::refresh_plan(
            &[1, 2, 3],
            DurationNs(100),
            Some(TimestampNs(50)),
        )
        .unwrap();

        assert_eq!(refresh_execution_plan.plan().targets(), [1, 2, 3]);
        assert_eq!(
            refresh_execution_plan.throttle().cooldown_duration(),
            DurationNs(100)
        );
        assert_eq!(
            refresh_execution_plan.throttle().last_refresh_at(),
            Some(TimestampNs(50))
        );
    }

    #[test]
    fn builds_refresh_execution_plan_with_timing() {
        let refresh_execution_plan = crate::policy::target_set::refresh_plan_with_timing(
            &[1, 2, 3],
            RefreshTiming::new(DurationNs(100), Some(TimestampNs(50))),
        )
        .unwrap();

        assert_eq!(refresh_execution_plan.plan().targets(), [1, 2, 3]);
        assert_eq!(
            refresh_execution_plan.throttle().cooldown_duration(),
            DurationNs(100)
        );
        assert_eq!(
            refresh_execution_plan.throttle().last_refresh_at(),
            Some(TimestampNs(50))
        );
    }
}

mod policy_withdraw_route_tests {
    pub use crate::policy::withdraw_route::*;

    use alloc::vec;

    fn route_entry(target_id: u32, max_amount: u128) -> WithdrawRouteEntry {
        WithdrawRouteEntry::new(target_id, max_amount).unwrap()
    }

    fn route_entry_with_liquidity(
        target_id: u32,
        max_amount: u128,
        available_liquidity: u128,
    ) -> WithdrawRouteEntry {
        WithdrawRouteEntry::new(target_id, max_amount)
            .unwrap()
            .with_liquidity(available_liquidity)
            .unwrap()
    }

    #[rstest::fixture]
    fn valid_route() -> WithdrawRoute {
        WithdrawRoute::new(
            vec![
                route_entry(1, 500),
                route_entry(2, 300),
                route_entry(3, 200),
            ],
            1000,
        )
        .unwrap()
    }

    #[rstest::fixture]
    fn two_entry_route() -> WithdrawRoute {
        WithdrawRoute::new(vec![route_entry(1, 500), route_entry(2, 300)], 800).unwrap()
    }

    #[test]
    fn test_new_route() {
        let route = WithdrawRoute::new(vec![route_entry(1, 1000)], 1000).unwrap();

        assert!(!route.is_empty());
        assert_eq!(route.target_amount(), 1000);
    }

    #[test]
    fn test_builder_pattern() {
        let route =
            WithdrawRoute::new(vec![route_entry(1, 500), route_entry(2, 600)], 1000).unwrap();

        assert_eq!(route.len(), 2);
        assert_eq!(route.checked_total().unwrap(), 1100);
    }

    #[test]
    fn test_entry_builder() {
        let entry = route_entry_with_liquidity(1, 400, 400);

        assert_eq!(entry.target_id, 1);
        assert_eq!(entry.max_amount, 400);
        assert_eq!(entry.available_liquidity, Some(400));
    }

    #[rstest::rstest]
    fn test_compute_route_total(valid_route: WithdrawRoute) {
        let route = valid_route;
        assert_eq!(route.checked_total().unwrap(), 1000);
    }

    #[test]
    fn test_route_total_overflow_is_error() {
        let route =
            WithdrawRoute::new(vec![route_entry(1, u128::MAX), route_entry(2, 1)], 1).unwrap();

        assert!(matches!(
            route.checked_total(),
            Err(WithdrawRouteError::AmountOverflow)
        ));
    }

    #[test]
    fn test_known_available_liquidity_overflow_is_error() {
        let route = WithdrawRoute::new(
            vec![
                route_entry_with_liquidity(1, 1, u128::MAX),
                route_entry_with_liquidity(2, 1, 1),
            ],
            1,
        )
        .unwrap();

        assert!(matches!(
            route.known_available_liquidity(),
            Err(WithdrawRouteError::AmountOverflow)
        ));
    }

    #[test]
    fn test_validate_withdraw_route_success() {
        let route =
            WithdrawRoute::new(vec![route_entry(1, 500), route_entry(2, 600)], 1000).unwrap();
        assert!(route.validate().is_ok());
    }

    #[test]
    fn test_validate_withdraw_route_zero_target() {
        let route = WithdrawRoute::new(vec![route_entry(1, 500)], 0);

        assert!(matches!(route, Err(WithdrawRouteError::ZeroTargetAmount)));
    }

    #[test]
    fn test_validate_withdraw_route_empty() {
        let route = WithdrawRoute::new(vec![], 1000);

        assert!(matches!(route, Err(WithdrawRouteError::EmptyRoute)));
    }

    #[test]
    fn test_validate_withdraw_route_insufficient() {
        let route = WithdrawRoute::new(vec![route_entry(1, 500)], 1000);

        assert!(matches!(
            route,
            Err(WithdrawRouteError::InsufficientRouteTotal { .. })
        ));
    }

    #[test]
    fn test_validate_withdraw_route_duplicate() {
        let route = WithdrawRoute::new(vec![route_entry(1, 500), route_entry(1, 600)], 1000);

        assert!(matches!(
            route,
            Err(WithdrawRouteError::DuplicateTarget { target_id: 1 })
        ));
    }

    #[test]
    fn test_entry_zero_max_is_rejected() {
        let entry = WithdrawRouteEntry::new(2, 0);

        assert!(matches!(
            entry,
            Err(WithdrawRouteError::ZeroMaxAmount { target_id: 2 })
        ));
    }

    #[test]
    fn test_entry_liquidity_less_than_max_is_rejected() {
        let entry = WithdrawRouteEntry::new(2, 100).unwrap().with_liquidity(50);

        assert!(matches!(
            entry,
            Err(WithdrawRouteError::LiquidityLessThanMaxAmount {
                target_id: 2,
                max_amount: 100,
                available_liquidity: 50,
            })
        ));
    }

    #[test]
    fn test_build_withdraw_route() {
        let principals = vec![(1, 1000), (2, 500), (3, 300)];

        let route = build_withdraw_route(&principals, 800).unwrap();

        assert_eq!(route.entries()[0].target_id, 1);
        assert_eq!(route.entries()[1].target_id, 2);
        assert_eq!(route.entries()[2].target_id, 3);
        assert_eq!(route.target_amount(), 800);
    }

    #[test]
    fn test_build_withdraw_route_tie_breaker() {
        let principals = vec![(2, 1000), (1, 1000), (3, 500)];

        let route = build_withdraw_route(&principals, 100).unwrap();

        assert_eq!(route.entries()[0].target_id, 1);
        assert_eq!(route.entries()[1].target_id, 2);
        assert_eq!(route.entries()[2].target_id, 3);
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
    fn test_build_withdraw_route_caps_satisfiability_without_overflow() {
        let route = build_withdraw_route(&[(1, u128::MAX), (2, 1)], 1).unwrap();

        assert_eq!(route.entries()[0].target_id, 1);
        assert!(route.can_satisfy());
    }

    #[test]
    fn test_build_withdraw_route_with_liquidity() {
        let market_data = vec![(1, 1000, 800), (2, 500, 500), (3, 300, 100)];

        let route = build_withdraw_route_with_liquidity(&market_data, 500).unwrap();

        assert_eq!(route.entries()[0].target_id, 1);
        assert_eq!(route.entries()[0].max_amount, 800);
        assert_eq!(route.entries()[0].available_liquidity, Some(800));
    }

    #[test]
    fn test_build_withdraw_route_with_liquidity_tie_breaker() {
        let market_data = vec![(2, 1000, 500), (1, 200, 500), (3, 300, 400)];

        let route = build_withdraw_route_with_liquidity(&market_data, 100).unwrap();

        assert_eq!(route.entries()[0].target_id, 2);
        assert_eq!(route.entries()[1].target_id, 3);
        assert_eq!(route.entries()[2].target_id, 1);
    }

    #[rstest::rstest]
    fn test_compute_known_available_liquidity(valid_route: WithdrawRoute) {
        let route = WithdrawRoute::new(
            valid_route
                .entries()
                .iter()
                .cloned()
                .map(|entry| match entry.target_id {
                    1 => entry.with_liquidity(500).unwrap(),
                    3 => entry.with_liquidity(200).unwrap(),
                    _ => entry,
                })
                .collect(),
            1000,
        )
        .unwrap();
        assert_eq!(route.known_available_liquidity().unwrap(), None);
    }

    #[rstest::rstest]
    fn test_to_target_amount_pairs(two_entry_route: WithdrawRoute) {
        let route = two_entry_route;
        let pairs = route.to_target_amount_pairs();

        assert_eq!(pairs, vec![(1, 500), (2, 300)]);
    }

    #[rstest::rstest]
    fn test_withdraw_plan(two_entry_route: WithdrawRoute) {
        let route = two_entry_route;
        let plan = route.withdraw_plan();

        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].target_id, 1);
        assert_eq!(plan[0].max_amount, 500);
        assert_eq!(plan[1].target_id, 2);
        assert_eq!(plan[1].max_amount, 300);
    }

    #[rstest::rstest]
    #[case(Err(WithdrawRoute::new(vec![route_entry(1, 500)], 1000).unwrap_err()), false)]
    #[case(Ok(WithdrawRoute::new(vec![route_entry(1, 1000)], 1000).unwrap()), true)]
    fn test_can_satisfy_valid_route(
        #[case] route: Result<WithdrawRoute, WithdrawRouteError>,
        #[case] expected: bool,
    ) {
        match route {
            Ok(route) => assert_eq!(route.can_satisfy(), expected),
            Err(WithdrawRouteError::InsufficientRouteTotal { .. }) => assert!(!expected),
            Err(error) => panic!("unexpected error: {error:?}"),
        }
    }

    #[rstest::rstest]
    fn test_get_entry_and_has_target(two_entry_route: WithdrawRoute) {
        let route = two_entry_route;

        assert!(route.has_target(1));
        assert!(route.has_target(2));
        assert!(!route.has_target(3));

        let entry = route.get_entry(1);
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().max_amount, 500);

        assert!(route.get_entry(3).is_none());
    }
}
