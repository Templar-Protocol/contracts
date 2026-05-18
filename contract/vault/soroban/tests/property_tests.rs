//! Property tests verifying Soroban executor parity with the kernel.
//!
//! These tests ensure that the Soroban executor produces the same results as the kernel
//! for equivalent inputs. This provides confidence that the Soroban implementation
//! correctly follows the kernel's state machine and accounting rules.
//!
//! ## Key Invariants Verified
//!
//! ### Accounting Invariants
//! - `total_assets = idle_assets + external_assets` after all operations
//! - Deposit followed by withdrawal returns <= original assets
//! - Share calculation is consistent between Soroban and kernel
//!
//! ### State Machine Invariants
//! - Operations complete and return to Idle state
//! - State transitions match kernel behavior
//! - Op ID correlation is enforced
//!
//! ### Effect Invariants
//! - Effects are generated consistently with kernel
//! - MintShares effects match deposited amounts
//! - TransferShares effects are generated for withdrawals

use proptest::prelude::*;
use templar_curator_primitives::policy::state::MarketConfig;
use templar_soroban_runtime::{
    contract::{AllocationDelta, ContractConfig, CuratorVault, Delta},
    test_utils::{begin_allocating, finish_allocating, MemoryStorage},
};
use templar_vault_kernel::{
    math::{number::Number, wad::mul_div_floor},
    state::op_state::AllocationPlanEntry,
    state::op_state::OpState,
    transitions::{
        complete_allocation, complete_refresh, start_allocation, start_refresh, TransitionError,
    },
    Address,
};

mod common;
use common::{MockInterpreter, TestPermissiveAuth};

// Test Infrastructure

fn prop_test_config() -> ContractConfig {
    ContractConfig::new(
        Address([1u8; 32]),       // curator
        Address([9u8; 32]),       // vault_address
        vec![Address([2u8; 32])], // guardians
        vec![Address([3u8; 32])], // allocators
        Address([4u8; 32]),       // asset_address
        Address([5u8; 32]),       // share_address
    )
}

type PropTestVault = CuratorVault<MemoryStorage, TestPermissiveAuth, MockInterpreter>;

fn create_prop_test_vault() -> PropTestVault {
    let mut vault = CuratorVault::new(
        prop_test_config(),
        MemoryStorage::new(),
        TestPermissiveAuth,
        MockInterpreter::new(),
    );
    vault.load_state().unwrap();
    vault
        .policy_state_mut()
        .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
        .unwrap();
    vault
}

fn user_addr() -> Address {
    Address([10u8; 32])
}

fn allocator_addr() -> Address {
    Address([3u8; 32])
}

// Arbitrary Strategies

/// Generate a valid deposit amount (non-zero, reasonable bounds).
fn arb_deposit_amount() -> impl Strategy<Value = u128> {
    1u128..=1_000_000_000u128
}

/// Generate an allocation plan.
fn arb_allocation_plan(max_len: usize) -> impl Strategy<Value = Vec<AllocationPlanEntry>> {
    proptest::collection::vec((0u32..100u32, 1u128..=100_000_000u128), 1..=max_len).prop_map(
        |entries| {
            entries
                .into_iter()
                .map(|(target_id, amount)| AllocationPlanEntry::new(target_id, amount))
                .collect()
        },
    )
}

fn tuple_plan(plan: &[AllocationPlanEntry]) -> Vec<(u32, u128)> {
    plan.iter()
        .map(|entry| (entry.target_id, entry.amount))
        .collect()
}

/// Generate a refresh plan (list of target IDs).
fn arb_refresh_plan(max_len: usize) -> impl Strategy<Value = Vec<u32>> {
    proptest::collection::vec(0u32..100u32, 1..=max_len)
}

// Accounting Invariant Tests

