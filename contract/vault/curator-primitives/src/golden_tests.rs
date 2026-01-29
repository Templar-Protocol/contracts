//! Golden tests that compare plan outputs against fixed NEAR curator vault snapshots.
//!
//! These tests validate that the curator primitives produce deterministic outputs
//! when given the same inputs, ensuring compatibility with the NEAR vault implementation.

#![cfg(test)]

use alloc::vec;
use alloc::vec::Vec;

use crate::policy::cap_group::{
    can_allocate_to_group, compute_available_capacity, compute_effective_cap, enforce_cap_group,
    CapGroup,
};
use crate::policy::refresh_plan::{
    build_refresh_plan, compute_refresh_plan_total, validate_refresh_plan,
};
use crate::policy::supply_queue::{
    compute_queue_total, enqueue_supply, to_allocation_plan, SupplyQueue, SupplyQueueEntry,
};
use crate::policy::withdraw_route::{
    build_withdraw_route, compute_route_total, validate_withdraw_route, WithdrawRoute,
    WithdrawRouteEntry,
};
use crate::recovery::{
    compute_recovery_stats, compute_settlement_shares, determine_recovery_action, RecoveryAction,
    RecoveryContext,
};
use templar_vault_kernel::{
    AllocatingState, OpState, PayoutState, RefreshingState, WithdrawingState,
};

// WAD constant matching templar-vault-kernel
const WAD: u128 = 1_000_000_000_000_000_000_000_000;

fn addr_with_tag(tag: u8, index: u64) -> [u8; 32] {
    let mut addr = [0u8; 32];
    addr[0] = tag;
    addr[1..9].copy_from_slice(&index.to_le_bytes());
    addr
}

fn owner_addr(index: u64) -> [u8; 32] {
    addr_with_tag(0x11, index)
}

fn receiver_addr(index: u64) -> [u8; 32] {
    addr_with_tag(0x22, index)
}

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

// ============================================================================
// Golden Test: Cap Group Enforcement
// ============================================================================

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
        let cap = CapGroup::new(*abs_cap, *rel_cap);
        let effective = compute_effective_cap(&cap, snapshot.total_assets);

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
        let cap = CapGroup::new(*abs_cap, *rel_cap);
        let available = compute_available_capacity(&cap, *principal, snapshot.total_assets);

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

    let volatile_cap = CapGroup::new(3_000_000_000_000, WAD * 30 / 100);
    let volatile_principal = 2_500_000_000_000u128;

    // Should succeed: allocate 400_000_000_000 (0.4M)
    assert!(can_allocate_to_group(
        &volatile_cap,
        volatile_principal,
        400_000_000_000,
        snapshot.total_assets
    ));

    // Should succeed: allocate exactly 500_000_000_000 (0.5M)
    assert!(can_allocate_to_group(
        &volatile_cap,
        volatile_principal,
        500_000_000_000,
        snapshot.total_assets
    ));

    // Should fail: allocate 600_000_000_000 (0.6M)
    assert!(!can_allocate_to_group(
        &volatile_cap,
        volatile_principal,
        600_000_000_000,
        snapshot.total_assets
    ));

    // Enforcement should return proper error
    let result = enforce_cap_group(
        &volatile_cap,
        volatile_principal,
        600_000_000_000,
        snapshot.total_assets,
    );
    assert!(result.is_err());
}

// ============================================================================
// Golden Test: Supply Queue to Allocation Plan
// ============================================================================

