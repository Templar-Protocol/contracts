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
    compute_recovery_stats, compute_settlement_shares, determine_recovery_action, RecoveryContext,
    RecoveryProgress,
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
        boundary_policy_class, canonical_policy_class, ActionKind, AuthAdapter, AuthError,
        AuthPolicyClass, AuthResult, Caller,
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
    // Simulate a supply queue with multiple entries for the same target
    let mut queue = SupplyQueue::default();

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
    let mut queue = SupplyQueue::default();

    // Add entries with different priorities
    queue = queue
        .enqueue(
            SupplyQueueEntry::builder()
                .target_id(0_u32)
                .amount(100_000_000_000_u128)
                .priority(0)
                .build(),
        )
        .unwrap();
    queue = queue
        .enqueue(
            SupplyQueueEntry::builder()
                .target_id(1_u32)
                .amount(200_000_000_000_u128)
                .priority(5)
                .build(),
        )
        .unwrap();
    queue = queue
        .enqueue(
            SupplyQueueEntry::builder()
                .target_id(2_u32)
                .amount(300_000_000_000_u128)
                .priority(10)
                .build(),
        )
        .unwrap();
    queue = queue
        .enqueue(
            SupplyQueueEntry::builder()
                .target_id(3_u32)
                .amount(400_000_000_000_u128)
                .priority(3)
                .build(),
        )
        .unwrap();

    // Expected order by priority (highest first): 2, 1, 3, 0
    let entries: Vec<u32> = queue.entries.iter().map(|e| e.target_id).collect();
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

#[rstest::rstest]
fn golden_refresh_plan_building(near_snapshot: NearVaultSnapshot) {
    let snapshot = near_snapshot;
    let enabled_targets: Vec<u32> = snapshot
        .market_principals
        .iter()
        .map(|(id, _)| *id)
        .collect();

    // Build refresh plan for all markets
    let plan = build_refresh_plan(&enabled_targets, Some(30_000_000_000)).unwrap();

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
            AllocationPlanEntry::new(0, 300_000_000_000),
            AllocationPlanEntry::new(1, 200_000_000_000),
            AllocationPlanEntry::new(2, 300_000_000_000),
            AllocationPlanEntry::new(3, 200_000_000_000),
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
    let settlement = compute_settlement_shares(escrow, expected, collected);
    assert_eq!(settlement.to_burn, expected_burn);
    assert_eq!(settlement.refund, expected_refund);
}

#[cfg(feature = "recovery")]
#[test]
fn golden_settlement_shares_large_values() {
    let escrow = u128::MAX / 2;
    let expected = u128::MAX / 4;
    let collected = expected / 2;

    let settlement = compute_settlement_shares(escrow, expected, collected);

    assert!(settlement.to_burn <= escrow);
    assert_eq!(settlement.to_burn + settlement.refund, escrow);
}

// Golden Test: Integration Scenario