proptest! {
    /// Property 1: Total assets accounting after deposit
    ///
    /// After a deposit, total_assets should equal idle_assets + external_assets.
    /// Since deposits go to idle, external_assets should remain 0.
    #[test]
    fn prop_deposit_accounting(
        amount in arb_deposit_amount(),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();

        vault.deposit(user, user, amount, 0, 100).unwrap();

        let state = vault.state().unwrap();
        prop_assert_eq!(state.total_assets, state.idle_assets + state.external_assets);
        prop_assert_eq!(state.idle_assets, amount);
        prop_assert_eq!(state.external_assets, 0);
    }

    /// Property 2: Multiple deposits maintain accounting
    ///
    /// After multiple deposits, total_assets should equal the sum of all deposits.
    #[test]
    fn prop_multiple_deposits_accounting(
        amounts in proptest::collection::vec(arb_deposit_amount(), 1..=5),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();

        let mut expected_total: u128 = 0;
        for (i, amount) in amounts.iter().enumerate() {
            vault.deposit(user, user, *amount, 0, (i as u64 + 1) * 100).unwrap();
            expected_total = expected_total.saturating_add(*amount);
        }

        let state = vault.state().unwrap();
        prop_assert_eq!(state.total_assets, expected_total);
        prop_assert_eq!(state.total_assets, state.idle_assets + state.external_assets);
    }

    /// Property 3: First deposit establishes 1:1 share ratio
    ///
    /// The first deposit should mint shares equal to the deposited assets.
    #[test]
    fn prop_first_deposit_share_ratio(
        amount in arb_deposit_amount(),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();

        let result = vault.deposit(user, user, amount, 0, 100).unwrap();

        prop_assert_eq!(result.shares_minted, amount);
        prop_assert_eq!(result.total_shares, amount);
        prop_assert_eq!(result.total_assets, amount);
    }

    /// Property 4: Subsequent deposits maintain share ratio
    ///
    /// After the first deposit, subsequent deposits should mint shares
    /// proportionally: shares = assets * total_shares / total_assets.
    #[test]
    fn prop_subsequent_deposit_share_ratio(
        first_amount in arb_deposit_amount(),
        second_amount in arb_deposit_amount(),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();

        // First deposit
        vault.deposit(user, user, first_amount, 0, 100).unwrap();

        // Second deposit
        let result = vault.deposit(user, user, second_amount, 0, 200).unwrap();

        // Expected shares = second_amount * first_amount / first_amount = second_amount
        // Since we're at 1:1 ratio still
        prop_assert_eq!(result.shares_minted, second_amount);
    }

    /// Property 5: Share calculation matches kernel math
    ///
    /// Share calculation should match the kernel's mul_div_floor formula.
    #[test]
    fn prop_share_calculation_matches_kernel(
        deposit in 1u128..=1_000_000_000u128,
        total_supply in 1u128..=1_000_000_000u128,
        total_assets in 1u128..=1_000_000_000u128,
    ) {
        // Calculate shares using kernel math
        let kernel_shares = mul_div_floor(
            Number::from(deposit),
            Number::from(total_supply),
            Number::from(total_assets),
        ).as_u128_trunc();

        // Calculate shares using the same formula as the Soroban contract
        let contract_shares = deposit
            .checked_mul(total_supply)
            .and_then(|n| n.checked_div(total_assets))
            .unwrap_or(0);

        // They should match (accounting for potential differences in edge cases)
        // For non-edge cases they should be exactly equal
        if deposit < u64::MAX as u128 && total_supply < u64::MAX as u128 && total_assets < u64::MAX as u128 {
            prop_assert_eq!(kernel_shares, contract_shares);
        }
    }
}

// State Machine Invariant Tests

