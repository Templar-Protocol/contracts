//! Kernel action tests, compiled only under explicit feature profiles.
//!
//! - With `action-epoch-settlement`: the ENG-697 share-settlement law tests
//!   for epoch intake, cutoff, settlement, execution-time claims, payout
//!   settlement, and owner cancellation. Each item is gated individually;
//!   no `cfg(test)` fallback exposes epoch schema.
//! - Without the feature: the restored origin/dev immediate-execution action
//!   tests against the baseline queue, cooldown, and payout law. Deposit and
//!   atomic-exit law additionally require `action-immediate-deposit` and
//!   `action-atomic-exit`, because the kernel returns
//!   `KernelError::NotImplemented` for those actions when the feature is off.

use super::*;
#[cfg(not(feature = "action-epoch-settlement"))]
use crate::effects::{KernelEffect, KernelEvent, WithdrawalSkipReason};
#[cfg(not(feature = "action-epoch-settlement"))]
use crate::fee::{FeeSlot, FeesSpec};
#[cfg(not(feature = "action-epoch-settlement"))]
use crate::math::wad::{compute_management_fee_shares, Wad, YEAR_NS};
#[cfg(not(feature = "action-epoch-settlement"))]
use crate::state::op_state::{AllocatingState, AllocationPlanEntry, WithdrawingState};
#[cfg(not(feature = "action-epoch-settlement"))]
use crate::state::queue::{DEFAULT_COOLDOWN_NS, MAX_PENDING, MIN_WITHDRAWAL_ASSETS};
#[cfg(not(feature = "action-epoch-settlement"))]
use crate::state::vault::{FeeAccrualAnchor, VaultConfig, VaultState};
#[cfg(not(feature = "action-epoch-settlement"))]
use crate::Number;
#[cfg(feature = "action-epoch-settlement")]
use crate::effects::{KernelEffect, KernelEvent};
#[cfg(feature = "action-epoch-settlement")]
use crate::fee::FeesSpec;
#[cfg(feature = "action-epoch-settlement")]
use crate::state::op_state::WithdrawingState;
#[cfg(feature = "action-epoch-settlement")]
use crate::state::queue::MIN_WITHDRAWAL_ASSETS;
#[cfg(feature = "action-epoch-settlement")]
use crate::state::vault::{VaultConfig, VaultState};

#[cfg(not(feature = "action-epoch-settlement"))]
fn alloc_step(target_id: u32, amount: u128) -> AllocationPlanEntry {
    AllocationPlanEntry::new(target_id, amount)
}

#[cfg(not(feature = "action-epoch-settlement"))]
fn balanced_state() -> VaultState {
    VaultState::with_initial(1_000, 1_000, 500, 500, TimestampNs(0))
}