#[rstest::rstest]
fn golden_full_allocation_cycle(near_snapshot: NearVaultSnapshot) {
    let snapshot = near_snapshot;

    // Step 1: Create supply queue with batched deposits (1M total)
    let mut queue = SupplyQueue::default();
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

    // Build refresh plan for all markets
    let enabled_targets: Vec<u32> = snapshot
        .market_principals
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let plan = build_refresh_plan(&enabled_targets, None).unwrap();

    // Validate plan
    // Simulate refreshing state
    let state = OpState::Refreshing(RefreshingState {
        op_id: 100,
        index: 1,
        plan: plan.targets().to_vec(),
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
        let group1 = CapGroupId::from("group1");
        let group2 = CapGroupId::from("group2");

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
        let group1 = CapGroupId::from("group1");

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
    fn test_zero_absolute_cap_is_unlimited() {
        let cap = CapGroup::builder().absolute_cap(0).build();
        assert!(cap.absolute_cap().is_none());
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
        let group = CapGroupId::from("group1");
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
        compute_settlement_shares, determine_recovery_action, handle_allocation_failure,
        handle_payout_failure, handle_payout_failure_default, handle_refresh_failure,
        handle_withdrawal_failure, RecoveryContext, RecoveryOutcome, RecoveryProgress,
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
            plan: vec![
                AllocationPlanEntry::new(0, 300),
                AllocationPlanEntry::new(1, 200),
                AllocationPlanEntry::new(2, 300),
                AllocationPlanEntry::new(3, 200),
            ],
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
            plan: vec![AllocationPlanEntry::new(0, 100)],
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
            plan: vec![AllocationPlanEntry::new(0, 100)],
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

    #[rstest::rstest]
    #[case(1000, 500, 500, 1000, 0)] // full collection
    #[case(1000, 500, 250, 500, 500)] // partial collection
    #[case(1000, 500, 600, 1000, 0)] // over collection
    #[case(1000, 0, 0, 0, 1000)] // zero expected
    #[case(0, 500, 250, 0, 0)] // zero escrow
    fn test_compute_settlement_shares_cases(
        #[case] escrow: u128,
        #[case] expected: u128,
        #[case] collected: u128,
        #[case] expected_burn: u128,
        #[case] expected_refund: u128,
    ) {
        let settlement = compute_settlement_shares(escrow, expected, collected);
        assert_eq!(settlement.to_burn, expected_burn);
        assert_eq!(settlement.refund, expected_refund);
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
    fn test_handle_allocation_failure() {
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

mod governance_module_tests {
    pub use crate::governance::*;
    use alloc::collections::BTreeSet;
    use templar_vault_kernel::TimestampNs;

    #[test]
    fn pending_value_maturity_is_time_based() {
        let pending = PendingValue {
            value: "ok",
            valid_at_ns: TimestampNs(1_000),
        };

        assert!(!pending.is_mature(TimestampNs(999)));
        assert!(pending.is_mature(TimestampNs(1_000)));
        assert!(pending.is_mature(TimestampNs(1_001)));
    }

    #[test]
    fn queue_take_mature_enforces_timelock() {
        let mut queue = PendingQueue::from(alloc::collections::VecDeque::from([PendingValue {
            value: "change",
            valid_at_ns: TimestampNs(1_000),
        }]));

        let not_ready = queue.take_mature(TimestampNs(999), |value| *value == "change");
        assert_eq!(not_ready, Err(PendingQueueError::NotMature));
        assert_eq!(queue.len(), 1);

        let ready = queue.take_mature(TimestampNs(1_000), |value| *value == "change");
        assert_eq!(ready, Ok(Some("change")));
        assert!(queue.is_empty());
    }

    #[test]
    fn cap_change_decision_market_new_cap_is_timelocked() {
        let decision = TimelockDecision::from_cap_change(None, 100);
        assert_eq!(decision, Ok(TimelockDecision::Timelocked));
    }

    #[test]
    fn cap_group_cap_change_decision_unlimited_to_finite_is_immediate() {
        let from_none = TimelockDecision::from_cap_group_cap_change(None, 100);
        assert_eq!(from_none, Ok(TimelockDecision::Immediate));

        let from_zero = TimelockDecision::from_cap_group_cap_change(Some(0), 100);
        assert_eq!(from_zero, Ok(TimelockDecision::Immediate));
    }

    #[test]
    fn cap_group_cap_change_decision_finite_to_unlimited_is_timelocked() {
        let decision = TimelockDecision::from_cap_group_cap_change(Some(100), 0);
        assert_eq!(decision, Ok(TimelockDecision::Timelocked));
    }

    #[test]
    fn determine_relaxed_paused_to_empty_whitelist_is_not_relaxing() {
        let current = Some(Restrictions::<&str>::Paused);
        let next = Some(Restrictions::Whitelist(BTreeSet::new()));

        assert!(!Restrictions::determine_relaxed(&current, &next));
    }

    #[test]
    fn determine_relaxed_paused_to_nonempty_whitelist_is_relaxing() {
        let current = Some(Restrictions::<&str>::Paused);
        let next = Some(Restrictions::Whitelist(BTreeSet::from(["alice"])));

        assert!(Restrictions::determine_relaxed(&current, &next));
    }
}

mod rbac_module_tests {
    use crate::auth::{ActionKind, AuthAdapter, AuthError};
    pub use crate::rbac::*;
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
        assert!(matches!(result, Err(AuthError::MissingRole)));
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
        assert!(matches!(result, Err(AuthError::MissingRole)));
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
        assert!(matches!(result, Err(AuthError::MissingRole)));

        let result = auth.authorize(ActionKind::Pause, user_addr, None);
        assert!(matches!(result, Err(AuthError::MissingRole)));
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
        assert!(matches!(result, Err(AuthError::MissingRole)));
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
        assert!(matches!(result, Err(AuthError::MissingRole)));

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
        assert!(matches!(result, Err(AuthError::MissingRole)));

        let result = auth.authorize(ActionKind::ManualReconcile, guardian_addr, None);
        assert!(matches!(result, Err(AuthError::MissingRole)));
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
        assert_eq!(cap.absolute_cap().map(|v| v.get()), Some(1_000));
        assert_eq!(cap.relative_cap(), Some(Wad::from(WAD / 2)));

        let record = CapGroupRecord {
            cap,
            principal: 300,
        };
        assert_eq!(record.principal, 300);
        assert_eq!(record.cap.absolute_cap().map(|v| v.get()), Some(1_000));
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
    fn test_unlimited_state_is_canonical_after_recording() {
        let unlimited = Cooldown::unlimited();
        let recorded = unlimited.recorded_at(123);

        assert_eq!(recorded, Cooldown::unlimited());
        assert_eq!(recorded.last_event_ns(), None);
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

    use crate::policy::market_lock::{MarketLock, MarketLockSet};
    use crate::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
    use crate::policy::withdraw_route::{WithdrawRoute, WithdrawRouteEntry, WithdrawRouteError};
    use templar_vault_kernel::{TargetId, TimestampNs};

    fn lock_set_with_target(target_id: TargetId) -> MarketLockSet {
        MarketLockSet::default()
            .acquire(MarketLock::new(target_id, 1_000), 1_000)
            .expect("lock should be acquirable")
    }

    #[rstest::fixture]
    fn lock_set_target_1() -> MarketLockSet {
        lock_set_with_target(1)
    }

    #[rstest::fixture]
    fn lock_set_target_2() -> MarketLockSet {
        lock_set_with_target(2)
    }

    #[rstest::rstest]
    fn filters_targets(lock_set_target_2: MarketLockSet) {
        let lock_set = lock_set_target_2;
        let targets = vec![1, 2, 3];
        assert_eq!(
            lock_set.excluding_leased_targets(&targets, TimestampNs(1_500)),
            vec![1, 3]
        );
    }

    #[rstest::rstest]
    fn excludes_locked_supply_queue_entries_and_preserves_max_length(
        lock_set_target_2: MarketLockSet,
    ) {
        let lock_set = lock_set_target_2;
        let queue = SupplyQueue {
            entries: vec![
                SupplyQueueEntry::new(1, 10),
                SupplyQueueEntry::new(2, 20),
                SupplyQueueEntry::new(3, 30),
            ],
            max_length: 16,
        };

        let filtered = queue.excluding_leased(&lock_set, TimestampNs(1_500));

        assert_eq!(filtered.max_length, 16);
        assert_eq!(filtered.entries.len(), 2);
        assert_eq!(filtered.entries[0].target_id, 1);
        assert_eq!(filtered.entries[1].target_id, 3);
    }

    #[rstest::rstest]
    fn excluding_leased_targets_can_invalidate_withdraw_route(lock_set_target_1: MarketLockSet) {
        let lock_set = lock_set_target_1;
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 100),
                WithdrawRouteEntry::new(2, 200),
            ],
            250,
        );

        let filtered = route.excluding_leased(&lock_set, TimestampNs(1_500));

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
    fn builds_allocation_plan_excluding_leased_targets(lock_set_target_2: MarketLockSet) {
        let lock_set = lock_set_target_2;
        let queue = SupplyQueue {
            entries: vec![
                SupplyQueueEntry::new(1, 10),
                SupplyQueueEntry::new(2, 20),
                SupplyQueueEntry::new(3, 30),
            ],
            max_length: 16,
        };

        assert_eq!(
            queue.to_allocation_plan_excluding_leased(&lock_set, TimestampNs(1_500)),
            vec![(1, 10), (3, 30)]
        );
    }

    #[rstest::rstest]
    fn builds_withdrawal_plan_excluding_leased_targets(lock_set_target_1: MarketLockSet) {
        let lock_set = lock_set_target_1;
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 100),
                WithdrawRouteEntry::new(2, 200),
                WithdrawRouteEntry::new(3, 300),
            ],
            450,
        );

        assert_eq!(
            route
                .to_withdrawal_plan_excluding_leased(&lock_set, TimestampNs(1_500))
                .expect("filtered route remains satisfiable"),
            vec![(2, 200), (3, 300)]
        );
    }

    #[test]
    fn filtered_withdrawal_plan_errors_when_locks_break_route() {
        let lock_set = lock_set_with_target(1);
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 100),
                WithdrawRouteEntry::new(2, 200),
            ],
            250,
        );

        let result = route.to_withdrawal_plan_excluding_leased(&lock_set, TimestampNs(1_500));

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
        let lock_set = lock_set_with_target(1);
        let invalid_route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 100),
                WithdrawRouteEntry::new(1, 200),
            ],
            250,
        );

        let result = invalid_route.excluding_leased(&lock_set, TimestampNs(1_500));

        assert!(matches!(
            result,
            Err(WithdrawRouteError::DuplicateTarget { target_id: 1 })
        ));
    }

    #[rstest::rstest]
    fn filters_refresh_targets(lock_set_target_2: MarketLockSet) {
        let lock_set = lock_set_target_2;
        let targets = vec![1, 2, 3, 4];

        assert_eq!(
            lock_set.excluding_leased_targets(&targets, TimestampNs(1_500)),
            vec![1, 3, 4]
        );
    }

    #[rstest::rstest]
    fn reports_unleased_targets(lock_set_target_2: MarketLockSet) {
        let lock_set = lock_set_target_2;

        assert!(lock_set.is_unleased(1, TimestampNs(1_500)));
        assert!(!lock_set.is_unleased(2, TimestampNs(1_500)));
        assert!(lock_set.is_unleased(3, TimestampNs(1_500)));
    }
}