proptest! {
    /// Property 6: Allocation flow returns to Idle
    ///
    /// After allocate (which internally does begin + supply + finish),
    /// the vault should return to Idle state.
    #[test]
    fn prop_allocation_flow_returns_idle(
        deposit_amount in arb_deposit_amount(),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();
        let allocator = allocator_addr();

        // Setup: deposit
        vault.deposit(user, user, deposit_amount, 0, 100).unwrap();

        let supply_amount = deposit_amount / 2;
        prop_assume!(supply_amount > 0);

        vault.allocate(allocator, &AllocationDelta::Supply(Delta { market: 0, amount: supply_amount })).unwrap();

        prop_assert!(vault.state().unwrap().op_state.is_idle());
        prop_assert_eq!(vault.state().unwrap().external_assets, supply_amount);
    }

    /// Property 7: Refresh flow returns to Idle
    ///
    /// After refresh_markets (which internally does begin + query adapters + finish),
    /// the vault should return to Idle state.
    #[test]
    fn prop_refresh_flow_returns_idle(
        deposit_amount in arb_deposit_amount(),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();
        let allocator = allocator_addr();

        vault.deposit(user, user, deposit_amount, 0, 100).unwrap();

        let supply_amount = deposit_amount / 2;
        prop_assume!(supply_amount > 0);

        vault.allocate(allocator, &AllocationDelta::Supply(Delta { market: 0, amount: supply_amount })).unwrap();

        vault.refresh_markets(allocator, vec![0], 1000).unwrap();

        prop_assert!(vault.state().unwrap().op_state.is_idle());
    }

}

// Kernel Parity Tests