#[test]
fn golden_supply_queue_to_plan() {
    // Simulate a supply queue with multiple entries for the same target
    let mut queue = SupplyQueue::new();

    // Add entries simulating batched deposits
    queue = enqueue_supply(&queue, SupplyQueueEntry::new(0, 500_000_000_000)).unwrap();
    queue = enqueue_supply(&queue, SupplyQueueEntry::new(1, 300_000_000_000)).unwrap();
    queue = enqueue_supply(&queue, SupplyQueueEntry::new(0, 200_000_000_000)).unwrap();
    queue = enqueue_supply(&queue, SupplyQueueEntry::new(2, 400_000_000_000)).unwrap();
    queue = enqueue_supply(&queue, SupplyQueueEntry::new(1, 100_000_000_000)).unwrap();

    // Expected total: 1.5M
    let total = compute_queue_total(&queue);
    assert_eq!(total, 1_500_000_000_000);

    // Expected plan (aggregated by target):
    // Target 0: 700_000_000_000 (0.7M)
    // Target 1: 400_000_000_000 (0.4M)
    // Target 2: 400_000_000_000 (0.4M)
    let plan = to_allocation_plan(&queue);

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
    queue = enqueue_supply(
        &queue,
        SupplyQueueEntry::with_priority(0, 100_000_000_000, 0),
    )
    .unwrap();
    queue = enqueue_supply(
        &queue,
        SupplyQueueEntry::with_priority(1, 200_000_000_000, 5),
    )
    .unwrap();
    queue = enqueue_supply(
        &queue,
        SupplyQueueEntry::with_priority(2, 300_000_000_000, 10),
    )
    .unwrap();
    queue = enqueue_supply(
        &queue,
        SupplyQueueEntry::with_priority(3, 400_000_000_000, 3),
    )
    .unwrap();

    // Expected order by priority (highest first): 2, 1, 3, 0
    let entries: Vec<u32> = queue.entries.iter().map(|e| e.target_id).collect();
    assert_eq!(entries, vec![2, 1, 3, 0]);
}

// ============================================================================
// Golden Test: Withdraw Route Building
// ============================================================================

#[test]
fn golden_withdraw_route_from_principals() {
    let snapshot = NearVaultSnapshot::default();

    // Build a withdraw route for 2M USDC
    let target_amount = 2_000_000_000_000u128;

    let route = build_withdraw_route(&snapshot.market_principals, target_amount).unwrap();

    // Validate route
    assert!(validate_withdraw_route(&route).is_ok());

    // Route total should cover target
    let route_total = compute_route_total(&route);
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
    assert!(validate_withdraw_route(&route).is_ok());

    // Route total
    assert_eq!(compute_route_total(&route), 2_300_000_000_000);
}

// ============================================================================
// Golden Test: Refresh Plan
// ============================================================================

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

    assert!(validate_refresh_plan(&plan).is_ok());
    assert_eq!(compute_refresh_plan_total(&plan), 3); // 3 markets
    assert_eq!(plan.cooldown_ns, 30_000_000_000); // 30 seconds
}

// ============================================================================
// Golden Test: Recovery Actions
// ============================================================================

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
    let action = determine_recovery_action(&state, &ctx);

    match action {
        RecoveryAction::AbortAllocating {
            op_id,
            remaining,
            completed_targets,
        } => {
            assert_eq!(op_id, 42);
            assert_eq!(remaining, 500_000_000_000);
            assert_eq!(completed_targets, vec![0, 1]); // First 2 completed
        }
        _ => panic!("Expected AbortAllocating"),
    }

    // Check recovery stats
    let stats = compute_recovery_stats(&state);
    assert_eq!(stats.completed_targets, 2);
    assert_eq!(stats.remaining_targets, 2);
    assert_eq!(stats.remaining_amount, 500_000_000_000);
}

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
    let action = determine_recovery_action(&state, &ctx);

    match action {
        RecoveryAction::AbortWithdrawing {
            op_id,
            escrow_shares,
            owner,
            collected,
        } => {
            assert_eq!(op_id, 43);
            assert_eq!(escrow_shares, 1_000_000_000_000);
            assert_eq!(owner, owner_addr(1));
            assert_eq!(collected, 600_000_000_000);
        }
        _ => panic!("Expected AbortWithdrawing"),
    }
}

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
    let action = determine_recovery_action(&state, &ctx);

    match action {
        RecoveryAction::SettlePayout {
            op_id,
            success,
            burn_shares,
            refund_shares,
            ..
        } => {
            assert_eq!(op_id, 44);
            assert!(!success); // Recovery always fails payout
            assert_eq!(burn_shares, 0);
            assert_eq!(refund_shares, 500_000_000_000); // Full refund
        }
        _ => panic!("Expected SettlePayout"),
    }
}