mod policy_market_lock_tests {
    pub use crate::policy::market_lock::*;

    use alloc::vec;

    #[rstest::fixture]
    fn empty_lock_set() -> MarketLockSet {
        MarketLockSet::default()
    }

    #[rstest::rstest]
    fn test_new_lock_set_is_empty(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert_eq!(set.active_count(0), 0);
    }

    #[rstest::rstest]
    fn test_acquire_lock(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock = MarketLock::new(1, 1000);

        let result = set.acquire(lock, 1000).unwrap();

        assert_eq!(result.len(), 1);
        assert!(result.is_locked(1, 1000));
    }

    #[rstest::rstest]
    fn test_acquire_lock_already_locked(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::new(1, 2000);

        let set = set.acquire(lock1, 1000).unwrap();
        let result = set.acquire(lock2, 2000);

        assert!(result.is_err());
    }

    #[rstest::rstest]
    fn test_acquire_lock_different_target(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::new(2, 2000);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 2000).unwrap();

        assert_eq!(set.len(), 2);
        assert!(set.is_locked(1, 2000));
        assert!(set.is_locked(2, 2000));
    }

    #[rstest::rstest]
    fn test_acquire_lock_after_expiry(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock1 = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(2000_u64)
            .build();
        let lock2 = MarketLock::new(1, 3000);

        let set = set.acquire(lock1, 1000).unwrap();

        // Should fail before expiry
        let result = set.acquire(lock2.clone(), 1500);
        assert!(result.is_err());

        // Should succeed after expiry
        let set = set.acquire(lock2, 3000).unwrap();
        assert_eq!(set.len(), 1); // Old expired lock removed
        assert!(set.is_locked(1, 3000));
    }

    #[rstest::rstest]
    fn test_release_lock(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock = MarketLock::new(1, 1000);

        let set = set.acquire(lock, 1000).unwrap();
        let set = set.release(1);

        assert!(set.is_empty());
        assert!(!set.is_locked(1, 2000));
    }

    #[rstest::rstest]
    fn test_release_lock_by_op(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock1 = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .op_id(100_u64)
            .build();
        let lock2 = MarketLock::builder()
            .target_id(2_u32)
            .locked_at_ns(1000_u64)
            .op_id(200_u64)
            .build();

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();

        // Release only the lock held by op 100
        let set = set.release_by_op(1, 100);

        assert_eq!(set.len(), 1);
        assert!(!set.is_locked(1, 2000));
        assert!(set.is_locked(2, 2000));
    }

    #[rstest::rstest]
    fn test_release_all_by_op(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock1 = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .op_id(100_u64)
            .build();
        let lock2 = MarketLock::builder()
            .target_id(2_u32)
            .locked_at_ns(1000_u64)
            .op_id(100_u64)
            .build();
        let lock3 = MarketLock::builder()
            .target_id(3_u32)
            .locked_at_ns(1000_u64)
            .op_id(200_u64)
            .build();

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();
        let set = set.acquire(lock3, 1000).unwrap();

        let set = set.release_all_by_op(100);

        assert_eq!(set.len(), 1);
        assert!(!set.is_locked(1, 2000));
        assert!(!set.is_locked(2, 2000));
        assert!(set.is_locked(3, 2000));
    }

    #[rstest::rstest]
    fn test_is_locked_by_op(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .op_id(100_u64)
            .build();

        let set = set.acquire(lock, 1000).unwrap();

        assert!(set.is_locked_by_op(1, 100, 1000));
        assert!(!set.is_locked_by_op(1, 200, 1000));
        assert!(!set.is_locked_by_op(2, 100, 1000));
    }

    #[test]
    fn test_is_locked_by_op_ignores_expired_locks() {
        let set = MarketLockSet::default();
        let lock = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(1500_u64)
            .op_id(100_u64)
            .build();

        let set = set.acquire(lock, 1000).unwrap();

        assert!(set.is_locked_by_op(1, 100, 1499));
        assert!(!set.is_locked_by_op(1, 100, 1500));
    }

    #[test]
    fn test_lock_expiry() {
        let lock = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(2000_u64)
            .build();

        assert!(!lock.is_expired(1000));
        assert!(!lock.is_expired(1999));
        assert!(lock.is_expired(2000));
        assert!(lock.is_expired(3000));
    }

    #[test]
    fn test_lock_no_expiry() {
        let lock = MarketLock::new(1, 1000);

        // No expiry means never expires
        assert!(!lock.is_expired(u64::MAX));
        assert!(lock.expires_at_ns.is_none());
    }

    #[test]
    fn test_lock_with_ttl() {
        let lock = MarketLock::new(1, 1000).with_ttl(500);
        assert_eq!(lock.expires_at_ns, Some(1500));
    }

    #[test]
    fn test_lock_remaining() {
        let lock = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(2000_u64)
            .build();
        assert_eq!(lock.remaining(1000), Some(1000));
        assert_eq!(lock.remaining(1500), Some(500));
        assert_eq!(lock.remaining(2000), Some(0));

        let no_expiry = MarketLock::new(1, 1000);
        assert_eq!(no_expiry.remaining(5000), None);
    }

    #[rstest::rstest]
    fn test_cleanup_expired_locks(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock1 = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(2000_u64)
            .build();
        let lock2 = MarketLock::builder()
            .target_id(2_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(3000_u64)
            .build();
        let lock3 = MarketLock::new(3, 1000); // no expiry

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();
        let set = set.acquire(lock3, 1000).unwrap();

        let cleaned = set.cleanup_expired(2500);

        assert_eq!(cleaned.len(), 2);
        assert!(!cleaned.is_locked(1, 2500)); // expired
        assert!(cleaned.is_locked(2, 2500)); // not yet expired
        assert!(cleaned.is_locked(3, 2500)); // no expiry
    }

    #[rstest::rstest]
    fn test_get_locked_targets(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::builder()
            .target_id(2_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(1500_u64)
            .build();
        let lock3 = MarketLock::new(3, 1000);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();
        let set = set.acquire(lock3, 1000).unwrap();

        let locked = set.locked_targets(2000);

        assert_eq!(locked.len(), 2);
        assert!(locked.contains(&1));
        assert!(!locked.contains(&2)); // expired
        assert!(locked.contains(&3));
    }

    #[rstest::rstest]
    fn test_find_locked_targets(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock = MarketLock::new(2, 1000);

        let set = set.acquire(lock, 1000).unwrap();

        let to_check = vec![1, 2, 3, 4];
        let locked = set.find_locked_targets(&to_check, 2000);

        assert_eq!(locked, vec![2]);
    }

    #[rstest::rstest]
    fn test_clear_all_locks(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::new(2, 1000);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();

        let cleared = set.clear();

        assert!(cleared.is_empty());
    }

    #[rstest::rstest]
    fn test_active_count(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock1 = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(2000_u64)
            .build();
        let lock2 = MarketLock::builder()
            .target_id(2_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(3000_u64)
            .build();
        let lock3 = MarketLock::new(3, 1000);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();
        let set = set.acquire(lock3, 1000).unwrap();

        assert_eq!(set.len(), 3); // Total locks
        assert_eq!(set.active_count(1500), 3); // All active
        assert_eq!(set.active_count(2500), 2); // lock1 expired
        assert_eq!(set.active_count(3500), 1); // lock1 and lock2 expired
    }

    #[rstest::rstest]
    fn test_get_lock(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .op_id(42_u64)
            .build();

        let set = set.acquire(lock, 1000).unwrap();

        let found = set.get_lock(1, 1500);
        assert!(found.is_some());
        assert_eq!(found.unwrap().op_id, Some(42));

        let not_found = set.get_lock(2, 1500);
        assert!(not_found.is_none());
    }

    #[rstest::rstest]
    fn test_is_all_expired(empty_lock_set: MarketLockSet) {
        let set = empty_lock_set;
        let lock1 = MarketLock::builder()
            .target_id(1_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(2000_u64)
            .build();
        let lock2 = MarketLock::builder()
            .target_id(2_u32)
            .locked_at_ns(1000_u64)
            .expires_at_ns(2000_u64)
            .build();

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();

        assert!(!set.is_all_expired(1500));
        assert!(set.is_all_expired(2500));
    }
}

mod policy_refresh_plan_tests {
    pub use crate::policy::refresh_plan::*;

    use crate::policy::target_set::find_first_duplicate;
    use alloc::vec;
    use alloc::vec::Vec;
    use templar_vault_kernel::TargetId;

    #[test]
    fn test_new_plan() {
        let plan = RefreshPlan::new(vec![1, 2, 3]).unwrap();
        assert!(!plan.is_empty());
        assert_eq!(plan.len(), 3);
        assert!(plan.cooldown().is_unlimited());
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
        let plan = RefreshPlan::new(vec![1, 2]).unwrap();
        assert!(plan.check_cooldown(1000).is_ok());
        assert!(plan.is_ready(1000));
    }

    #[test]
    fn test_check_refresh_cooldown_first_refresh() {
        let plan = RefreshPlan::new(vec![1, 2]).unwrap().with_cooldown(1000);
        // No last_refresh_ns, so first refresh should be allowed
        assert!(plan.check_cooldown(100).is_ok());
        assert!(plan.is_ready(100));
    }

    #[test]
    fn test_check_refresh_cooldown_on_cooldown() {
        let plan = RefreshPlan::new(vec![1, 2])
            .unwrap()
            .with_cooldown(1000)
            .with_last_refresh(Some(100));

        // Only 500ns elapsed, cooldown is 1000ns
        let result = plan.check_cooldown(600);
        assert!(matches!(result, Err(RefreshPlanError::OnCooldown { .. })));
        assert!(!plan.is_ready(600));
    }

    #[test]
    fn test_check_refresh_cooldown_after_cooldown() {
        let plan = RefreshPlan::new(vec![1, 2])
            .unwrap()
            .with_cooldown(1000)
            .with_last_refresh(Some(100));

        // 1100ns elapsed, cooldown is 1000ns
        assert!(plan.check_cooldown(1200).is_ok());
        assert!(plan.is_ready(1200));
    }

    #[test]
    fn test_with_cooldown_preserves_last_refresh_timestamp() {
        let plan = RefreshPlan::new(vec![1, 2])
            .unwrap()
            .with_last_refresh(Some(50))
            .with_cooldown(200);

        assert_eq!(plan.last_refresh_ns(), Some(50));
        assert_eq!(plan.cooldown_ns(), 200);
    }

    #[test]
    fn test_zero_cooldown_maps_to_unlimited() {
        let plan = RefreshPlan::new(vec![1, 2]).unwrap().with_cooldown(0);

        assert!(plan.cooldown().is_unlimited());
        assert_eq!(plan.cooldown_ns(), 0);
        assert_eq!(plan.last_refresh_ns(), None);
    }

    #[test]
    fn test_build_refresh_plan() {
        let enabled = vec![1, 2, 3];
        let plan = build_refresh_plan(&enabled, Some(5000)).unwrap();

        assert_eq!(plan.targets(), [1, 2, 3]);
        assert_eq!(plan.cooldown_ns(), 5000);
    }

    #[test]
    fn test_build_refresh_plan_empty() {
        let enabled: Vec<TargetId> = vec![];
        let result = build_refresh_plan(&enabled, None);

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
        let plan = RefreshPlan::new(vec![1, 2]).unwrap().with_cooldown(1000);
        let updated = plan.record_completion(5000);

        assert_eq!(updated.last_refresh_ns(), Some(5000));
        assert_eq!(updated.cooldown_ns(), 1000);
        assert_eq!(updated.targets(), [1, 2]);
    }

    #[test]
    fn test_filter_stale_targets() {
        let targets = vec![
            (1, 1000), // refreshed at 1000
            (2, 500),  // refreshed at 500
            (3, 2000), // refreshed at 2000
        ];

        // Current time is 3000, max age is 1500
        // Target 2 (age 2500) is stale
        // Target 1 (age 2000) is stale
        // Target 3 (age 1000) is fresh
        let stale = filter_stale_targets(&targets, 1500, 3000);

        assert_eq!(stale.len(), 2);
        assert!(stale.contains(&1));
        assert!(stale.contains(&2));
        assert!(!stale.contains(&3));
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

    use crate::policy::cap_group::{CapGroupId, CapGroupRecord};
    use alloc::string::String;

    #[test]
    fn external_assets_sums_principals() {
        let mut state = PolicyState::default();
        state.set_principal(1, 100);
        state.set_principal(2, 250);
        state.set_principal(3, 50);

        assert_eq!(state.external_assets(), 400);
    }

    #[test]
    fn cap_group_totals_aggregate_by_group() {
        let mut state = PolicyState::default();
        let group_a: CapGroupId = "group-a".into();
        let group_b: CapGroupId = "group-b".into();

        state.set_market_config(1, MarketConfig::new(true, Some(group_a.clone())));
        state.set_market_config(2, MarketConfig::new(true, Some(group_a.clone())));
        state.set_market_config(3, MarketConfig::new(true, Some(group_b.clone())));

        state.set_principal(1, 10);
        state.set_principal(2, 20);
        state.set_principal(3, 40);

        let totals = state.compute_cap_group_totals();
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
    fn refresh_cap_group_principals_updates_records() {
        let mut state = PolicyState::default();
        let group: CapGroupId = String::from("group").into();
        state
            .cap_groups
            .insert(group.clone(), CapGroupRecord::default());
        state.set_market_config(1, MarketConfig::new(true, Some(group.clone())));
        state.set_principal(1, 123);

        state.refresh_cap_group_principals();

        let record = state.cap_groups.get(&group).expect("cap group");
        assert_eq!(record.principal, 123);
    }
}

mod policy_supply_queue_tests {
    pub use crate::policy::supply_queue::*;

    #[rstest::fixture]
    fn empty_queue() -> SupplyQueue {
        SupplyQueue::default()
    }

    #[rstest::fixture]
    fn queue_two_entries(empty_queue: SupplyQueue) -> SupplyQueue {
        empty_queue
            .enqueue(SupplyQueueEntry::new(1, 100))
            .unwrap()
            .enqueue(SupplyQueueEntry::new(2, 200))
            .unwrap()
    }

    #[rstest::fixture]
    fn queue_with_repeated_target(empty_queue: SupplyQueue) -> SupplyQueue {
        empty_queue
            .enqueue(SupplyQueueEntry::new(1, 100))
            .unwrap()
            .enqueue(SupplyQueueEntry::new(2, 200))
            .unwrap()
            .enqueue(SupplyQueueEntry::new(1, 50))
            .unwrap()
    }

    #[rstest::rstest]
    fn test_new_queue_is_empty(empty_queue: SupplyQueue) {
        let queue = empty_queue;
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert!(!queue.is_full());
    }

    #[rstest::rstest]
    fn test_enqueue_supply(empty_queue: SupplyQueue) {
        let queue = empty_queue;
        let entry = SupplyQueueEntry::new(1, 100);

        let result = queue.enqueue(entry.clone()).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.entries[0], entry);
    }

    #[rstest::rstest]
    fn test_enqueue_zero_amount_error(empty_queue: SupplyQueue) {
        let queue = empty_queue;
        let entry = SupplyQueueEntry::new(1, 0);

        let result = queue.enqueue(entry);

        assert!(matches!(result, Err(SupplyQueueError::ZeroAmount)));
    }

    #[test]
    fn test_enqueue_full_queue_error() {
        let queue = SupplyQueue {
            entries: alloc::vec![],
            max_length: 2,
        };
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(3, 300);

        let queue = queue.enqueue(entry1).unwrap();
        let queue = queue.enqueue(entry2).unwrap();
        let result = queue.enqueue(entry3);

        assert!(matches!(
            result,
            Err(SupplyQueueError::QueueFull { max_length: 2 })
        ));
    }

    #[rstest::rstest]
    fn test_enqueue_with_priority(empty_queue: SupplyQueue) {
        let queue = empty_queue;
        let low = SupplyQueueEntry::builder()
            .target_id(1_u32)
            .amount(100_u128)
            .priority(0)
            .build();
        let high = SupplyQueueEntry::builder()
            .target_id(2_u32)
            .amount(200_u128)
            .priority(10)
            .build();
        let medium = SupplyQueueEntry::builder()
            .target_id(3_u32)
            .amount(300_u128)
            .priority(5)
            .build();

        let queue = queue.enqueue(low).unwrap();
        let queue = queue.enqueue(high).unwrap();
        let queue = queue.enqueue(medium).unwrap();

        // High priority should be first
        assert_eq!(queue.entries[0].target_id, 2);
        assert_eq!(queue.entries[1].target_id, 3);
        assert_eq!(queue.entries[2].target_id, 1);
    }

    #[rstest::rstest]
    fn test_dequeue_supply(queue_two_entries: SupplyQueue) {
        let queue = queue_two_entries;
        let (queue, dequeued) = queue.dequeue().unwrap();

        assert_eq!(dequeued.target_id, 1);
        assert_eq!(dequeued.amount, 100);
        assert_eq!(queue.len(), 1);
    }

    #[rstest::rstest]
    fn test_dequeue_empty_error(empty_queue: SupplyQueue) {
        let queue = empty_queue;
        let result = queue.dequeue();

        assert!(matches!(result, Err(SupplyQueueError::QueueEmpty)));
    }

    #[rstest::rstest]
    fn test_peek(empty_queue: SupplyQueue) {
        let queue = empty_queue;
        assert!(queue.peek().is_none());

        let entry = SupplyQueueEntry::new(1, 100);
        let queue = queue.enqueue(entry.clone()).unwrap();

        assert_eq!(queue.peek(), Some(&entry));
        assert_eq!(queue.len(), 1); // Still in queue
    }

    #[rstest::rstest]
    fn test_compute_queue_total(queue_with_repeated_target: SupplyQueue) {
        let queue = queue_with_repeated_target;
        assert_eq!(queue.total(), 350);
    }

    #[rstest::rstest]
    fn test_compute_queue_totals_by_target(queue_with_repeated_target: SupplyQueue) {
        let queue = queue_with_repeated_target;
        let totals = queue.totals_by_target();

        assert_eq!(totals.len(), 2);
        assert!(totals.contains(&(1, 150)));
        assert!(totals.contains(&(2, 200)));
    }

    #[rstest::rstest]
    fn test_remove_target_entries(queue_with_repeated_target: SupplyQueue) {
        let queue = queue_with_repeated_target;
        let filtered = queue.remove_target(1);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered.entries[0].target_id, 2);
    }

    #[rstest::rstest]
    fn test_drain_queue(queue_two_entries: SupplyQueue) {
        let queue = queue_two_entries;
        let (empty, entries) = queue.drain();

        assert!(empty.is_empty());
        assert_eq!(entries.len(), 2);
    }

    #[rstest::rstest]
    fn test_to_allocation_plan(queue_with_repeated_target: SupplyQueue) {
        let queue = queue_with_repeated_target;
        let plan = queue.to_allocation_plan();

        // Should be aggregated by target
        assert_eq!(plan.len(), 2);
        assert!(plan.contains(&(1, 150)));
        assert!(plan.contains(&(2, 200)));
    }

    #[rstest::rstest]
    fn test_total_for_target(queue_with_repeated_target: SupplyQueue) {
        let queue = queue_with_repeated_target;
        assert_eq!(queue.total_for_target(1), 150);
        assert_eq!(queue.total_for_target(2), 200);
        assert_eq!(queue.total_for_target(3), 0);
    }

    #[rstest::rstest]
    fn test_has_target(empty_queue: SupplyQueue) {
        let queue = empty_queue;
        let entry = SupplyQueueEntry::new(1, 100);
        let queue = queue.enqueue(entry).unwrap();

        assert!(queue.has_target(1));
        assert!(!queue.has_target(2));
    }

    #[test]
    fn test_builder_pattern() {
        let entry = SupplyQueueEntry::builder()
            .target_id(1_u32)
            .amount(100_u128)
            .priority(5)
            .queued_at_ns(1000_u64)
            .build();

        assert_eq!(entry.target_id, 1);
        assert_eq!(entry.amount, 100);
        assert_eq!(entry.priority, 5);
        assert_eq!(entry.queued_at_ns, 1000);
    }
}

mod policy_target_set_tests {
    pub use crate::policy::target_set::*;

    use alloc::vec;

    use crate::policy::market_lock::{MarketLock, MarketLockSet};

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
    fn validates_no_duplicate_targets() {
        assert!(validate_no_duplicate_targets(&[1, 2, 3]));
        assert!(!validate_no_duplicate_targets(&[1, 2, 1]));
        assert_eq!(find_duplicate_target_id(&[1, 2, 1]), Some(1));
    }

    #[test]
    fn builds_withdraw_plan_from_target_principals() {
        let principals = vec![(1, 100), (2, 200), (3, 300)];
        let plan = build_withdraw_plan_from_target_principals(&principals, 250).unwrap();

        assert_eq!(plan, vec![(3, 300), (2, 200), (1, 100)]);
    }

    #[test]
    fn target_lock_helpers_delegate_to_lock_set() {
        let mut set = MarketLockSet::default();
        set = set.acquire(MarketLock::new(2, 1_000), 1_000).unwrap();

        let targets = vec![1, 2, 3];
        assert_eq!(find_locked_targets(&set, &targets, 1_500), vec![2]);
        assert!(is_target_locked(&set, 2, 1_500));
        assert!(!is_target_locked(&set, 1, 1_500));
        assert_eq!(get_locked_targets(&set, 1_500), vec![2]);
    }

    #[test]
    fn builds_refresh_plan_from_targets() {
        let plan = build_refresh_plan_from_targets(&[1, 2, 3], 100, Some(50)).unwrap();
        assert_eq!(plan.targets(), [1, 2, 3]);
        assert_eq!(plan.cooldown_ns(), 100);
        assert_eq!(plan.last_refresh_ns(), Some(50));
    }
}

mod policy_withdraw_route_tests {
    pub use crate::policy::withdraw_route::*;

    use alloc::vec;

    #[rstest::fixture]
    fn empty_route() -> WithdrawRoute {
        WithdrawRoute::from_entries(vec![], 1000)
    }

    #[rstest::fixture]
    fn valid_route() -> WithdrawRoute {
        WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 500),
                WithdrawRouteEntry::new(2, 300),
                WithdrawRouteEntry::new(3, 200),
            ],
            1000,
        )
    }

    #[rstest::fixture]
    fn two_entry_route() -> WithdrawRoute {
        WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 500),
                WithdrawRouteEntry::new(2, 300),
            ],
            800,
        )
    }

    #[rstest::fixture]
    fn duplicate_target_route() -> WithdrawRoute {
        WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 500),
                WithdrawRouteEntry::new(1, 600),
            ],
            1000,
        )
    }

    #[rstest::fixture]
    fn zero_max_route() -> WithdrawRoute {
        WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 500),
                WithdrawRouteEntry::new(2, 0),
            ],
            500,
        )
    }

    #[rstest::fixture]
    fn insufficient_route() -> WithdrawRoute {
        WithdrawRoute::from_entries(vec![WithdrawRouteEntry::new(1, 500)], 1000)
    }

    #[rstest::rstest]
    fn test_new_route(empty_route: WithdrawRoute) {
        let route = empty_route;
        assert!(route.is_empty());
        assert_eq!(route.target_amount, 1000);
    }

    #[test]
    fn test_builder_pattern() {
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 500),
                WithdrawRouteEntry::new(2, 600),
            ],
            1000,
        );

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

    #[rstest::rstest]
    fn test_compute_route_total(valid_route: WithdrawRoute) {
        let route = valid_route;
        assert_eq!(route.total(), 1000);
    }

    #[test]
    #[should_panic(expected = "called `Option::unwrap()` on a `None` value")]
    fn test_route_total_overflow_panics() {
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, u128::MAX),
                WithdrawRouteEntry::new(2, 1),
            ],
            1,
        );

        let _ = route.total();
    }

    #[test]
    #[should_panic(expected = "called `Option::unwrap()` on a `None` value")]
    fn test_available_liquidity_overflow_panics() {
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 1).with_liquidity(u128::MAX),
                WithdrawRouteEntry::new(2, 1).with_liquidity(1),
            ],
            1,
        );

        let _ = route.available_liquidity();
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

    #[rstest::rstest]
    fn test_validate_withdraw_route_empty(empty_route: WithdrawRoute) {
        let route = empty_route;
        assert!(matches!(
            route.validate(),
            Err(WithdrawRouteError::EmptyRoute)
        ));
    }

    #[rstest::rstest]
    fn test_validate_withdraw_route_insufficient(insufficient_route: WithdrawRoute) {
        let route = insufficient_route;
        assert!(matches!(
            route.validate(),
            Err(WithdrawRouteError::InsufficientRouteTotal { .. })
        ));
    }

    #[rstest::rstest]
    fn test_validate_withdraw_route_duplicate(duplicate_target_route: WithdrawRoute) {
        let route = duplicate_target_route;
        assert!(matches!(
            route.validate(),
            Err(WithdrawRouteError::DuplicateTarget { target_id: 1 })
        ));
    }

    #[rstest::rstest]
    fn test_validate_withdraw_route_zero_max(zero_max_route: WithdrawRoute) {
        let route = zero_max_route;
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
    fn test_build_withdraw_route_overflow_errors() {
        let result = build_withdraw_route(&[(1, u128::MAX), (2, 1)], 1);

        assert!(matches!(result, Err(WithdrawRouteError::AmountOverflow)));
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

    #[rstest::rstest]
    fn test_compute_available_liquidity(valid_route: WithdrawRoute) {
        let route = WithdrawRoute::from_entries(
            valid_route
                .entries
                .into_iter()
                .map(|entry| {
                    if entry.target_id == 1 {
                        entry.with_liquidity(400)
                    } else if entry.target_id == 3 {
                        entry.with_liquidity(200)
                    } else {
                        entry
                    }
                })
                .collect(),
            1000,
        );
        assert_eq!(route.available_liquidity(), 600);
    }

    #[rstest::rstest]
    fn test_to_withdrawal_plan(two_entry_route: WithdrawRoute) {
        let route = two_entry_route;
        let plan = route.to_withdrawal_plan();

        assert_eq!(plan, vec![(1, 500), (2, 300)]);
    }

    #[rstest::rstest]
    #[case(WithdrawRoute::from_entries(vec![WithdrawRouteEntry::new(1, 500)], 1000), false)]
    #[case(WithdrawRoute::from_entries(vec![WithdrawRouteEntry::new(1, 1000)], 1000), true)]
    fn test_can_satisfy(#[case] route: WithdrawRoute, #[case] expected: bool) {
        assert_eq!(route.can_satisfy(), expected);
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