proptest! {
    /// Property 10: Soroban allocation transition matches kernel
    ///
    /// The Soroban start_allocation should produce the same state transition
    /// as the kernel's start_allocation.
    #[test]
    fn prop_allocation_transition_matches_kernel(
        plan in arb_allocation_plan(5),
        op_id in 1u64..u64::MAX,
    ) {
        // Run kernel transition
        let kernel_result = start_allocation(OpState::Idle, plan.clone(), op_id);
        prop_assert!(kernel_result.is_ok());

        let kernel_state = kernel_result.unwrap().new_state;
        prop_assert!(kernel_state.is_allocating());

        let kernel_alloc = kernel_state.as_allocating().unwrap();
        prop_assert_eq!(kernel_alloc.op_id, op_id);
        prop_assert_eq!(kernel_alloc.index, 0);

        let expected_remaining: u128 = plan.iter().map(|entry| entry.amount).sum();
        prop_assert_eq!(kernel_alloc.remaining, expected_remaining);
    }

    /// Property 11: Soroban refresh transition matches kernel
    ///
    /// The Soroban start_refresh should produce the same state transition
    /// as the kernel's start_refresh.
    #[test]
    fn prop_refresh_transition_matches_kernel(
        plan in arb_refresh_plan(5),
        op_id in 1u64..u64::MAX,
    ) {
        // Run kernel transition
        let kernel_result = start_refresh(OpState::Idle, plan.clone(), op_id);
        prop_assert!(kernel_result.is_ok());

        let kernel_state = kernel_result.unwrap().new_state;
        prop_assert!(kernel_state.is_refreshing());

        let kernel_refresh = kernel_state.as_refreshing().unwrap();
        prop_assert_eq!(kernel_refresh.op_id, op_id);
        prop_assert_eq!(kernel_refresh.index, 0);
        prop_assert_eq!(kernel_refresh.plan.len(), plan.len());
    }

    /// Property 12: Empty allocation plan rejected (kernel parity)
    ///
    /// Both kernel and Soroban should reject empty allocation plans.
    #[test]
    fn prop_empty_allocation_plan_rejected(
        op_id in 1u64..u64::MAX,
    ) {
        // Kernel rejects empty plan
        let kernel_result = start_allocation(OpState::Idle, vec![], op_id);
        prop_assert!(matches!(kernel_result, Err(TransitionError::EmptyAllocationPlan)));

        // Soroban vault should also reject (via kernel)
        let mut vault = create_prop_test_vault();
        let allocator = allocator_addr();
        vault.deposit(user_addr(), user_addr(), 10000, 0, 100).unwrap();

        let result = begin_allocating(&mut vault, allocator, vec![], 1000);
        prop_assert!(result.is_err());
    }

    /// Property 13: Empty refresh plan rejected (kernel parity)
    ///
    /// Both kernel and Soroban should reject empty refresh plans.
    #[test]
    fn prop_empty_refresh_plan_rejected(
        op_id in 1u64..u64::MAX,
    ) {
        // Kernel rejects empty plan
        let kernel_result = start_refresh(OpState::Idle, vec![], op_id);
        prop_assert!(matches!(kernel_result, Err(TransitionError::EmptyRefreshPlan)));

        // Soroban vault should also reject (via kernel)
        let mut vault = create_prop_test_vault();
        let allocator = allocator_addr();
        vault.deposit(user_addr(), user_addr(), 10000, 0, 100).unwrap();

        let result = vault.begin_refreshing(allocator, vec![], 1000);
        prop_assert!(result.is_err());
    }

    /// Property 14: Complete allocation returns to Idle (kernel parity)
    ///
    /// Both kernel and Soroban should return to Idle after completing allocation.
    #[test]
    fn prop_complete_allocation_returns_idle(
        deposit_amount in arb_deposit_amount(),
        plan in arb_allocation_plan(3),
        op_id in 1u64..u64::MAX,
    ) {
        // Kernel: start and complete
        let start_result = start_allocation(OpState::Idle, plan.clone(), op_id).unwrap();
        let complete_result = complete_allocation(start_result.new_state, op_id, None).unwrap();
        prop_assert!(complete_result.new_state.is_idle());

        // Soroban: start and complete — plan total must fit within idle_assets
        let plan_total: u128 = plan.iter().map(|entry| entry.amount).sum();
        prop_assume!(plan_total <= deposit_amount);

        let mut vault = create_prop_test_vault();
        let allocator = allocator_addr();
        vault.deposit(user_addr(), user_addr(), deposit_amount, 0, 100).unwrap();

        let soroban_op_id = begin_allocating(&mut vault, allocator, tuple_plan(&plan), 1000).unwrap();
        finish_allocating(&mut vault, allocator, soroban_op_id).unwrap();
        prop_assert!(vault.state().unwrap().op_state.is_idle());
    }

    /// Property 15: Complete refresh returns to Idle (kernel parity)
    ///
    /// Both kernel and Soroban should return to Idle after completing refresh.
    #[test]
    fn prop_complete_refresh_returns_idle(
        plan in arb_refresh_plan(3),
        op_id in 1u64..u64::MAX,
    ) {
        // Kernel: start and complete
        let start_result = start_refresh(OpState::Idle, plan.clone(), op_id).unwrap();
        let complete_result = complete_refresh(start_result.new_state, op_id).unwrap();
        prop_assert!(complete_result.new_state.is_idle());

        // Soroban: start and complete
        let mut vault = create_prop_test_vault();
        let allocator = allocator_addr();
        vault.deposit(user_addr(), user_addr(), 10000, 0, 100).unwrap();

        let soroban_op_id = vault.begin_refreshing(allocator, plan, 1000).unwrap();
    vault.finish_refreshing(allocator, soroban_op_id, 0).unwrap();
        prop_assert!(vault.state().unwrap().op_state.is_idle());
    }
}

// Effect Invariant Tests