// ============================================================================
// Golden Test: Settlement Share Calculations
// ============================================================================

#[test]
fn golden_settlement_shares_full() {
    // Full withdrawal: collected == expected
    let (burn, refund) =
        compute_settlement_shares(1_000_000_000_000, 500_000_000_000, 500_000_000_000);
    assert_eq!(burn, 1_000_000_000_000); // All shares burned
    assert_eq!(refund, 0); // Nothing refunded
}

#[test]
fn golden_settlement_shares_partial() {
    // Partial withdrawal: collected 60% of expected
    let (burn, refund) =
        compute_settlement_shares(1_000_000_000_000, 500_000_000_000, 300_000_000_000);

    // burn = 1_000_000_000_000 * 300 / 500 = 600_000_000_000
    assert_eq!(burn, 600_000_000_000);
    assert_eq!(refund, 400_000_000_000);
}

#[test]
fn golden_settlement_shares_over_collection() {
    // Over-collection: collected > expected (edge case)
    let (burn, refund) =
        compute_settlement_shares(1_000_000_000_000, 500_000_000_000, 600_000_000_000);
    assert_eq!(burn, 1_000_000_000_000); // All shares burned
    assert_eq!(refund, 0); // Nothing refunded
}

#[test]
fn golden_settlement_shares_large_values() {
    // Test with large values to ensure no overflow
    let escrow = u128::MAX / 2;
    let expected = u128::MAX / 4;
    let collected = expected / 2;

    let (burn, refund) = compute_settlement_shares(escrow, expected, collected);

    // burn = escrow * collected / expected = (MAX/2) * (MAX/8) / (MAX/4) = MAX/4
    // With saturating arithmetic, this should be safe
    assert!(burn <= escrow);
    assert_eq!(burn + refund, escrow);
}

// ============================================================================
// Golden Test: Integration Scenario
// ============================================================================

#[test]
fn golden_full_allocation_cycle() {
    let snapshot = NearVaultSnapshot::default();

    // Step 1: Create supply queue with batched deposits (1M total)
    let mut queue = SupplyQueue::new();
    queue = enqueue_supply(&queue, SupplyQueueEntry::new(0, 400_000_000_000)).unwrap();
    queue = enqueue_supply(&queue, SupplyQueueEntry::new(1, 300_000_000_000)).unwrap();
    queue = enqueue_supply(&queue, SupplyQueueEntry::new(2, 300_000_000_000)).unwrap();

    // Step 2: Convert to allocation plan
    let plan = to_allocation_plan(&queue);
    assert_eq!(compute_queue_total(&queue), 1_000_000_000_000);

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

        let cap = CapGroup::new(*abs_cap, *rel_cap);
        let result = enforce_cap_group(&cap, *principal, *amount, snapshot.total_assets);

        assert!(
            result.is_ok(),
            "Cap group '{}' should allow allocation of {} to market {}",
            group_id,
            amount,
            target_id
        );
    }
}

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
    assert!(validate_refresh_plan(&plan).is_ok());

    // Simulate refreshing state
    let state = OpState::Refreshing(RefreshingState {
        op_id: 100,
        index: 1,
        plan: plan.targets.clone(),
    });

    // Check recovery from stuck refresh
    let ctx = RecoveryContext::new(1_000_000_000_000);
    let action = determine_recovery_action(&state, &ctx);

    match action {
        RecoveryAction::AbortRefreshing {
            op_id,
            completed_targets,
            remaining_targets,
        } => {
            assert_eq!(op_id, 100);
            assert_eq!(completed_targets.len(), 1);
            assert_eq!(remaining_targets.len(), 2);
        }
        _ => panic!("Expected AbortRefreshing"),
    }
}