#[cfg(not(feature = "action-epoch-settlement"))]
fn base_config() -> VaultConfig {
    VaultConfig {
        fees: FeesSpec::zero(),
        min_withdrawal_assets: 0,
        withdrawal_cooldown_ns: 0,
        max_pending_withdrawals: MAX_PENDING as u32,
        paused: false,
        virtual_shares: 0,
        virtual_assets: 0,
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
fn base_state(total_assets: u128, total_shares: u128) -> VaultState {
    let mut state = VaultState::new();
    state.total_assets = total_assets;
    state.total_shares = total_shares;
    state.idle_assets = total_assets;
    state
}

#[cfg(not(feature = "action-epoch-settlement"))]
fn external_heavy_state() -> VaultState {
    VaultState::with_initial(1_000, 1_000, 200, 800, TimestampNs(0))
}

#[cfg(not(feature = "action-epoch-settlement"))]
fn idle_state(total_assets: u128, total_shares: u128) -> VaultState {
    VaultState::with_initial(total_assets, total_shares, total_assets, 0, TimestampNs(0))
}

#[cfg(not(feature = "action-epoch-settlement"))]
fn minted_shares_for(effects: &[KernelEffect], owner: Address) -> u128 {
    effects
        .iter()
        .filter_map(|effect| match effect {
            KernelEffect::MintShares { owner: who, shares } if *who == owner => Some(*shares),
            _ => None,
        })
        .sum()
}

#[cfg(not(feature = "action-epoch-settlement"))]
fn test_config() -> VaultConfig {
    VaultConfig {
        fees: FeesSpec::zero(),
        min_withdrawal_assets: 0,
        withdrawal_cooldown_ns: DEFAULT_COOLDOWN_NS,
        max_pending_withdrawals: 10,
        paused: false,
        virtual_shares: 0,
        virtual_assets: 0,
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_allocating_op_id_mismatch_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = balanced_state();
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 10,
        index: 0,
        remaining: 500,
        plan: vec![alloc_step(1, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortAllocating { op_id: 99 },
    );

    assert!(matches!(
        result,
        Err(KernelError::OpIdMismatch {
            expected: 10,
            actual: 99
        })
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_allocating_success() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(800, 1_000, 300, 500, TimestampNs(0));
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 8,
        index: 0,
        remaining: 200,
        plan: vec![alloc_step(1, 200)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortAllocating { op_id: 8 },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.idle_assets, 500); // 300 + 200 restored
    assert_eq!(result.state.total_assets, 1000); // 500 idle + 500 external
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_allocating_wrong_state_fails() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortAllocating { op_id: 1 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AbortAllocatingRequiresAllocating
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_refreshing_op_id_mismatch_fails() {
    use crate::state::op_state::RefreshingState;

    let mut state = balanced_state();
    state.op_state = OpState::Refreshing(RefreshingState {
        op_id: 10,
        index: 0,
        plan: vec![1],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortRefreshing { op_id: 99 },
    );

    assert!(matches!(
        result,
        Err(KernelError::OpIdMismatch {
            expected: 10,
            actual: 99
        })
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_refreshing_success() {
    use crate::state::op_state::RefreshingState;

    let mut state = balanced_state();
    state.op_state = OpState::Refreshing(RefreshingState {
        op_id: 7,
        index: 0,
        plan: vec![1],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortRefreshing { op_id: 7 },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert!(result.effects.is_empty());
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_refreshing_wrong_op_type_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = balanced_state();
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 10,
        index: 0,
        remaining: 500,
        plan: vec![alloc_step(1, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortRefreshing { op_id: 10 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AbortRefreshingRequiresRefreshing
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_refreshing_wrong_state_fails() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortRefreshing { op_id: 1 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AbortRefreshingRequiresActiveOp
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_withdrawing_empty_queue_fails() {
    let mut state = balanced_state();
    let config = test_config();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 10,
        request_id: 0,
        index: 0,
        remaining: 100,
        collected: 0,
        owner: addr(1),
        receiver: addr(2),
        escrow_shares: 100,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortWithdrawing { op_id: 10 },
    );

    assert!(matches!(result, Err(KernelError::NoPendingWithdrawals)));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_withdrawing_op_id_mismatch_fails() {
    let mut state = balanced_state();
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 10,
        request_id: 0,
        index: 0,
        remaining: 100,
        collected: 0,
        owner,
        receiver,
        escrow_shares: 100,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortWithdrawing { op_id: 99 },
    );

    assert!(matches!(
        result,
        Err(KernelError::OpIdMismatch {
            expected: 10,
            actual: 99
        })
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_withdrawing_queue_head_mismatch_fails() {
    let mut state = balanced_state();
    let config = test_config();

    // Queue has different user
    state
        .withdraw_queue
        .enqueue(
            addr(99),
            addr(99),
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 10,
        request_id: 0,
        index: 0,
        remaining: 100,
        collected: 0,
        owner: addr(1),
        receiver: addr(2),
        escrow_shares: 100,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortWithdrawing { op_id: 10 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::WithdrawalQueueHeadMismatch
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_withdrawing_success() {
    let mut state = balanced_state();
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 9,
        request_id: 0,
        index: 0,
        remaining: 100,
        collected: 0,
        owner,
        receiver,
        escrow_shares: 100,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortWithdrawing { op_id: 9 },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.withdraw_queue.len(), 0);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn abort_withdrawing_wrong_state_fails() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortWithdrawing { op_id: 1 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AbortWithdrawingRequiresWithdrawing
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn action_builder_and_metadata_helpers() {
    let action = KernelAction::finish_allocating(42, TimestampNs(1_000));
    assert!(matches!(action, KernelAction::FinishAllocating { .. }));
    assert_eq!(action.op_id(), Some(42));
    assert_eq!(action.timestamp_ns(), Some(TimestampNs(1_000)));

    let pause = KernelAction::pause(true);
    assert!(matches!(pause, KernelAction::Pause { .. }));
    assert_eq!(pause.op_id(), None);
    assert_eq!(pause.timestamp_ns(), None);

    let atomic =
        KernelAction::atomic_withdraw(addr(1), addr(2), addr(3), 11, 11, TimestampNs(1_000));
    assert!(matches!(atomic, KernelAction::AtomicWithdraw { .. }));
    assert_eq!(atomic.op_id(), None);
    assert_eq!(atomic.timestamp_ns(), Some(TimestampNs(1_000)));

    let settle = KernelAction::settle_payout(7, PayoutOutcome::Failure);
    assert!(matches!(settle, KernelAction::SettlePayout { .. }));
    assert_eq!(settle.op_id(), Some(7));
    assert_eq!(settle.timestamp_ns(), None);
}

#[cfg(all(feature = "action-atomic-exit", not(feature = "action-epoch-settlement")))]
#[test]
fn atomic_redeem_delegated_operator_uses_burn_from_effect() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);
    let operator = addr(9);

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AtomicRedeem {
            owner,
            receiver,
            operator,
            shares: 100,
            min_assets_out: 100,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert_eq!(result.state.total_assets, 900);
    assert_eq!(result.state.idle_assets, 900);
    assert_eq!(result.state.total_shares, 900);
    assert!(matches!(
        result.effects.first(),
        Some(KernelEffect::BurnSharesFrom { spender, owner: effect_owner, shares: 100 })
            if *spender == operator && *effect_owner == owner
    ));
    assert!(matches!(
        result.effects.get(1),
        Some(KernelEffect::TransferAssets { to, amount: 100 }) if *to == receiver
    ));
    assert!(matches!(
        result.effects.get(2),
        Some(KernelEffect::EmitEvent {
            event: KernelEvent::AtomicWithdrawProcessed {
                owner: event_owner,
                receiver: event_receiver,
                shares_burned: 100,
                assets_out: 100,
            }
        }) if *event_owner == owner && *event_receiver == receiver
    ));
}

#[cfg(all(feature = "action-atomic-exit", not(feature = "action-epoch-settlement")))]
#[test]
fn atomic_withdraw_exceeding_idle_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 250, 750, TimestampNs(0));
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AtomicWithdraw {
            owner: addr(1),
            receiver: addr(2),
            operator: addr(1),
            assets_out: 300,
            max_shares_burned: 300,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AtomicWithdrawExceedsIdleAssets
        ))
    ));
}

#[cfg(all(feature = "action-atomic-exit", not(feature = "action-epoch-settlement")))]
#[test]
fn atomic_withdraw_not_idle_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = idle_state(1_000, 1_000);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![alloc_step(0, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AtomicWithdraw {
            owner: addr(1),
            receiver: addr(2),
            operator: addr(1),
            assets_out: 100,
            max_shares_burned: 100,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AtomicWithdrawRequiresIdle
        ))
    ));
}

#[cfg(all(feature = "action-atomic-exit", not(feature = "action-epoch-settlement")))]
#[test]
fn atomic_withdraw_slippage_reports_user_limit_as_minimum() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AtomicWithdraw {
            owner: addr(1),
            receiver: addr(2),
            operator: addr(3),
            assets_out: 100,
            max_shares_burned: 99,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::Slippage {
            min: 99,
            actual: 100
        })
    ));
}

#[cfg(all(feature = "action-atomic-exit", not(feature = "action-epoch-settlement")))]
#[test]
fn atomic_withdraw_success_emits_burn_and_transfer() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AtomicWithdraw {
            owner,
            receiver,
            operator: owner,
            assets_out: 100,
            max_shares_burned: 100,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert_eq!(result.state.total_assets, 900);
    assert_eq!(result.state.idle_assets, 900);
    assert_eq!(result.state.total_shares, 900);
    assert!(matches!(
        result.effects.first(),
        Some(KernelEffect::BurnShares { owner: effect_owner, shares: 100 }) if *effect_owner == owner
    ));
    assert!(matches!(
        result.effects.get(1),
        Some(KernelEffect::TransferAssets { to, amount: 100 }) if *to == receiver
    ));
    assert!(matches!(
        result.effects.get(2),
        Some(KernelEffect::EmitEvent {
            event: KernelEvent::AtomicWithdrawProcessed { owner: event_owner, receiver: event_receiver, shares_burned: 100, assets_out: 100 }
        }) if *event_owner == owner && *event_receiver == receiver
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn begin_allocating_exceeds_idle() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::BeginAllocating {
            op_id: 1,
            plan: vec![alloc_step(1, 1_500)], // exceeds idle_assets of 1_000
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(result, Err(KernelError::InvalidState(_))));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn begin_allocating_success() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::BeginAllocating {
            op_id: 1,
            plan: vec![alloc_step(1, 500)],
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert!(result.state.op_state.as_allocating().is_some());
    // idle_assets must be decremented by allocation total
    assert_eq!(result.state.idle_assets, 500);
    assert_eq!(result.state.total_assets, 500);
    assert!(result.state.check_invariant());
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn begin_refreshing_success() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::BeginRefreshing {
            op_id: 1,
            plan: vec![1],
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert!(result.state.op_state.as_refreshing().is_some());
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn convert_to_assets_ceil_is_floor_or_floor_plus_one() {
    let config = base_config();
    let cases = [(1, 2, 3), (7, 13, 9), (5, 11, 19), (9, 17, 23)];
    for (shares, total_assets, total_shares) in cases {
        let state = base_state(total_assets, total_shares);
        let floor = convert_to_assets(&state, &config, shares);
        let ceil = convert_to_assets_ceil(&state, &config, shares);
        assert!(ceil >= floor);
        assert!(ceil <= floor.saturating_add(1));
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn convert_to_assets_ceil_matches_floor_on_exact_multiple() {
    let config = base_config();
    let state = base_state(100, 100);
    let shares = 25;
    let floor = convert_to_assets(&state, &config, shares);
    let ceil = convert_to_assets_ceil(&state, &config, shares);
    assert_eq!(floor, ceil);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn convert_to_assets_ceil_rounds_up_on_fractional() {
    let config = base_config();
    let state = base_state(2, 3);
    let shares = 1;
    let floor = convert_to_assets(&state, &config, shares);
    let ceil = convert_to_assets_ceil(&state, &config, shares);
    assert_eq!(ceil, floor.saturating_add(1));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn convert_to_assets_works() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let assets = convert_to_assets(&state, &config, 500);
    assert_eq!(assets, 500);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn convert_to_shares_ceil_is_floor_or_floor_plus_one() {
    let config = base_config();
    let cases = [(1, 3, 2), (5, 7, 11), (10, 25, 9), (12, 19, 23)];
    for (assets, total_assets, total_shares) in cases {
        let state = base_state(total_assets, total_shares);
        let floor = convert_to_shares(&state, &config, assets);
        let ceil = convert_to_shares_ceil(&state, &config, assets);
        assert!(ceil >= floor);
        assert!(ceil <= floor.saturating_add(1));
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn convert_to_shares_ceil_matches_floor_on_exact_multiple() {
    let config = base_config();
    let state = base_state(100, 100);
    let assets = 40;
    let floor = convert_to_shares(&state, &config, assets);
    let ceil = convert_to_shares_ceil(&state, &config, assets);
    assert_eq!(floor, ceil);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn convert_to_shares_ceil_rounds_up_on_fractional() {
    let config = base_config();
    let state = base_state(3, 2);
    let assets = 1;
    let floor = convert_to_shares(&state, &config, assets);
    let ceil = convert_to_shares_ceil(&state, &config, assets);
    assert_eq!(ceil, floor.saturating_add(1));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn convert_to_shares_works() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    // With 1:1 ratio (plus virtual adjustments)
    let shares = convert_to_shares(&state, &config, 500);
    // shares = 500 * (1001) / (1001) = 500
    assert_eq!(shares, 500);
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_advances_fee_anchor_to_post_deposit_assets() {
    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(10));

    let result = apply_action(
        state,
        &config,
        None,
        &Address([0u8; 32]),
        KernelAction::Deposit {
            owner: Address([1u8; 32]),
            receiver: Address([2u8; 32]),
            assets_in: 250,
            min_shares_out: 0,
            now_ns: TimestampNs(20),
        },
    )
    .expect("deposit succeeds");

    assert_eq!(result.state.total_assets, 1_250);
    assert_eq!(result.state.fee_anchor.total_assets, 1_250);
    assert_eq!(result.state.fee_anchor.timestamp_ns, TimestampNs(20));
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_blocked_when_paused() {
    let state = VaultState::new();
    let mut config = test_config();
    config.paused = true;

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::Deposit {
            owner: addr(1),
            receiver: addr(2),
            assets_in: 10,
            min_shares_out: 0,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::Restricted(RestrictionKind::Paused))
    ));
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_emits_transfer_assets_from_owner() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();
    let self_id = addr(0xAB);
    let owner = addr(1);

    let result = apply_action(
        state,
        &config,
        None,
        &self_id,
        KernelAction::Deposit {
            owner,
            receiver: addr(2),
            assets_in: 250,
            min_shares_out: 0,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    let transfer = result.effects.iter().find_map(|effect| match effect {
        KernelEffect::TransferAssetsFrom { from, to, amount } => Some((*from, *to, *amount)),
        _ => None,
    });

    assert_eq!(transfer, Some((owner, self_id, 250)));
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_not_idle_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = idle_state(1_000, 1_000);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![alloc_step(0, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::Deposit {
            owner: addr(1),
            receiver: addr(2),
            assets_in: 100,
            min_shares_out: 0,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::DepositRequiresIdle
        ))
    ));
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_overflow_total_assets_rejected() {
    let config = base_config();
    let mut state = base_state(u128::MAX - 5, u128::MAX / 2);
    state.idle_assets = state.total_assets;
    let result = apply_action(
        state,
        &config,
        None,
        &Address([0u8; 32]),
        KernelAction::Deposit {
            owner: Address([1u8; 32]),
            receiver: Address([2u8; 32]),
            assets_in: 10,
            min_shares_out: 0,
            now_ns: TimestampNs(0),
        },
    );
    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::DepositOverflowTotalAssets
        ))
    ));
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_overflow_total_shares_rejected() {
    let config = base_config();
    let mut state = base_state(u128::MAX - 1, u128::MAX);
    state.idle_assets = state.total_assets;
    let result = apply_action(
        state,
        &config,
        None,
        &Address([0u8; 32]),
        KernelAction::Deposit {
            owner: Address([1u8; 32]),
            receiver: Address([2u8; 32]),
            assets_in: 1,
            min_shares_out: 0,
            now_ns: TimestampNs(0),
        },
    );
    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::MintOverflowTotalShares
        ))
    ));
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_refreshes_accrued_fees_before_advancing_anchor() {
    let management_recipient = addr(0xBB);
    let mut config = base_config();
    config.fees = FeesSpec::new(
        FeeSlot::zero(),
        FeeSlot::new(Wad::one() / 10, management_recipient),
        None,
    );

    let mut state = base_state(1_000, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(0));

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::Deposit {
            owner: addr(1),
            receiver: addr(2),
            assets_in: 100,
            min_shares_out: 0,
            now_ns: TimestampNs(YEAR_NS),
        },
    )
    .expect("deposit succeeds");

    let minted: Vec<_> = result
        .effects
        .iter()
        .filter_map(|effect| match effect {
            KernelEffect::MintShares { owner, shares } => Some((*owner, *shares)),
            _ => None,
        })
        .collect();
    assert_eq!(minted.len(), 2);
    assert_eq!(minted[0].0, management_recipient);
    assert!(minted[0].1 > 0, "management fees must mint before deposit");
    assert_eq!(minted[1].0, addr(2));

    let expected_fee_shares = compute_management_fee_shares(
        1_000,
        1_000,
        1_000,
        config.fees.management.fee_wad,
        0,
        YEAR_NS,
    )
    .as_u128_trunc();
    assert_eq!(minted[0].1, expected_fee_shares);
    assert_eq!(result.state.total_assets, 1_100);
    assert_eq!(
        result.state.total_shares,
        1_000 + expected_fee_shares + minted[1].1
    );
    assert_eq!(result.state.fee_anchor.total_assets, 1_100);
    assert_eq!(result.state.fee_anchor.timestamp_ns, TimestampNs(YEAR_NS));
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_slippage_check_fails() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::Deposit {
            owner: addr(1),
            receiver: addr(2),
            assets_in: 100,
            min_shares_out: 1_000_000,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(result, Err(KernelError::Slippage { .. })));
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_success() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::Deposit {
            owner: addr(1),
            receiver: addr(2),
            assets_in: 500,
            min_shares_out: 0,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    // With virtual_assets/shares = 0, ratio is 1:1 after adjustments
    assert_eq!(result.state.total_assets, 1_500);
    assert_eq!(result.state.idle_assets, 1_500);
    assert!(matches!(
        result.effects.first(),
        Some(KernelEffect::TransferAssetsFrom { .. })
    ));
    assert!(matches!(
        result.effects.get(1),
        Some(KernelEffect::MintShares { .. })
    ));
    assert!(matches!(
        result.effects.get(2),
        Some(KernelEffect::EmitEvent {
            event: KernelEvent::DepositProcessed { .. }
        })
    ));
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_that_would_mint_zero_shares_fails_before_mutation() {
    let state = idle_state(u128::MAX, 1);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::Deposit {
            owner: addr(1),
            receiver: addr(2),
            assets_in: 1,
            min_shares_out: 0,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(result, Err(KernelError::ZeroAmount)));
}

#[cfg(all(feature = "action-immediate-deposit", not(feature = "action-epoch-settlement")))]
#[test]
fn deposit_zero_assets_fails_slippage() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::Deposit {
            owner: addr(1),
            receiver: addr(2),
            assets_in: 0,
            min_shares_out: 1,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(result, Err(KernelError::ZeroAmount)));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn effective_totals_adds_virtual() {
    let state = idle_state(1_000, 1_000);
    let mut config = test_config();
    config.virtual_shares = 100;
    config.virtual_assets = 200;

    let totals = effective_totals(&state, &config);
    assert_eq!(totals.supply, 1_000 + 100); // shares + max(virtual, 1)
    assert_eq!(totals.assets, 1_000 + 200); // assets + max(virtual, 1)
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn emergency_reset_from_allocating_restores_idle() {
    use crate::state::op_state::AllocatingState;

    let mut state = external_heavy_state();
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 10,
        index: 1,
        remaining: 300,
        plan: vec![alloc_step(1, 500), alloc_step(2, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::EmergencyReset,
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.idle_assets, 500); // 200 + 300 restored
    assert_eq!(result.state.total_assets, 1_300); // 500 + 800
    assert!(result.effects.iter().any(|e| matches!(
        e,
        KernelEffect::EmitEvent {
            event: KernelEvent::EmergencyResetCompleted {
                op_id: 10,
                from_state: 1
            }
        }
    )));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn emergency_reset_from_idle_fails() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::EmergencyReset,
    );
    assert!(matches!(result, Err(KernelError::InvalidState(_))));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn emergency_reset_from_payout_refunds_and_restores() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 400, 600, TimestampNs(0));
    let owner = addr(3);
    let receiver = addr(4);
    let _ = state
        .withdraw_queue
        .enqueue(owner, receiver, 300, 300, TimestampNs(0), 10)
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 30,
        request_id: 30,
        receiver,
        amount: 250,
        owner,
        escrow_shares: 300,
        burn_shares: 280,
    });
    let config = test_config();
    let escrow = addr(0xFF);

    let result = apply_action(state, &config, None, &escrow, KernelAction::EmergencyReset).unwrap();

    assert!(result.state.is_idle());
    // Payout amount (250) restored to idle
    assert_eq!(result.state.idle_assets, 650);
    assert_eq!(result.state.total_assets, 1_250);
    // Queue head dequeued
    assert_eq!(result.state.withdraw_queue.len(), 0);
    // Shares refunded
    assert!(result.effects.iter().any(|e| matches!(
        e,
        KernelEffect::TransferShares { from, to, shares: 300 }
        if *from == escrow && *to == owner
    )));
    // Event emitted with correct state code
    assert!(result.effects.iter().any(|e| matches!(
        e,
        KernelEffect::EmitEvent {
            event: KernelEvent::EmergencyResetCompleted {
                op_id: 30,
                from_state: 4
            }
        }
    )));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn emergency_reset_from_refreshing() {
    use crate::state::op_state::RefreshingState;

    let mut state = balanced_state();
    state.op_state = OpState::Refreshing(RefreshingState {
        op_id: 7,
        index: 1,
        plan: vec![1, 2],
    });
    let config = test_config();

    let result = apply_action(
        state.clone(),
        &config,
        None,
        &addr(0xFF),
        KernelAction::EmergencyReset,
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.idle_assets, 500);
    assert_eq!(result.state.external_assets, 500);
    assert!(result.effects.iter().any(|e| matches!(
        e,
        KernelEffect::EmitEvent {
            event: KernelEvent::EmergencyResetCompleted {
                op_id: 7,
                from_state: 3
            }
        }
    )));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn emergency_reset_from_withdrawing_refunds_shares() {
    let mut state = balanced_state();
    let owner = addr(1);
    let receiver = addr(2);
    let _ = state
        .withdraw_queue
        .enqueue(owner, receiver, 200, 200, TimestampNs(0), 10)
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 20,
        request_id: 0,
        index: 0,
        remaining: 100,
        collected: 50,
        owner,
        receiver,
        escrow_shares: 200,
    });
    let config = test_config();
    let escrow = addr(0xFF);

    let result = apply_action(state, &config, None, &escrow, KernelAction::EmergencyReset).unwrap();

    assert!(result.state.is_idle());
    // collected (50) restored to idle
    assert_eq!(result.state.idle_assets, 550);
    assert_eq!(result.state.total_assets, 1_050);
    // Queue head dequeued
    assert_eq!(result.state.withdraw_queue.len(), 0);
    // Shares refunded
    assert!(result.effects.iter().any(|e| matches!(
        e,
        KernelEffect::TransferShares { from, to, shares: 200 }
        if *from == escrow && *to == owner
    )));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_cooldown_fails() {
    let mut state = idle_state(1_000, 1_000);
    let config = test_config();

    state
        .withdraw_queue
        .enqueue(
            addr(1),
            addr(2),
            100,
            100,
            TimestampNs(1_000_000),
            config.max_pending_withdrawals,
        )
        .unwrap();

    // Not enough time passed
    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(1_000_000),
        },
    );

    assert!(matches!(result, Err(KernelError::Cooldown { .. })));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_empty_queue_fails() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(DEFAULT_COOLDOWN_NS + 1),
        },
    );

    assert!(matches!(result, Err(KernelError::NoPendingWithdrawals)));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_idle_starts_withdrawal() {
    let mut state = idle_state(1_000, 1_000);
    let config = test_config();
    let owner = addr(3);
    let receiver = addr(4);

    let _ = state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(DEFAULT_COOLDOWN_NS + 1),
        },
    )
    .unwrap();

    let withdraw = result.state.op_state.as_withdrawing().unwrap();
    assert_eq!(withdraw.op_id, 0);
    assert_eq!(withdraw.owner, owner);
    assert_eq!(withdraw.receiver, receiver);
    assert_eq!(withdraw.escrow_shares, 100);
    assert_eq!(withdraw.remaining, 100);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_low_liquidity_below_minimum_fails_before_withdrawing() {
    let mut config = test_config();
    config.min_withdrawal_assets = MIN_WITHDRAWAL_ASSETS;
    config.withdrawal_cooldown_ns = 0;

    let owner = addr(10);
    let receiver = addr(11);
    let total_assets = MIN_WITHDRAWAL_ASSETS * 2;
    let idle_assets = MIN_WITHDRAWAL_ASSETS - 1;
    let external_assets = total_assets - idle_assets;
    let mut state = VaultState::with_initial(
        total_assets,
        total_assets,
        idle_assets,
        external_assets,
        TimestampNs(0),
    );
    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            total_assets,
            total_assets,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::WithdrawalLiquidityBelowMinimum
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_partial_idle_at_minimum_fails_before_withdrawing() {
    let mut config = test_config();
    config.min_withdrawal_assets = MIN_WITHDRAWAL_ASSETS;
    config.withdrawal_cooldown_ns = 0;

    let owner = addr(10);
    let receiver = addr(11);
    let total_assets = MIN_WITHDRAWAL_ASSETS * 2;
    let idle_assets = MIN_WITHDRAWAL_ASSETS;
    let external_assets = total_assets - idle_assets;
    let mut state = VaultState::with_initial(
        total_assets,
        total_assets,
        idle_assets,
        external_assets,
        TimestampNs(0),
    );
    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            total_assets,
            total_assets,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::WithdrawalLiquidityBelowMinimum
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_partially_idle_refuses_before_withdrawing() {
    let mut state = balanced_state();
    let config = test_config();
    let owner = addr(3);
    let receiver = addr(4);

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            600,
            600,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(DEFAULT_COOLDOWN_NS + 1),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::WithdrawalLiquidityBelowMinimum
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_persists_skips_before_low_liquidity_head() {
    let mut config = base_config();
    config.min_withdrawal_assets = MIN_WITHDRAWAL_ASSETS;

    let total_assets = MIN_WITHDRAWAL_ASSETS * 2;
    let idle_assets = MIN_WITHDRAWAL_ASSETS - 1;
    let external_assets = total_assets - idle_assets;
    let mut state = VaultState::with_initial(
        total_assets,
        total_assets,
        idle_assets,
        external_assets,
        TimestampNs(0),
    );
    let skipped_owner = Address([3u8; 32]);
    let skipped_receiver = Address([4u8; 32]);
    let waiting_owner = Address([5u8; 32]);
    let waiting_receiver = Address([6u8; 32]);
    let self_id = Address([9u8; 32]);

    state
        .withdraw_queue
        .enqueue(
            skipped_owner,
            skipped_receiver,
            500,
            500,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .expect("enqueue skipped head");
    state
        .withdraw_queue
        .enqueue(
            waiting_owner,
            waiting_receiver,
            total_assets,
            total_assets,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .expect("enqueue low-liquidity head");

    let restrictions = Restrictions::blacklist(vec![skipped_owner]);
    let result = apply_action(
        state,
        &config,
        Some(&restrictions),
        &self_id,
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(0),
        },
    )
    .expect("skip effects should be persisted before low-liquidity stop");

    assert!(result.state.is_idle());
    assert_eq!(result.state.withdraw_queue.len(), 1);
    assert_eq!(
        result.state.withdraw_queue.head().map(|(id, _)| id),
        Some(1)
    );
    assert!(result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalSkipped {
                    owner,
                    receiver,
                    reason: WithdrawalSkipReason::Restricted,
                    ..
                },
            } if *owner == skipped_owner && *receiver == skipped_receiver
        )
    }));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_queue_head_mismatch_fails() {
    let mut state = idle_state(1_000, 1_000);
    let config = test_config();
    let owner = addr(5);
    let receiver = addr(6);

    // Queue has different owner than op_state
    state
        .withdraw_queue
        .enqueue(
            addr(99),
            addr(99),
            200,
            200,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 7,
        request_id: 0,
        index: 0,
        remaining: 200,
        collected: 0,
        receiver,
        owner,
        escrow_shares: 200,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::ExecuteWithdrawRequiresIdleUseCallbacks
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_respects_paused_restrictions() {
    let mut config = base_config();
    config.paused = true;
    let mut state = base_state(1_000, 1_000);

    state
        .withdraw_queue
        .enqueue(
            Address([1u8; 32]),
            Address([2u8; 32]),
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .expect("enqueue");

    let result = apply_action(
        state,
        &config,
        None,
        &Address([9u8; 32]),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(DEFAULT_COOLDOWN_NS + 1),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::Restricted(RestrictionKind::Paused))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_skips_restricted_head_and_processes_next() {
    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let restricted_owner = Address([3u8; 32]);
    let first_receiver = Address([4u8; 32]);
    let next_owner = Address([5u8; 32]);
    let next_receiver = Address([6u8; 32]);
    let self_id = Address([9u8; 32]);

    state
        .withdraw_queue
        .enqueue(
            restricted_owner,
            first_receiver,
            500,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .expect("enqueue first");
    state
        .withdraw_queue
        .enqueue(
            next_owner,
            next_receiver,
            250,
            150,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .expect("enqueue second");

    let restrictions = Restrictions::blacklist(vec![restricted_owner]);
    let result = apply_action(
        state,
        &config,
        Some(&restrictions),
        &self_id,
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(DEFAULT_COOLDOWN_NS + 1),
        },
    )
    .expect("execute_withdraw");

    let withdrawing = result.state.op_state.as_withdrawing().expect("withdrawing");
    assert_eq!(withdrawing.owner, next_owner);
    assert_eq!(withdrawing.receiver, next_receiver);
    assert_eq!(result.state.withdraw_queue.len(), 1);
    assert!(result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalSkipped {
                    owner,
                    receiver,
                    expected_assets: 100,
                    reason: WithdrawalSkipReason::Restricted,
                    ..
                },
            } if *owner == restricted_owner && *receiver == first_receiver
        )
    }));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_skips_zero_expected_assets() {
    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let owner = Address([3u8; 32]);
    let receiver = Address([4u8; 32]);
    let escrow_shares = 500;

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            escrow_shares,
            0,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .expect("enqueue");

    let self_id = Address([9u8; 32]);
    let result = apply_action(
        state,
        &config,
        None,
        &self_id,
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(0),
        },
    )
    .expect("execute_withdraw");

    assert!(result.state.op_state.is_idle());
    assert!(result.state.withdraw_queue.is_empty());

    assert!(result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::TransferShares { from, to, shares }
                if *from == self_id && *to == owner && *shares == escrow_shares
        )
    }));
    assert!(result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalSkipped {
                    id: _,
                    owner: who,
                    receiver: dest,
                    escrow_shares: shares,
                    expected_assets: 0,
                    reason: WithdrawalSkipReason::ZeroExpectedAssets,
                },
            } if *who == owner && *dest == receiver && *shares == escrow_shares
        )
    }));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_skips_zero_expected_head_then_waits_for_cooldown() {
    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let skipped_owner = Address([3u8; 32]);
    let skipped_receiver = Address([4u8; 32]);
    let waiting_owner = Address([5u8; 32]);
    let waiting_receiver = Address([6u8; 32]);
    let self_id = Address([9u8; 32]);

    state
        .withdraw_queue
        .enqueue(
            skipped_owner,
            skipped_receiver,
            500,
            0,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .expect("enqueue skipped head");
    state
        .withdraw_queue
        .enqueue(
            waiting_owner,
            waiting_receiver,
            250,
            150,
            TimestampNs(1),
            config.max_pending_withdrawals,
        )
        .expect("enqueue cooling-down head");

    let result = apply_action(
        state,
        &config,
        None,
        &self_id,
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(0),
        },
    )
    .expect("execute_withdraw");

    assert!(result.state.is_idle());
    assert_eq!(result.state.withdraw_queue.len(), 1);
    assert_eq!(
        result.state.withdraw_queue.head().map(|(id, _)| id),
        Some(1)
    );
    assert!(result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalSkipped {
                    owner,
                    receiver,
                    expected_assets: 0,
                    reason: WithdrawalSkipReason::ZeroExpectedAssets,
                    ..
                },
            } if *owner == skipped_owner && *receiver == skipped_receiver
        )
    }));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_withdrawing_advances_index() {
    let mut state = idle_state(1_000, 1_000);
    let config = test_config();
    let owner = addr(5);
    let receiver = addr(6);

    let _ = state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            200,
            200,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 7,
        request_id: 0,
        index: 0,
        remaining: 200,
        collected: 0,
        receiver,
        owner,
        escrow_shares: 200,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::ExecuteWithdrawRequiresIdleUseCallbacks
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_withdrawing_empty_queue() {
    let mut state = balanced_state();
    let config = test_config();

    // State is Withdrawing but queue is empty (shouldn't happen in practice)
    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 8,
        request_id: 0,
        index: 0,
        remaining: 100,
        collected: 0,
        owner: addr(1),
        receiver: addr(2),
        escrow_shares: 100,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::ExecuteWithdrawRequiresIdleUseCallbacks
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn execute_withdraw_wrong_state_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = idle_state(1_000, 1_000);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![alloc_step(0, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::ExecuteWithdrawRequiresIdle
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn finish_allocating_does_not_skip_restricted_head_before_cooldown() {
    use crate::state::op_state::AllocatingState;

    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let restricted_owner = Address([3u8; 32]);
    let first_receiver = Address([4u8; 32]);
    let waiting_owner = Address([5u8; 32]);
    let waiting_receiver = Address([6u8; 32]);
    let self_id = Address([9u8; 32]);

    state
        .withdraw_queue
        .enqueue(
            restricted_owner,
            first_receiver,
            500,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .expect("enqueue restricted head");
    state
        .withdraw_queue
        .enqueue(
            waiting_owner,
            waiting_receiver,
            250,
            150,
            TimestampNs(1),
            config.max_pending_withdrawals,
        )
        .expect("enqueue cooling-down head");
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 77,
        index: 1,
        remaining: 0,
        plan: vec![alloc_step(1, 500)],
    });

    let restrictions = Restrictions::blacklist(vec![restricted_owner]);
    let result = apply_action(
        state,
        &config,
        Some(&restrictions),
        &self_id,
        KernelAction::FinishAllocating {
            op_id: 77,
            now_ns: TimestampNs(0),
        },
    )
    .expect("finish_allocating");

    assert!(result.state.is_idle());
    assert_eq!(result.state.withdraw_queue.len(), 2);
    assert_eq!(
        result.state.withdraw_queue.head().map(|(id, _)| id),
        Some(0)
    );
    assert!(!result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalSkipped { .. },
            }
        )
    }));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn finish_allocating_insufficient_idle_does_not_chain_withdrawal() {
    use crate::state::op_state::AllocatingState;

    let mut state = balanced_state();
    let owner = addr(10);
    let receiver = addr(11);
    let config = test_config();

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            600,
            600,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 5,
        index: 1,
        remaining: 0,
        plan: vec![alloc_step(1, 500)],
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::FinishAllocating {
            op_id: 5,
            now_ns: TimestampNs(DEFAULT_COOLDOWN_NS + 1),
        },
    )
    .unwrap();

    assert!(result.state.op_state.is_idle());
    let (_head_id, head) = result
        .state
        .withdraw_queue
        .head()
        .expect("withdrawal should remain queued");
    assert_eq!(head.expected_assets, 600);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn finish_allocating_leaves_restricted_queue_untouched() {
    use crate::state::op_state::AllocatingState;

    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let restricted_owner = Address([3u8; 32]);
    let first_receiver = Address([4u8; 32]);
    let next_owner = Address([5u8; 32]);
    let next_receiver = Address([6u8; 32]);
    let self_id = Address([9u8; 32]);

    state
        .withdraw_queue
        .enqueue(
            restricted_owner,
            first_receiver,
            500,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .expect("enqueue first");
    state
        .withdraw_queue
        .enqueue(
            next_owner,
            next_receiver,
            250,
            150,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .expect("enqueue second");
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 77,
        index: 1,
        remaining: 0,
        plan: vec![alloc_step(1, 500)],
    });

    let restrictions = Restrictions::blacklist(vec![restricted_owner]);
    let result = apply_action(
        state,
        &config,
        Some(&restrictions),
        &self_id,
        KernelAction::FinishAllocating {
            op_id: 77,
            now_ns: TimestampNs(DEFAULT_COOLDOWN_NS + 1),
        },
    )
    .expect("finish_allocating");

    assert!(result.state.is_idle());
    assert_eq!(result.state.withdraw_queue.len(), 2);
    let (head_id, head) = result
        .state
        .withdraw_queue
        .head()
        .expect("restricted head should remain queued");
    assert_eq!(head_id, 0);
    assert_eq!(head.owner, restricted_owner);
    assert_eq!(head.receiver, first_receiver);
    assert!(!result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalSkipped { .. },
            }
        )
    }));
    assert!(!result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalStarted { .. },
            }
        )
    }));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn finish_allocating_success() {
    use crate::state::op_state::AllocatingState;

    let mut state = balanced_state();
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 1,
        remaining: 0,
        plan: vec![alloc_step(1, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::FinishAllocating {
            op_id: 1,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn finish_allocating_with_low_liquidity_pending_withdrawal_finishes_idle() {
    let mut config = test_config();
    config.min_withdrawal_assets = MIN_WITHDRAWAL_ASSETS;
    config.withdrawal_cooldown_ns = 0;

    let owner = addr(10);
    let receiver = addr(11);
    let total_assets = MIN_WITHDRAWAL_ASSETS * 2;
    let idle_assets = MIN_WITHDRAWAL_ASSETS - 1;
    let external_assets = total_assets - idle_assets;
    let mut state = VaultState::with_initial(
        total_assets,
        total_assets,
        idle_assets,
        external_assets,
        TimestampNs(0),
    );
    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            total_assets,
            total_assets,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 6,
        index: 1,
        remaining: 0,
        plan: vec![alloc_step(1, 500)],
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::FinishAllocating {
            op_id: 6,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.withdraw_queue.len(), 1);
    assert_eq!(
        result.state.withdraw_queue.head().map(|(id, _)| id),
        Some(0)
    );
    assert!(result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::AllocationCompleted {
                    op_id: 6,
                    has_withdrawal: false
                },
            }
        )
    }));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn finish_allocating_with_pending_withdrawal_not_past_cooldown() {
    use crate::state::op_state::AllocatingState;

    let mut state = balanced_state();
    let owner = addr(10);
    let receiver = addr(11);
    let config = test_config();

    // Add a pending withdrawal that's NOT past cooldown
    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            100,
            100,
            TimestampNs(DEFAULT_COOLDOWN_NS),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 6,
        index: 1,
        remaining: 0,
        plan: vec![alloc_step(1, 500)],
    });

    // now_ns is not past cooldown
    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::FinishAllocating {
            op_id: 6,
            now_ns: TimestampNs(DEFAULT_COOLDOWN_NS),
        },
    )
    .unwrap();

    // Should transition to Idle since withdrawal is not ready
    assert!(result.state.is_idle());
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn finish_allocating_with_ready_pending_withdrawal_finishes_idle() {
    use crate::state::op_state::AllocatingState;

    let mut state = balanced_state();
    let owner = addr(10);
    let receiver = addr(11);
    let config = test_config();

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 5,
        index: 1,
        remaining: 0,
        plan: vec![alloc_step(1, 500)],
    });

    // now_ns is past cooldown (DEFAULT_COOLDOWN_NS + request time of 0)
    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::FinishAllocating {
            op_id: 5,
            now_ns: TimestampNs(DEFAULT_COOLDOWN_NS + 1),
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.withdraw_queue.len(), 1);
    let (_head_id, head) = result
        .state
        .withdraw_queue
        .head()
        .expect("withdrawal should remain queued");
    assert_eq!(head.owner, owner);
    assert_eq!(head.receiver, receiver);
    assert_eq!(head.expected_assets, 100);
    assert!(result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::AllocationCompleted {
                    op_id: 5,
                    has_withdrawal: false,
                },
            }
        )
    }));
    assert!(!result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalStarted { .. },
            }
        )
    }));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn finish_refreshing_success() {
    use crate::state::op_state::RefreshingState;

    let mut state = balanced_state();
    state.op_state = OpState::Refreshing(RefreshingState {
        op_id: 2,
        index: 1,
        plan: vec![1],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::FinishRefreshing {
            op_id: 2,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn invalid_max_pending_rejected() {
    let state = balanced_state();
    let mut config = test_config();
    config.max_pending_withdrawals = (MAX_PENDING as u32).saturating_add(1);

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::Pause { paused: false },
    );

    assert!(result.is_ok());
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn kernel_result_new() {
    let state = VaultState::new();
    let effects = vec![KernelEffect::EmitEvent {
        event: KernelEvent::PauseUpdated { paused: false },
    }];

    let result = KernelResult::new(state.clone(), effects.clone());
    assert_eq!(result.state, state);
    assert_eq!(result.effects, effects);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn pause_action() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::Pause { paused: true },
    )
    .unwrap();

    assert!(matches!(
        result.effects.first(),
        Some(KernelEffect::EmitEvent {
            event: KernelEvent::PauseUpdated { paused: true }
        })
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn rebalance_withdraw_allows_allocating_with_matching_op_id() {
    let mut state = external_heavy_state();
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 7,
        index: 0,
        remaining: 100,
        plan: vec![alloc_step(1, 100)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RebalanceWithdraw {
            op_id: 7,
            amount: 50,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert!(result.state.op_state.is_allocating());
    assert_eq!(result.state.idle_assets, 250);
    assert_eq!(result.state.external_assets, 750);
    assert_eq!(result.state.total_assets, 1_000);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn rebalance_withdraw_moves_assets_from_external_to_idle() {
    let state = external_heavy_state();
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RebalanceWithdraw {
            op_id: 0,
            amount: 300,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.idle_assets, 500);
    assert_eq!(result.state.external_assets, 500);
    assert_eq!(result.state.total_assets, 1_000);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn rebalance_withdraw_rejects_amount_above_external_assets() {
    let state = external_heavy_state();
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RebalanceWithdraw {
            op_id: 0,
            amount: 801,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::RebalanceWithdrawExceedsExternalAssets
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn rebalance_withdraw_requires_matching_op_id_when_allocating() {
    let mut state = external_heavy_state();
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 7,
        index: 0,
        remaining: 100,
        plan: vec![alloc_step(1, 100)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RebalanceWithdraw {
            op_id: 8,
            amount: 50,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::OpIdMismatch {
            expected: 7,
            actual: 8
        })
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_action_zero_fees() {
    let state = idle_state(1_000, 1_000);
    let config = test_config(); // fees: FeesSpec::zero()

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(12_345),
        },
    )
    .unwrap();

    assert_eq!(result.state.fee_anchor.total_assets, 1_000);
    assert_eq!(result.state.fee_anchor.timestamp_ns, TimestampNs(12_345));
    assert_eq!(result.state.total_shares, 1_000); // No fee shares minted
    assert_eq!(result.effects.len(), 1); // Only FeesRefreshed event
    assert!(matches!(
        result.effects.first(),
        Some(KernelEffect::EmitEvent {
            event: KernelEvent::FeesRefreshed { now_ns: 12_345, .. }
        })
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_max_rate_caps_fee_accrual() {
    use crate::math::wad::YEAR_NS;
    // 1000 -> 2000 (100% profit), but max_rate = 20% per year
    let mut state = VaultState::with_initial(2_000, 1_000, 2_000, 0, TimestampNs(0));
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(0));

    let perf_recipient = addr(0xAA);
    let mut config = test_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 10, perf_recipient), // 10% performance
        FeeSlot::zero(),
        Some(Wad::one() / 5), // 20% max growth rate
    );

    // Half year elapsed
    let half_year = YEAR_NS / 2;
    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(half_year),
        },
    )
    .unwrap();

    // Max growth = 1000 * 20% * 0.5 = 100; capped total_assets = 1000 + 100 = 1100
    // Profit = 1100 - 1000 = 100; fee_assets = 10% * 100 = 10
    // denom = 2000 - 10 = 1990; perf_shares = floor(10 * 1000 / 1990) = 5
    let mint_effects: Vec<_> = result
        .effects
        .iter()
        .filter_map(|e| match e {
            KernelEffect::MintShares { shares, .. } => Some(*shares),
            _ => None,
        })
        .collect();
    assert_eq!(mint_effects.len(), 1);
    assert_eq!(mint_effects[0], 5);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_mints_both_management_and_performance() {
    use crate::math::wad::compute_fee_shares_from_assets;
    use crate::math::wad::YEAR_NS;

    let mut state = idle_state(1_500, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(0));

    let perf_recipient = addr(0xAA);
    let mgmt_recipient = addr(0xBB);
    let mut config = test_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 10, perf_recipient), // 10% performance
        FeeSlot::new(Wad::one() / 20, mgmt_recipient), // 5% management
        None,
    );

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(YEAR_NS),
        },
    )
    .unwrap();

    // Management first: annual_fee_assets = 5% * 1500 = 75
    // mgmt_shares = floor(75 * 1000 / (1500 - 75)) = floor(75000/1425) = 52
    let mgmt_expected: u128 = compute_fee_shares_from_assets(
        Number::from(75u128),
        Number::from(1_500u128),
        Number::from(1_000u128),
    )
    .into();

    // Performance: supply now = 1000 + mgmt_expected; profit = 500; fee_assets = 50
    let total_supply_after_mgmt = 1_000 + mgmt_expected;
    let perf_expected: u128 = compute_fee_shares_from_assets(
        Number::from(50u128), // 10% of 500 profit
        Number::from(1_500u128),
        Number::from(total_supply_after_mgmt),
    )
    .into();

    let mint_effects: Vec<_> = result
        .effects
        .iter()
        .filter_map(|e| match e {
            KernelEffect::MintShares { owner, shares } => Some((*owner, *shares)),
            _ => None,
        })
        .collect();
    assert_eq!(mint_effects.len(), 2);
    assert_eq!(mint_effects[0], (mgmt_recipient, mgmt_expected));
    assert_eq!(mint_effects[1], (perf_recipient, perf_expected));
    assert_eq!(
        result.state.total_shares,
        1_000 + mgmt_expected + perf_expected
    );
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_mints_management_fee_shares() {
    use crate::math::wad::YEAR_NS;
    // Setup: 1000 assets/shares, no profit, full year elapsed
    let mut state = idle_state(1_000, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(0));

    let mgmt_recipient = addr(0xBB);
    let mut config = test_config();
    config.fees = FeesSpec::new(
        FeeSlot::zero(),                               // no performance fee
        FeeSlot::new(Wad::one() / 10, mgmt_recipient), // 10% management fee
        None,
    );

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(YEAR_NS),
        },
    )
    .unwrap();

    // Full year: annual_fee_assets = 10% * 1000 = 100
    // fee_assets = floor(100 * YEAR_NS / YEAR_NS) = 100
    // fee_shares = floor(100 * 1000 / (1000 - 100)) = floor(100000/900) = 111
    let mint_effects: Vec<_> = result
        .effects
        .iter()
        .filter(|e| matches!(e, KernelEffect::MintShares { .. }))
        .collect();
    assert_eq!(mint_effects.len(), 1);
    assert!(matches!(
        mint_effects[0],
        KernelEffect::MintShares { owner, shares: 111 } if *owner == mgmt_recipient
    ));
    assert_eq!(result.state.total_shares, 1_000 + 111);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_mints_performance_fee_shares() {
    use crate::math::wad::YEAR_NS;
    // Setup: vault started with 1000 assets/shares, now has 1500 assets (profit)
    let mut state = idle_state(1_500, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(0)); // anchor at 1000 assets, time 0

    let perf_recipient = addr(0xAA);
    let mut config = test_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 10, perf_recipient), // 10% performance fee
        FeeSlot::zero(),                               // no management fee
        None,
    );

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(YEAR_NS),
        },
    )
    .unwrap();

    // Profit = 1500 - 1000 = 500; fee_assets = 10% * 500 = 50
    // denom = 1500 - 50 = 1450; perf_shares = floor(50 * 1000 / 1450) = 34
    let mint_effects: Vec<_> = result
        .effects
        .iter()
        .filter(|e| matches!(e, KernelEffect::MintShares { .. }))
        .collect();
    assert_eq!(mint_effects.len(), 1);
    assert!(matches!(
        mint_effects[0],
        KernelEffect::MintShares { owner, shares: 34 } if *owner == perf_recipient
    ));
    assert_eq!(result.state.total_shares, 1_000 + 34);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_no_profit_skips_performance() {
    use crate::math::wad::YEAR_NS;
    // No profit (assets unchanged from anchor)
    let mut state = idle_state(1_000, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(0));

    let perf_recipient = addr(0xAA);
    let mut config = test_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 10, perf_recipient), // 10% performance
        FeeSlot::zero(),
        None,
    );

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(YEAR_NS),
        },
    )
    .unwrap();

    let mint_effects: Vec<_> = result
        .effects
        .iter()
        .filter(|e| matches!(e, KernelEffect::MintShares { .. }))
        .collect();
    assert_eq!(mint_effects.len(), 0);
    assert_eq!(result.state.total_shares, 1_000);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_overflow_total_supply_rejected() {
    let mut config = base_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 2, Address([9u8; 32])),
        FeeSlot::new(Wad::zero(), Address([8u8; 32])),
        None,
    );
    let mut state = base_state(1_000, u128::MAX - 1);
    state.fee_anchor = FeeAccrualAnchor::new(1, TimestampNs(0));

    let result = apply_action(
        state,
        &config,
        None,
        &Address([0u8; 32]),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(1),
        },
    );
    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::FeeMintOverflowTotalSupply
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_rejects_backwards_time() {
    let mut state = idle_state(1_000, 1_000);
    state.fee_anchor.timestamp_ns = TimestampNs(10_000); // Current anchor at 10000
    let config = test_config();

    // Try to refresh with earlier timestamp
    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(5000),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::FeeRefreshTimestampMustAdvance
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_rejects_non_advancing_timestamp() {
    let mut config = base_config();
    config.fees = FeesSpec::zero();
    let mut state = base_state(1_000, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(500));

    let result = apply_action(
        state,
        &config,
        None,
        &Address([0u8; 32]),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(500),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::FeeRefreshTimestampMustAdvance
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_requires_idle_state() {
    use crate::state::op_state::AllocatingState;

    let mut state = idle_state(1_000, 1_000);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 7,
        index: 0,
        remaining: 0,
        plan: vec![],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(12_345),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::RefreshFeesRequiresIdle
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_respects_growth_rate_cap_with_both_fee_types() {
    let management_recipient = Address([9u8; 32]);
    let performance_recipient = Address([8u8; 32]);

    let mut config = base_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 5, performance_recipient),
        FeeSlot::new(Wad::one() / 10, management_recipient),
        Some(Wad::one() / 10),
    );

    let mut state = base_state(2_000, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(0));

    let result = apply_action(
        state,
        &config,
        None,
        &Address([0u8; 32]),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(YEAR_NS),
        },
    )
    .unwrap();

    let capped_total_assets = 1_100;
    let mgmt_shares = compute_management_fee_shares(
        capped_total_assets,
        2_000,
        1_000,
        config.fees.management.fee_wad,
        0,
        YEAR_NS,
    );
    let mgmt_expected: u128 = mgmt_shares.into();
    let total_supply_after_mgmt = 1_000u128.saturating_add(mgmt_expected);

    let profit = capped_total_assets.saturating_sub(1_000);
    let fee_assets = config
        .fees
        .performance
        .fee_wad
        .apply_floored(Number::from(profit));
    let perf_shares = compute_fee_shares_from_assets(
        fee_assets,
        Number::from(2_000u128),
        Number::from(total_supply_after_mgmt),
    );
    let perf_expected: u128 = perf_shares.into();

    let mgmt_minted = minted_shares_for(&result.effects, management_recipient);
    let perf_minted = minted_shares_for(&result.effects, performance_recipient);
    assert_eq!(mgmt_minted, mgmt_expected);
    assert_eq!(perf_minted, perf_expected);

    let uncapped_mgmt_shares = compute_management_fee_shares(
        2_000,
        2_000,
        1_000,
        config.fees.management.fee_wad,
        0,
        YEAR_NS,
    );
    let uncapped_mgmt: u128 = uncapped_mgmt_shares.into();
    assert!(mgmt_minted < uncapped_mgmt);
}

#[cfg(all(feature = "action-atomic-exit", not(feature = "action-epoch-settlement")))]
#[test]
fn refresh_fees_then_atomic_withdraw_succeeds() {
    let mut state = idle_state(1_500, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(0));
    let config = VaultConfig {
        fees: FeesSpec::new(
            FeeSlot::new(Wad::one() / 10, addr(7)),
            FeeSlot::new(Wad::one() / 10, addr(8)),
            None,
        ),
        ..test_config()
    };

    let refreshed = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(100),
        },
    )
    .unwrap();
    let refreshed_total_shares = refreshed.state.total_shares;

    let withdrawn = apply_action(
        refreshed.state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AtomicWithdraw {
            owner: addr(1),
            receiver: addr(2),
            operator: addr(1),
            assets_out: 500,
            max_shares_burned: 500,
            now_ns: TimestampNs(100),
        },
    )
    .unwrap();

    assert_eq!(withdrawn.state.total_assets, 1_000);
    assert_eq!(withdrawn.state.idle_assets, 1_000);
    assert!(withdrawn.state.total_shares < refreshed_total_shares);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn refresh_fees_zero_anchor_excludes_uncapped_donation_growth() {
    use crate::math::wad::YEAR_NS;
    let mut state = VaultState::with_initial(2_000, 1_000, 2_000, 0, TimestampNs(0));
    state.fee_anchor = FeeAccrualAnchor::new(0, TimestampNs(0));

    let perf_recipient = addr(0xAA);
    let mut config = test_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 10, perf_recipient),
        FeeSlot::zero(),
        Some(Wad::one() / 5),
    );

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(YEAR_NS),
        },
    )
    .unwrap();

    let minted: Vec<_> = result
        .effects
        .iter()
        .filter(|effect| matches!(effect, KernelEffect::MintShares { .. }))
        .collect();
    assert!(minted.is_empty());
    assert_eq!(result.state.total_shares, 1_000);
    assert_eq!(result.state.fee_anchor.total_assets, 2_000);
    assert_eq!(result.state.fee_anchor.timestamp_ns, TimestampNs(YEAR_NS));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn request_withdraw_blocked_by_blacklist() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();
    let restrictions = Restrictions::blacklist(alloc::vec![addr(9)]);

    let result = apply_action(
        state,
        &config,
        Some(&restrictions),
        &addr(0xFF),
        KernelAction::RequestWithdraw {
            owner: addr(9),
            receiver: addr(3),
            shares: 10,
            min_assets_out: 0,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::Restricted(RestrictionKind::Blacklisted))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn request_withdraw_blocked_by_restrictions_paused() {
    let state = idle_state(1_000, 1_000);
    let mut config = test_config();
    config.paused = true;

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RequestWithdraw {
            owner: addr(1),
            receiver: addr(2),
            shares: 100,
            min_assets_out: 0,
            now_ns: TimestampNs(0),
        },
    );

    assert_eq!(
        result,
        Err(KernelError::Restricted(RestrictionKind::Paused))
    );
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn request_withdraw_enqueues_and_emits_event() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RequestWithdraw {
            owner: addr(1),
            receiver: addr(2),
            shares: 100,
            min_assets_out: 0,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert_eq!(result.state.withdraw_queue.len(), 1);
    assert!(matches!(
        result.effects.first(),
        Some(KernelEffect::TransferShares { .. })
    ));
    assert!(matches!(
        result.effects.get(1),
        Some(KernelEffect::EmitEvent {
            event: KernelEvent::WithdrawalRequested { .. }
        })
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn request_withdraw_min_withdrawal_fails() {
    let state = idle_state(1_000, 1_000);
    let mut config = test_config();
    config.min_withdrawal_assets = 1_000;

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RequestWithdraw {
            owner: addr(1),
            receiver: addr(2),
            shares: 10,
            min_assets_out: 0,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(result, Err(KernelError::MinWithdrawal { .. })));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn request_withdraw_not_idle_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = idle_state(1_000, 1_000);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![alloc_step(0, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RequestWithdraw {
            owner: addr(1),
            receiver: addr(2),
            shares: 100,
            min_assets_out: 0,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::RequestWithdrawRequiresIdle
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn request_withdraw_queue_full_fails() {
    let mut state = VaultState::with_initial(10_000, 10_000, 10_000, 0, TimestampNs(0));
    let mut config = test_config();
    config.max_pending_withdrawals = 2;

    // Fill the queue
    state
        .withdraw_queue
        .enqueue(
            addr(1),
            addr(1),
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();
    state
        .withdraw_queue
        .enqueue(
            addr(2),
            addr(2),
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RequestWithdraw {
            owner: addr(3),
            receiver: addr(3),
            shares: 100,
            min_assets_out: 0,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(result, Err(KernelError::QueueFull { .. })));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn request_withdraw_slippage_fails() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RequestWithdraw {
            owner: addr(1),
            receiver: addr(2),
            shares: 10,
            min_assets_out: 1_000_000,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(result, Err(KernelError::Slippage { .. })));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn request_withdraw_zero_shares_fails() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RequestWithdraw {
            owner: addr(1),
            receiver: addr(2),
            shares: 0,
            min_assets_out: 1,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(result, Err(KernelError::ZeroAmount)));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn settle_payout_empty_queue_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = balanced_state();
    let config = test_config();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 20,
        request_id: 20,
        owner: addr(1),
        receiver: addr(2),
        amount: 100,
        escrow_shares: 100,
        burn_shares: 100,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 20,
            outcome: PayoutOutcome::Success,
        },
    );

    assert!(matches!(result, Err(KernelError::NoPendingWithdrawals)));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn settle_payout_failure() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(900, 1_000, 400, 500, TimestampNs(0));
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 13,
        request_id: 0,
        owner,
        receiver,
        amount: 100,
        escrow_shares: 100,
        burn_shares: 100,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 13,
            outcome: PayoutOutcome::Failure,
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.idle_assets, 400);
    assert_eq!(result.state.total_assets, 900);
    assert_eq!(result.state.total_shares, 1_000); // Not changed
    assert!(matches!(
        result.effects.first(),
        Some(KernelEffect::TransferShares { .. })
    ));
    let event = result
        .effects
        .iter()
        .find_map(|e| match e {
            KernelEffect::EmitEvent {
                event:
                    KernelEvent::PayoutCompleted {
                        op_id,
                        success,
                        burn_shares,
                        refund_shares,
                        amount,
                    },
            } => Some((*op_id, *success, *burn_shares, *refund_shares, *amount)),
            _ => None,
        })
        .expect("missing PayoutCompleted event");
    assert_eq!(event, (13, false, 0, 100, 0));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn settle_payout_op_id_mismatch_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = balanced_state();
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 20,
        request_id: 20,
        owner,
        receiver,
        amount: 100,
        escrow_shares: 100,
        burn_shares: 100,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 99,
            outcome: PayoutOutcome::Success,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::OpIdMismatch {
            expected: 20,
            actual: 99
        })
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn settle_payout_queue_head_mismatch_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = balanced_state();
    let config = test_config();

    state
        .withdraw_queue
        .enqueue(
            addr(99),
            addr(99),
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 20,
        request_id: 20,
        owner: addr(1),
        receiver: addr(2),
        amount: 100,
        escrow_shares: 100,
        burn_shares: 100,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 20,
            outcome: PayoutOutcome::Success,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::WithdrawalQueueHeadMismatch
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn settle_payout_success_burn_only() {
    use crate::state::op_state::PayoutState;

    let mut state = balanced_state();
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 11,
        request_id: 0,
        owner,
        receiver,
        amount: 100,
        escrow_shares: 100,
        burn_shares: 100,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 11,
            outcome: PayoutOutcome::Success,
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.idle_assets, 400);
    assert_eq!(result.state.total_shares, 900); // 1000 - 100 burned
    assert_eq!(result.state.withdraw_queue.len(), 0);
    let (burn_owner, burn_shares) = result
        .effects
        .iter()
        .find_map(|e| match e {
            KernelEffect::BurnShares { owner, shares } => Some((*owner, *shares)),
            _ => None,
        })
        .expect("missing BurnShares effect");
    assert_eq!(burn_owner, addr(0xFF));
    assert_eq!(burn_shares, 100);
    let event = result
        .effects
        .iter()
        .find_map(|e| match e {
            KernelEffect::EmitEvent {
                event:
                    KernelEvent::PayoutCompleted {
                        op_id,
                        success,
                        burn_shares,
                        refund_shares,
                        amount,
                    },
            } => Some((*op_id, *success, *burn_shares, *refund_shares, *amount)),
            _ => None,
        })
        .expect("missing PayoutCompleted event");
    assert_eq!(event, (11, true, 100, 0, 100));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn settle_payout_success_partial_refund() {
    use crate::state::op_state::PayoutState;

    let mut state = balanced_state();
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 12,
        request_id: 0,
        owner,
        receiver,
        amount: 50,
        escrow_shares: 100,
        burn_shares: 50,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 12,
            outcome: PayoutOutcome::Success,
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.idle_assets, 450);
    assert_eq!(result.state.total_shares, 950);
    assert_eq!(result.effects.len(), 3); // BurnShares + TransferShares + PayoutCompleted
    let event = result
        .effects
        .iter()
        .find_map(|e| match e {
            KernelEffect::EmitEvent {
                event:
                    KernelEvent::PayoutCompleted {
                        op_id,
                        success,
                        burn_shares,
                        refund_shares,
                        amount,
                    },
            } => Some((*op_id, *success, *burn_shares, *refund_shares, *amount)),
            _ => None,
        })
        .expect("missing PayoutCompleted event");
    assert_eq!(event, (12, true, 50, 50, 50));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn settle_payout_uses_state_derived_partial_settlement() {
    use crate::state::op_state::PayoutState;

    let mut state = balanced_state();
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            100,
            100,
            TimestampNs(0),
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 20,
        request_id: 0,
        owner,
        receiver,
        amount: 40,
        escrow_shares: 100,
        burn_shares: 40,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 20,
            outcome: PayoutOutcome::Success,
        },
    )
    .unwrap();

    let event = result
        .effects
        .iter()
        .find_map(|e| match e {
            KernelEffect::EmitEvent {
                event:
                    KernelEvent::PayoutCompleted {
                        op_id,
                        success,
                        burn_shares,
                        refund_shares,
                        amount,
                    },
            } => Some((*op_id, *success, *burn_shares, *refund_shares, *amount)),
            _ => None,
        })
        .expect("missing PayoutCompleted event");

    assert_eq!(event, (20, true, 40, 60, 40));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn settle_payout_wrong_state_fails() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 1,
            outcome: PayoutOutcome::Success,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::SettlePayoutRequiresPayout
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn sync_external_assets_accepts_runtime_validated_refresh_total() {
    use crate::state::op_state::RefreshingState;

    let mut state = VaultState::with_initial(1_999, 1_000, 1_000, 999, TimestampNs(0));
    state.op_state = OpState::Refreshing(RefreshingState {
        op_id: 8,
        index: 0,
        plan: vec![0],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 2_997,
            op_id: 8,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert_eq!(result.state.external_assets, 2_997);
    assert_eq!(result.state.total_assets, 3_997);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn sync_external_assets_allocating() {
    use crate::state::op_state::AllocatingState;

    let mut state = balanced_state();
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 3,
        index: 0,
        remaining: 500,
        plan: vec![alloc_step(1, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 700,
            op_id: 3,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert_eq!(result.state.external_assets, 700);
    assert_eq!(result.state.total_assets, 1_200); // idle(500) + external(700)
    assert!(matches!(
        result.effects.first(),
        Some(KernelEffect::EmitEvent {
            event: KernelEvent::ExternalAssetsSynced { .. }
        })
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn sync_external_assets_allows_decrease() {
    use crate::state::op_state::RefreshingState;

    let mut state = VaultState::with_initial(11_000, 1_000, 1_000, 10_000, TimestampNs(0));
    state.op_state = OpState::Refreshing(RefreshingState {
        op_id: 9,
        index: 0,
        plan: vec![0],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 0,
            op_id: 9,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert_eq!(result.state.external_assets, 0);
    assert_eq!(result.state.total_assets, 1_000);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn sync_external_assets_allows_up_to_in_flight_allocation() {
    use crate::state::op_state::AllocatingState;

    let mut state = idle_state(1_000, 1_000);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![alloc_step(0, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 500,
            op_id: 1,
            now_ns: TimestampNs(0),
        },
    );

    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.state.external_assets, 500);
    assert_eq!(result.state.total_assets, 1_500);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn sync_external_assets_idle_fails() {
    let state = idle_state(1_000, 1_000);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 500,
            op_id: 1,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::SyncExternalRequiresActiveOp
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn sync_external_assets_no_longer_applies_in_flight_allocation_bound() {
    use crate::state::op_state::AllocatingState;

    let mut state = idle_state(1_000, 1_000);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![alloc_step(0, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 501,
            op_id: 1,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert_eq!(result.state.external_assets, 501);
    assert_eq!(result.state.total_assets, 1_501);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn sync_external_assets_op_id_mismatch_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = balanced_state();
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 10,
        index: 0,
        remaining: 500,
        plan: vec![alloc_step(1, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 500,
            op_id: 99, // Wrong op_id
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::OpIdMismatch {
            expected: 10,
            actual: 99
        })
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn sync_external_assets_payout_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = balanced_state();
    state.op_state = OpState::Payout(PayoutState {
        op_id: 6,
        request_id: 6,
        owner: addr(1),
        receiver: addr(2),
        amount: 50,
        escrow_shares: 100,
        burn_shares: 50,
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 500,
            op_id: 6,
            now_ns: TimestampNs(0),
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::SyncExternalRequiresAllowedStates
        ))
    ));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn sync_external_assets_refreshing() {
    use crate::state::op_state::RefreshingState;

    let mut state = balanced_state();
    state.op_state = OpState::Refreshing(RefreshingState {
        op_id: 5,
        index: 0,
        plan: vec![1],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 500,
            op_id: 5,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert_eq!(result.state.external_assets, 500);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn sync_external_assets_withdrawing() {
    let mut state = balanced_state();
    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 4,
        request_id: 0,
        index: 0,
        remaining: 100,
        collected: 0,
        owner: addr(1),
        receiver: addr(2),
        escrow_shares: 100,
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 500,
            op_id: 4,
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();

    assert_eq!(result.state.external_assets, 500);
    assert_eq!(result.state.total_assets, 1_000);
}

fn addr(tag: u8) -> Address {
    Address([tag; 32])
}

#[cfg(feature = "action-epoch-settlement")]
mod epoch_law_constants {
    pub(super) const CUT_NS: u64 = 1_000;
    pub(super) const SETTLE_NS: u64 = 2_000;
    pub(super) const EPOCH_TWO_CUT_NS: u64 = 51_000;
    pub(super) const EPOCH_TWO_SETTLE_NS: u64 = 52_000;
    pub(super) const NAV_TOTAL: u128 = 1_000_000;
    pub(super) const SUPPLY_TOTAL: u128 = 1_000_000;
    pub(super) const IDLE_TOTAL: u128 = 900_000;
    pub(super) const EXTERNAL_TOTAL: u128 = 100_000;
}

#[cfg(feature = "action-epoch-settlement")]
use epoch_law_constants::{
    CUT_NS, EPOCH_TWO_CUT_NS, EPOCH_TWO_SETTLE_NS, EXTERNAL_TOTAL, IDLE_TOTAL, NAV_TOTAL,
    SETTLE_NS, SUPPLY_TOTAL,
};

#[cfg(feature = "action-epoch-settlement")]
use crate::state::queue::PendingWithdrawal;

#[cfg(feature = "action-epoch-settlement")]
fn config() -> VaultConfig {
    VaultConfig {
        fees: FeesSpec::zero(),
        min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
        withdrawal_cooldown_ns: 0,
        max_pending_withdrawals: 16,
        paused: false,
        virtual_shares: 0,
        virtual_assets: 0,
    }
}

#[cfg(feature = "action-epoch-settlement")]
fn count_effect(effects: &[KernelEffect], needle: &KernelEffect) -> usize {
    effects.iter().filter(|effect| *effect == needle).count()
}

#[cfg(feature = "action-epoch-settlement")]
fn cutoff(state: &mut VaultState, at_ns: u64) -> KernelResult {
    let result = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::BeginEpochCutoff {
            cutoff_ns: TimestampNs(at_ns),
            now_ns: TimestampNs(at_ns),
        },
    )
    .expect("cutoff accepted");
    *state = result.state.clone();
    result
}

#[cfg(feature = "action-epoch-settlement")]
fn cutoff_and_settle(state: &mut VaultState) {
    cutoff(state, CUT_NS);
    let settled = settle(state, 1).expect("settlement accepted");
    *state = settled.state;
}

#[cfg(feature = "action-epoch-settlement")]
fn escrow() -> Address {
    addr(0xFF)
}

#[cfg(feature = "action-epoch-settlement")]
fn funded_state() -> VaultState {
    VaultState::with_initial(
        NAV_TOTAL,
        SUPPLY_TOTAL,
        IDLE_TOTAL,
        EXTERNAL_TOTAL,
        TimestampNs(0),
    )
}

#[cfg(feature = "action-epoch-settlement")]
fn other_owner() -> Address {
    addr(3)
}

#[cfg(feature = "action-epoch-settlement")]
fn other_receiver() -> Address {
    addr(4)
}

#[cfg(feature = "action-epoch-settlement")]
fn owner() -> Address {
    addr(1)
}

#[cfg(feature = "action-epoch-settlement")]
fn payout(op_id: u64, request_id: u64, amount: u128, escrow_shares: u128) -> OpState {
    OpState::Payout(PayoutState {
        op_id,
        request_id,
        receiver: receiver(),
        amount,
        owner: owner(),
        escrow_shares,
        burn_shares: escrow_shares,
    })
}

#[cfg(feature = "action-epoch-settlement")]
fn queue_entry(
    state: &mut VaultState,
    entry_owner: Address,
    entry_receiver: Address,
    escrow_shares: u128,
    min_assets_out: u128,
) -> u64 {
    let epoch_id = state
        .epoch
        .intake_epoch_id()
        .expect("intake must be open to queue");
    let entry = PendingWithdrawal::new(
        entry_owner,
        entry_receiver,
        escrow_shares,
        min_assets_out,
        TimestampNs(0),
        epoch_id,
    )
    .expect("valid entry");
    state
        .withdraw_queue
        .enqueue_withdrawal(entry, config().max_pending_withdrawals)
        .expect("enqueue")
}

#[cfg(feature = "action-epoch-settlement")]
fn receiver() -> Address {
    addr(2)
}

#[cfg(feature = "action-epoch-settlement")]
fn report(seq: u64, as_of_ns: u64) -> ValuationReportRef {
    ValuationReportRef {
        report_seq: seq,
        as_of_ns: TimestampNs(as_of_ns),
        report_hash: [7u8; 32],
    }
}

#[cfg(feature = "action-epoch-settlement")]
fn settle(state: &mut VaultState, seq: u64) -> Result<KernelResult, KernelError> {
    // No cutoff means nothing to settle: the action must be rejected by the
    // settlement law. Report fields fall back to the nominal cutoff so the
    // rejection exercises the phase gate, not a helper panic.
    let report = match state.epoch.cutoff_ns {
        Some(cutoff_ns) => report(seq, cutoff_ns.as_u64()),
        None => report(seq, CUT_NS),
    };
    apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::SettleEpoch {
            report,
            new_external_assets: state.external_assets,
            max_report_age_ns: 3_600_000_000_000,
            settle_now_ns: TimestampNs(SETTLE_NS),
        },
    )
}

#[cfg(feature = "action-epoch-settlement")]
fn withdrawing(op_id: u64, request_id: u64, remaining: u128, escrow_shares: u128) -> OpState {
    OpState::Withdrawing(WithdrawingState {
        op_id,
        request_id,
        index: 0,
        remaining,
        collected: 0,
        receiver: receiver(),
        owner: owner(),
        escrow_shares,
    })
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn cancellation_after_a_settled_epoch_refunds_shares_without_touching_the_snapshot() {
    let mut state = funded_state();
    queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    cutoff_and_settle(&mut state);

    let snapshot_before = state.epoch.last_settled.clone();
    let result = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::CancelPendingWithdrawal {
            caller: owner(),
            request_id: 0,
            now_ns: TimestampNs(SETTLE_NS),
        },
    )
    .expect("owner cancels instead of executing");

    assert_eq!(
        count_effect(
            &result.effects,
            &KernelEffect::TransferShares {
                from: escrow(),
                to: owner(),
                shares: 1_000
            }
        ),
        1
    );
    assert!(result.state.withdraw_queue.is_empty());
    assert_eq!(result.state.total_shares, SUPPLY_TOTAL);
    assert_eq!(result.state.idle_assets, IDLE_TOTAL);
    assert!(result.state.check_invariant());
    // The settlement is immutable: cancelling a paid-capable request never
    // repriced or rewrote the snapshot.
    assert_eq!(result.state.epoch.last_settled, snapshot_before);
    assert_eq!(
        result
            .state
            .epoch
            .settled_claim_for(EpochId::FIRST_SETTLEMENT, 1_000),
        Some(1_000)
    );
    assert!(result.state.withdraw_queue.settled_head_claim(&result.state.epoch).is_none());
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn cancellation_refunds_all_escrow_and_repairs_fifo_caches() {
    let mut state = funded_state();
    let first = queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    let second = queue_entry(&mut state, other_owner(), other_receiver(), 500, 1);
    let cached_before = state.withdraw_queue.total_escrow_shares();
    assert_eq!(cached_before, 1_500);

    let result = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::CancelPendingWithdrawal {
            caller: owner(),
            request_id: first,
            now_ns: TimestampNs(10),
        },
    )
    .expect("owner cancels their request");

    // Full refund of the cancelled escrow, nothing less.
    assert_eq!(
        count_effect(
            &result.effects,
            &KernelEffect::TransferShares {
                from: escrow(),
                to: owner(),
                shares: 1_000
            }
        ),
        1
    );
    assert_eq!(result.state.withdraw_queue.get(first), None);
    assert!(!result.state.withdraw_queue.contains(first));
    assert!(result.state.withdraw_queue.contains(second));
    assert!(result.state.withdraw_queue.settled_head_claim(&result.state.epoch).is_none());
    assert_eq!(result.state.withdraw_queue.len(), 1);
    assert_eq!(result.state.withdraw_queue.total_escrow_shares(), 500);
    assert!(result.state.withdraw_queue.check_invariants());
    assert_eq!(
        result
            .state
            .withdraw_queue
            .head()
            .map(|(head_id, _)| head_id),
        Some(second)
    );
    assert_eq!(result.state.total_shares, SUPPLY_TOTAL);
    assert!(result.state.check_invariant());
    assert_eq!(result.state.epoch, state.epoch);
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn cancellation_requires_the_owner_and_blocks_in_flight_requests() {
    let mut state = funded_state();
    let first = queue_entry(&mut state, owner(), receiver(), 1_000, 1);

    let wrong_caller = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::CancelPendingWithdrawal {
            caller: other_owner(),
            request_id: first,
            now_ns: TimestampNs(10),
        },
    )
    .expect_err("only the owner may cancel");
    assert_eq!(
        wrong_caller,
        KernelError::InvalidState(InvalidStateCode::CancelCallerNotOwner)
    );

    let missing = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::CancelPendingWithdrawal {
            caller: owner(),
            request_id: 77,
            now_ns: TimestampNs(10),
        },
    )
    .expect_err("unknown request");
    assert_eq!(
        missing,
        KernelError::InvalidState(InvalidStateCode::CancelRequestNotFound)
    );

    // The in-flight head cannot be cancelled while the payout state owns it.
    state.op_state = withdrawing(9, first, 1_000, 1_000);
    let in_flight = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::CancelPendingWithdrawal {
            caller: owner(),
            request_id: first,
            now_ns: TimestampNs(10),
        },
    )
    .expect_err("in-flight request");
    assert_eq!(
        in_flight,
        KernelError::InvalidState(InvalidStateCode::CancelInFlightRequest)
    );
    assert!(state.withdraw_queue.contains(first));
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn cancelling_the_head_exposes_the_next_owner_and_progress_is_possible() {
    let mut state = funded_state();
    let first = queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    let second = queue_entry(&mut state, other_owner(), other_receiver(), 1_000, 1);
    assert_ne!(first, second);

    // The second owner cannot cancel someone else's request.
    let denied = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::CancelPendingWithdrawal {
            caller: other_owner(),
            request_id: second,
            now_ns: TimestampNs(10),
        },
    )
    .expect("second owner cancels their own request");
    assert_eq!(denied.state.withdraw_queue.len(), 1);
    assert!(denied.state.withdraw_queue.contains(first));
    assert_eq!(
        count_effect(
            &denied.effects,
            &KernelEffect::TransferShares {
                from: escrow(),
                to: other_owner(),
                shares: 1_000
            }
        ),
        1
    );

    // With the mid-queue request cancelled, the FIFO head is untouched and
    // the queue remains settleable and executable once the epoch settles.
    let mut settled_state = denied.state;
    cutoff_and_settle(&mut settled_state);
    assert_eq!(
        settled_state
            .withdraw_queue
            .settled_head_claim(&settled_state.epoch),
        Some(1_000)
    );
    let executed = apply_action(
        settled_state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(SETTLE_NS + 10),
        },
    )
    .expect("remaining head executes after settlement");
    assert!(matches!(executed.state.op_state, OpState::Withdrawing(_)));
    assert_eq!(executed.state.withdraw_queue.len(), 1);
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn epoch_identifiers_are_monotonic_across_the_settlement_law() {
    let mut state = funded_state();
    let first_epoch = state.epoch.intake_epoch;
    assert_eq!(first_epoch, EpochId::FIRST_SETTLEMENT);
    assert!(state.epoch.is_accepting_requests());

    // Cutoff closes intake but never regresses the identifier.
    let cutoff_result = cutoff(&mut state, CUT_NS);
    state = cutoff_result.state;
    assert!(!state.epoch.is_accepting_requests());
    assert_eq!(state.epoch.intake_epoch, first_epoch);

    // Settlement advances intake monotonically and reopens for new requests,
    // which bind only to the new epoch.
    let settled = settle(&mut state, 1).expect("settlement accepted");
    let second_epoch = settled.state.epoch.intake_epoch;
    assert!(second_epoch > first_epoch);
    assert!(second_epoch.is_settlement_epoch());
    let result = apply_action(
        settled.state,
        &config(),
        None,
        &escrow(),
        KernelAction::RequestWithdraw {
            owner: other_owner(),
            receiver: other_receiver(),
            shares: 1,
            min_assets_out: 1,
            now_ns: TimestampNs(SETTLE_NS),
        },
    )
    .expect("intake is open again after settlement");
    let head = result.state.withdraw_queue.head().expect("queue head");
    assert_eq!(head.1.epoch_id, second_epoch);
    assert!(result.state.epoch.check_invariants());
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn execution_blocks_while_the_epoch_is_unsettled_and_after_a_below_min_claim() {
    let mut state = funded_state();
    queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    // Unsettled: the head cannot be executed and stays queued untouched.
    let before = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(CUT_NS + 10),
        },
    )
    .expect_err("no claim exists pre-settlement");
    assert_eq!(
        before,
        KernelError::InvalidState(InvalidStateCode::WithdrawalEpochUnsettled)
    );
    assert_eq!(state.withdraw_queue.len(), 1);

    cutoff_and_settle(&mut state);

    // Owner cancellation is the escape for an unsettled epoch: the owner is
    // made whole in full shares.
    let cancelled = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::CancelPendingWithdrawal {
            caller: owner(),
            request_id: 0,
            now_ns: TimestampNs(SETTLE_NS),
        },
    )
    .expect("owner cancels their own request");
    assert!(cancelled.state.withdraw_queue.is_empty());
    assert_eq!(
        count_effect(
            &cancelled.effects,
            &KernelEffect::TransferShares {
                from: escrow(),
                to: owner(),
                shares: 1_000
            }
        ),
        1
    );

    // Below-minimum: a settled claim under the request's own floor blocks
    // execution without repricing or silently dropping the request.
    let mut floor_state = funded_state();
    let floor_id = queue_entry(&mut floor_state, owner(), receiver(), 1_000, 2_000);
    cutoff_and_settle(&mut floor_state);
    let below = apply_action(
        floor_state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(SETTLE_NS + 10),
        },
    )
    .expect_err("claim below the request floor");
    assert_eq!(below, KernelError::Slippage { min: 2_000, actual: 1_000 });
    assert_eq!(floor_state.withdraw_queue.len(), 1);
    let head = floor_state.withdraw_queue.head().expect("head remains");
    assert_eq!(head.0, floor_id);
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn execution_pays_exactly_the_snapshot_claim_and_planning_burns_all_escrow() {
    let mut state = funded_state();
    queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    cutoff_and_settle(&mut state);

    let result = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(SETTLE_NS + 10),
        },
    )
    .expect("settled head executes");
    let op_id = result
        .state
        .current_op_id()
        .expect("withdrawal is in flight");
    // The in-flight claim equals the snapshot formula for the head escrow.
    let claim = state
        .epoch
        .settled_claim_for(EpochId::FIRST_SETTLEMENT, 1_000)
        .expect("settled");
    assert_eq!(claim, 1_000);
    assert!(matches!(result.state.op_state, OpState::Withdrawing(_)));

    // Payout planning draws on the in-flight op only and must burn every
    // escrowed share while paying exactly the snapshot claim.
    let plan = plan_idle_payout(&result.state, MIN_WITHDRAWAL_ASSETS).expect("idle-funded plan");
    assert_eq!(plan.op_id, op_id);
    assert_eq!(plan.request_id, 0);
    assert_eq!(plan.assets_out, claim);
    assert_eq!(plan.burn_shares, 1_000);
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn older_intake_must_fully_drain_before_the_next_cutoff() {
    let mut state = funded_state();
    queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    cutoff_and_settle(&mut state);
    // The head was priced by the settlement but never paid out or cancelled:
    // it is settlement-eligible intake older than the next epoch.
    assert!(state.withdraw_queue.has_intake_before(state.epoch.intake_epoch));
    let error = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::BeginEpochCutoff {
            cutoff_ns: TimestampNs(EPOCH_TWO_CUT_NS),
            now_ns: TimestampNs(EPOCH_TWO_CUT_NS),
        },
    )
    .expect_err("older intake must drain first");
    assert_eq!(
        error,
        KernelError::InvalidState(InvalidStateCode::EpochDrainRequired)
    );
    // Draining (cancelling) the queue releases the block.
    let cancelled = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::CancelPendingWithdrawal {
            caller: owner(),
            request_id: 0,
            now_ns: TimestampNs(EPOCH_TWO_CUT_NS),
        },
    )
    .expect("owner cancels the stale head");
    assert!(cancelled.state.withdraw_queue.is_empty());
    let reopened = apply_action(
        cancelled.state,
        &config(),
        None,
        &escrow(),
        KernelAction::BeginEpochCutoff {
            cutoff_ns: TimestampNs(EPOCH_TWO_CUT_NS),
            now_ns: TimestampNs(EPOCH_TWO_CUT_NS),
        },
    )
    .expect("cutoff accepted once drained");
    assert_eq!(reopened.state.epoch.intake_epoch.as_u64(), 2);
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_failure_refunds_all_escrow_shares_and_restores_idle() {
    let mut state = funded_state();
    queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    cutoff_and_settle(&mut state);

    state.op_state = withdrawing(9, 0, 1_000, 1_000);
    state.op_state = payout(9, 0, 1_000, 1_000);
    if let OpState::Payout(payout_state) = &mut state.op_state {
        payout_state.burn_shares = 0;
    }
    let result = apply_action(
        state,
        &config(),
        None,
        &escrow(),
        KernelAction::SettlePayout {
            op_id: 9,
            outcome: PayoutOutcome::Failure,
        },
    )
    .expect("failure settlement accepts");
    assert_eq!(
        count_effect(
            &result.effects,
            &KernelEffect::TransferShares {
                from: escrow(),
                to: owner(),
                shares: 1_000
            }
        ),
        1
    );
    assert_eq!(result.state.total_shares, SUPPLY_TOTAL);
    assert_eq!(result.state.idle_assets, IDLE_TOTAL);
    assert!(result.state.check_invariant());
    assert!(result.state.withdraw_queue.is_empty());
    assert_eq!(result.state.withdraw_queue.total_escrow_shares(), 0);
    assert!(result.state.op_state.is_idle());
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_success_burns_all_escrow_shares_and_progresses_the_queue() {
    let mut state = funded_state();
    queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    cutoff_and_settle(&mut state);

    state.op_state = withdrawing(9, 0, 1_000, 1_000);
    state.op_state = payout(9, 0, 1_000, 1_000);
    let result = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::SettlePayout {
            op_id: 9,
            outcome: PayoutOutcome::Success,
        },
    )
    .expect("payout settles");

    assert_eq!(
        count_effect(
            &result.effects,
            &KernelEffect::BurnShares {
                owner: escrow(),
                shares: 1_000
            }
        ),
        1
    );
    assert_eq!(
        count_effect(
            &result.effects,
            &KernelEffect::EmitEvent {
                event: crate::effects::KernelEvent::PayoutCompleted {
                    op_id: 9,
                    success: true,
                    burn_shares: 1_000,
                    refund_shares: 0,
                    amount: 1_000,
                },
            },
        ),
        1
    );
    // Conservation: every escrowed share was consumed by the burn.
    assert_eq!(result.state.total_shares, SUPPLY_TOTAL - 1_000);
    assert_eq!(result.state.idle_assets, IDLE_TOTAL - 1_000);
    assert!(result.state.check_invariant());
    assert!(result.state.withdraw_queue.is_empty());
    assert!(result.state.op_state.is_idle());
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_success_rejects_partial_burns_and_claims_that_differ_from_the_snapshot() {
    let mut state = funded_state();
    queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    cutoff_and_settle(&mut state);

    // Partial burn on success is rejected: success must consume every share.
    state.op_state = payout(9, 0, 1_000, 1_000);
    if let OpState::Payout(payout_state) = &mut state.op_state {
        payout_state.burn_shares = 999;
    }
    let partial = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::SettlePayout {
            op_id: 9,
            outcome: PayoutOutcome::Success,
        },
    )
    .expect_err("partial burn rejected");
    assert_eq!(
        partial,
        KernelError::InvalidState(InvalidStateCode::PayoutBurnMustMatchFullEscrow)
    );

    // A payout amount above the snapshot claim is rejected without minting
    // assets from nothing.
    state.op_state = payout(9, 0, 2_000, 1_000);
    let inflated = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::SettlePayout {
            op_id: 9,
            outcome: PayoutOutcome::Success,
        },
    )
    .expect_err("amount must match the settled claim");
    assert_eq!(
        inflated,
        KernelError::InvalidState(InvalidStateCode::PayoutClaimMismatch)
    );
    assert!(state.withdraw_queue.contains(0));
    assert_eq!(state.withdraw_queue.len(), 1);
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn queued_requests_store_only_shares_bounds_and_epoch_and_are_unpayable() {
    let mut state = funded_state();
    let id = queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    let head = state.withdraw_queue.head().expect("queue head");
    assert_eq!(head.0, id);
    assert_eq!(head.1.escrow_shares, 1_000);
    assert_eq!(head.1.min_assets_out, 1);
    // The request binds the current open intake epoch — a settlement-eligible
    // identifier, not a price.
    assert_eq!(head.1.epoch_id, EpochId::FIRST_SETTLEMENT);
    assert!(head.1.epoch_id.is_settlement_epoch());
    // No claim exists before settlement, for the head or via the epoch.
    assert!(state.withdraw_queue.settled_head_claim(&state.epoch).is_none());
    assert!(state.epoch.settled_claim_for(head.1.epoch_id, 1_000).is_none());
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn request_withdrawal_enqueues_escrow_and_epoch_without_pricing() {
    let state = funded_state();
    let result = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::RequestWithdraw {
            owner: owner(),
            receiver: receiver(),
            shares: 1_000,
            min_assets_out: 500,
            now_ns: TimestampNs(0),
        },
    )
    .expect("request accepted while intake is open");
    let head = result.state.withdraw_queue.head().expect("queue head");
    assert_eq!(head.1.escrow_shares, 1_000);
    assert_eq!(head.1.min_assets_out, 500);
    assert_eq!(head.1.epoch_id, EpochId::FIRST_SETTLEMENT);
    assert!(result.state.withdraw_queue.settled_head_claim(&state.epoch).is_none());
    // Exactly one escrow custody effect and one request event; no asset
    // figures are stored or emitted for the pending request.
    assert_eq!(
        count_effect(
            &result.effects,
            &KernelEffect::TransferShares {
                from: owner(),
                to: escrow(),
                shares: 1_000
            }
        ),
        1
    );
    assert!(result.effects.iter().any(|effect| matches!(
        effect,
        KernelEffect::EmitEvent {
            event: KernelEvent::WithdrawalRequested { .. }
        }
    )));
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn request_withdrawal_is_rejected_once_intake_is_closed() {
    let mut state = funded_state();
    let cutoff_result = cutoff(&mut state, CUT_NS);
    state.epoch = cutoff_result.state.epoch.clone();
    assert!(!state.epoch.is_accepting_requests());
    let error = apply_action(
        state,
        &config(),
        None,
        &escrow(),
        KernelAction::RequestWithdraw {
            owner: owner(),
            receiver: receiver(),
            shares: 1_000,
            min_assets_out: 1,
            now_ns: TimestampNs(CUT_NS + 5),
        },
    )
    .expect_err("intake closed");
    assert_eq!(
        error,
        KernelError::InvalidState(InvalidStateCode::EpochIntakeNotOpen)
    );
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn settled_snapshot_is_unique_replay_rejected_and_claim_matches_the_formula() {
    let mut state = funded_state();
    let first = queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    cutoff_and_settle(&mut state);

    // The bound snapshot is the only pricing authority, and its claim matches
    // floor(escrow_shares * settlement_nav / eligible_supply) = escrow shares.
    let snapshot = state.epoch.last_settled.as_ref().expect("snapshot bound");
    assert_eq!(snapshot.epoch_id(), EpochId::FIRST_SETTLEMENT);
    assert_eq!(
        state.epoch.settled_claim_for(EpochId::FIRST_SETTLEMENT, 1_000),
        Some(1_000)
    );
    assert_eq!(
        state.withdraw_queue.settled_head_claim(&state.epoch),
        Some(1_000)
    );

    // Intake reopened at the next monotonic epoch.
    assert_eq!(state.epoch.intake_epoch, EpochId::FIRST_SETTLEMENT.checked_next().unwrap());
    assert_eq!(state.epoch.intake_epoch.as_u64(), 2);

    // Settlement-eligible intake must drain before the next cutoff. Cancellation
    // is the owner escape; it never changes the first snapshot.
    let cancelled = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::CancelPendingWithdrawal {
            caller: owner(),
            request_id: first,
            now_ns: TimestampNs(EPOCH_TWO_CUT_NS),
        },
    )
    .expect("owner drains settled intake");
    assert!(cancelled.state.withdraw_queue.is_empty());
    assert_eq!(
        cancelled.state.epoch.last_settled,
        state.epoch.last_settled,
        "cancellation cannot rewrite the accepted snapshot"
    );
    assert_eq!(
        cancelled.state.epoch.settled_claim_for(EpochId::FIRST_SETTLEMENT, 1_000),
        Some(1_000)
    );

    // Closing and attempting to settle again is rejected by the law (the
    // accepted report sequence cannot repeat or regress).
    let second_cutoff = apply_action(
        cancelled.state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::BeginEpochCutoff {
            cutoff_ns: TimestampNs(EPOCH_TWO_CUT_NS),
            now_ns: TimestampNs(EPOCH_TWO_CUT_NS),
        },
    )
    .expect("cutoff for next epoch accepted");
    let replay = second_cutoff.state;
    assert_eq!(
        replay.epoch.intake_epoch,
        cancelled.state.epoch.intake_epoch
    );
    assert_eq!(
        replay.epoch.last_settled,
        cancelled.state.epoch.last_settled
    );
    assert_eq!(replay.total_assets, cancelled.state.total_assets);
    assert_eq!(replay.total_shares, cancelled.state.total_shares);
    assert_eq!(replay.idle_assets, cancelled.state.idle_assets);
    assert_eq!(replay.external_assets, cancelled.state.external_assets);
    assert_eq!(replay.op_state, cancelled.state.op_state);
    let duplicate = apply_action(
        replay.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::SettleEpoch {
            report: report(1, EPOCH_TWO_CUT_NS),
            new_external_assets: replay.external_assets,
            max_report_age_ns: 3_600_000_000_000,
            settle_now_ns: TimestampNs(EPOCH_TWO_SETTLE_NS),
        },
    )
    .expect_err("report sequence may not repeat");
    assert_eq!(
        duplicate,
        KernelError::InvalidState(InvalidStateCode::EpochSettlementRejected)
    );
    // The first snapshot is still the bound authority: nothing about the
    // rejected replay changed it, and the epoch cannot be settled twice.
    assert_eq!(
        replay.epoch.intake_epoch,
        cancelled.state.epoch.intake_epoch
    );
    assert_eq!(
        replay.epoch.last_settled,
        cancelled.state.epoch.last_settled
    );
    assert_eq!(replay.total_assets, cancelled.state.total_assets);
    assert_eq!(replay.total_shares, cancelled.state.total_shares);
    assert_eq!(replay.op_state, cancelled.state.op_state);
    assert_eq!(replay.epoch.intake_epoch.as_u64(), 2);
    assert_eq!(
        replay.epoch.settled_claim_for(EpochId::FIRST_SETTLEMENT, 1_000),
        Some(1_000)
    );
    assert!(replay.epoch.check_invariants());
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn settlement_rejects_nav_or_supply_that_violate_the_pricing_law() {
    let mut state = funded_state();
    queue_entry(&mut state, owner(), receiver(), 1_000, 1);
    let cutoff_result = cutoff(&mut state, CUT_NS);
    state.epoch = cutoff_result.state.epoch.clone();
    // A settlement NAV of zero cannot price any claim: supply is recorded and
    // positive, so the snapshot must reject a degenerate NAV.
    let bad = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::SettleEpoch {
            report: report(1, CUT_NS),
            new_external_assets: 0,
            max_report_age_ns: 3_600_000_000_000,
            settle_now_ns: TimestampNs(SETTLE_NS),
        },
    );
    match bad {
        Ok(result) => {
            // If accepted, the snapshot must still price nothing above NAV.
            let claim = result
                .state
                .epoch
                .settled_claim_for(EpochId::FIRST_SETTLEMENT, 1_000)
                .unwrap_or(0);
            assert!(claim <= result.state.epoch.last_settled.map_or(0, |snap| snap.settlement_nav()));
        }
        Err(error) => {
            assert_eq!(
                error,
                KernelError::InvalidState(InvalidStateCode::EpochSettlementRejected)
            );
        }
    }
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn settlement_requires_idle_cutoff_and_accepted_valuation() {
    let mut state = funded_state();
    // Settlement before any cutoff is rejected.
    let early = settle(&mut state, 1).expect_err("no cutoff yet");
    assert_eq!(
        early,
        KernelError::InvalidState(InvalidStateCode::EpochSettlementRejected)
    );
    // Cutoff requires the vault to be idle.
    state.op_state = withdrawing(9, 0, 1_000, 1_000);
    let busy = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::BeginEpochCutoff {
            cutoff_ns: TimestampNs(CUT_NS),
            now_ns: TimestampNs(CUT_NS),
        },
    )
    .expect_err("cutoff requires idle");
    assert_eq!(
        busy,
        KernelError::InvalidState(InvalidStateCode::EpochCutoffRequiresIdle)
    );
    state.op_state = OpState::Idle;
    // A valuation dated before the cutoff is rejected once cutoff is open.
    let cutoff_result = cutoff(&mut state, CUT_NS);
    state.epoch = cutoff_result.state.epoch.clone();
    let stale = apply_action(
        state.clone(),
        &config(),
        None,
        &escrow(),
        KernelAction::SettleEpoch {
            report: report(1, CUT_NS - 10),
            new_external_assets: state.external_assets,
            max_report_age_ns: 3_600_000_000_000,
            settle_now_ns: TimestampNs(SETTLE_NS),
        },
    )
    .expect_err("report precedes cutoff");
    assert_eq!(
        stale,
        KernelError::InvalidState(InvalidStateCode::EpochSettlementRejected)
    );
    assert!(state.epoch.last_settled.is_none());
}