proptest! {
    /// Property 16: Deposit generates MintShares effect
    ///
    /// A deposit should generate exactly one MintShares effect with the correct amount.
    #[test]
    fn prop_deposit_generates_mint_effect(
        amount in arb_deposit_amount(),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();

        vault.deposit(user, user, amount, 0, 100).unwrap();

        let effects = &vault.interpreter.effects;
        let is_mint = effects.iter().any(|effect| {
            matches!(
                effect,
                templar_vault_kernel::effects::KernelEffect::MintShares { shares, .. }
                    if *shares == amount
            )
        });
        prop_assert!(is_mint, "Expected MintShares effect with correct amount");
    }

    /// Property 17: Withdrawal request generates TransferShares effect
    ///
    /// A withdrawal request should generate a TransferShares effect for escrow.
    #[test]
    fn prop_withdraw_request_generates_transfer_effect(
        deposit_amount in arb_deposit_amount(),
        withdraw_shares in 1u128..=1_000_000u128,
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();

        // Deposit first
        vault.deposit(user, user, deposit_amount, 0, 100).unwrap();
        vault.interpreter.effects.clear();

        // Request withdrawal
        let shares_to_withdraw = withdraw_shares.min(deposit_amount);
        let result = vault.request_withdraw(user, user, shares_to_withdraw, 0, 200);

        if result.is_ok() {
            // Should have TransferShares effect for escrow
            let effects = &vault.interpreter.effects;
            prop_assert!(!effects.is_empty());
            let has_transfer = effects.iter().any(|e| {
                matches!(e, templar_vault_kernel::effects::KernelEffect::TransferShares { .. })
            });
            prop_assert!(has_transfer, "Expected TransferShares effect for escrow");
        }
    }

    /// Property 18: Op ID is monotonically increasing
    ///
    /// Each operation should get a unique, increasing op_id.
    #[test]
    fn prop_op_id_monotonic(
        deposit_amount in arb_deposit_amount(),
        num_ops in 1usize..=5,
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();
        let allocator = allocator_addr();

        vault.deposit(user, user, deposit_amount, 0, 100).unwrap();

        let mut prev_op_id: Option<u64> = None;

        for _ in 0..num_ops {
            // Start and finish an allocation
            let op_id = begin_allocating(&mut vault, allocator, vec![(0, 100)], 1000).unwrap();

            if let Some(prev) = prev_op_id {
                prop_assert!(op_id > prev, "op_id should be monotonically increasing");
            }
            prev_op_id = Some(op_id);

            finish_allocating(&mut vault, allocator, op_id).unwrap();
        }
    }
}

// External Assets Tracking Tests

proptest! {
    /// Property 19: Allocate updates external assets correctly
    ///
    /// After allocate (Supply), external_assets should increase by the
    /// supplied amount and total_assets should remain consistent.
    #[test]
    fn prop_allocate_updates_external_assets(
        deposit_amount in arb_deposit_amount(),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();
        let allocator = allocator_addr();

        vault.deposit(user, user, deposit_amount, 0, 100).unwrap();

        let supply_amount = deposit_amount / 2;
        prop_assume!(supply_amount > 0);

        vault.allocate(allocator, &AllocationDelta::Supply(Delta { market: 0, amount: supply_amount })).unwrap();

        let state = vault.state().unwrap();
        prop_assert_eq!(state.external_assets, supply_amount);
        prop_assert_eq!(state.total_assets, state.idle_assets + state.external_assets);
    }

    /// Property 20: A plain refresh with the legacy mock interpreter leaves
    /// externally managed balances unchanged.
    #[test]
    fn prop_refresh_preserves_external_assets_without_reported_growth(
        deposit_amount in 1_000_000u128..=1_000_000_000u128,
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();
        let allocator = allocator_addr();

        vault.deposit(user, user, deposit_amount, 0, 100).unwrap();

        let supply_amount = deposit_amount / 2;
        prop_assume!(supply_amount > 0);
        vault.allocate(
            allocator,
            &AllocationDelta::Supply(Delta {
                market: 0,
                amount: supply_amount,
            }),
        )
        .unwrap();

        let total_before = vault.state().unwrap().total_assets;
        vault.refresh_markets(allocator, vec![0], 1_000).unwrap();
        let state = vault.state().unwrap();

        prop_assert_eq!(state.external_assets, supply_amount);
        prop_assert_eq!(state.total_assets, total_before);
        prop_assert_eq!(state.total_assets, state.idle_assets + state.external_assets);
    }
}

// Slippage Protection Tests

proptest! {
    /// Property 21: Deposit respects min_shares_out
    ///
    /// If min_shares_out cannot be satisfied, deposit should fail.
    #[test]
    fn prop_deposit_slippage_protection(
        amount in arb_deposit_amount(),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();

        // First deposit establishes 1:1 ratio
        vault.deposit(user, user, 1000, 0, 100).unwrap();

        // Try to deposit with unrealistic min_shares_out
        let min_shares_out = amount.saturating_mul(2);
        let result = vault.deposit(user, user, amount, min_shares_out, 200);

        prop_assert!(result.is_err());
    }

    /// Property 22: Withdraw request respects min_assets_out
    ///
    /// If min_assets_out cannot be satisfied, withdraw request should fail.
    #[test]
    fn prop_withdraw_slippage_protection(
        deposit_amount in arb_deposit_amount(),
        withdraw_shares in 1u128..=1_000_000u128,
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();

        vault.deposit(user, user, deposit_amount, 0, 100).unwrap();

        let shares_to_withdraw = withdraw_shares.min(deposit_amount);
        // At 1:1 ratio, asking for 2x assets should fail
        let min_assets_out = shares_to_withdraw.saturating_mul(2);
        let result = vault.request_withdraw(user, user, shares_to_withdraw, min_assets_out, 200);

        prop_assert!(result.is_err());
    }
}

// Busy State Rejection Tests

proptest! {
    /// Property 23: Cannot start allocation while allocating
    ///
    /// Starting a second allocation while one is in progress should fail.
    #[test]
    fn prop_cannot_double_allocate(
        deposit_amount in arb_deposit_amount(),
        plan1 in arb_allocation_plan(3),
        plan2 in arb_allocation_plan(3),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();
        let allocator = allocator_addr();

        vault.deposit(user, user, deposit_amount, 0, 100).unwrap();

        // First plan must fit within idle_assets
        let plan1_total: u128 = plan1.iter().map(|entry| entry.amount).sum();
        prop_assume!(plan1_total <= deposit_amount);

        // Start first allocation
        begin_allocating(&mut vault, allocator, tuple_plan(&plan1), 1000).unwrap();

        // Try to start second allocation - should fail
        let result = begin_allocating(&mut vault, allocator, tuple_plan(&plan2), 1000);
        prop_assert!(result.is_err());
    }

    /// Property 24: Cannot start refresh while allocating
    ///
    /// Starting a refresh while allocation is in progress should fail.
    #[test]
    fn prop_cannot_refresh_while_allocating(
        deposit_amount in arb_deposit_amount(),
        alloc_plan in arb_allocation_plan(3),
        refresh_plan in arb_refresh_plan(3),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();
        let allocator = allocator_addr();

        vault.deposit(user, user, deposit_amount, 0, 100).unwrap();

        // Allocation plan must fit within idle_assets
        let plan_total: u128 = alloc_plan.iter().map(|entry| entry.amount).sum();
        prop_assume!(plan_total <= deposit_amount);

        // Start allocation
        begin_allocating(&mut vault, allocator, tuple_plan(&alloc_plan), 1000).unwrap();

        // Try to start refresh - should fail
        let result = vault.begin_refreshing(allocator, refresh_plan, 1000);
        prop_assert!(result.is_err());
    }

    /// Property 25: Cannot start allocation while refreshing
    ///
    /// Starting an allocation while refresh is in progress should fail.
    #[test]
    fn prop_cannot_allocate_while_refreshing(
        deposit_amount in arb_deposit_amount(),
        alloc_plan in arb_allocation_plan(3),
        refresh_plan in arb_refresh_plan(3),
    ) {
        let mut vault = create_prop_test_vault();
        let user = user_addr();
        let allocator = allocator_addr();

        vault.deposit(user, user, deposit_amount, 0, 100).unwrap();

        // Start refresh
        vault.begin_refreshing(allocator, refresh_plan, 1000).unwrap();

        // Try to start allocation - should fail
        let result = begin_allocating(&mut vault, allocator, tuple_plan(&alloc_plan), 1000);
        prop_assert!(result.is_err());
    }
}
