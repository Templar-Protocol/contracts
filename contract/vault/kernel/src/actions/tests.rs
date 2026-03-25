use super::*;
use crate::effects::{KernelEffect, KernelEvent, WithdrawalSkipReason};
use crate::error::{InvalidConfigCode, InvalidStateCode};
use crate::fee::{FeeSlot, FeesSpec};
use crate::math::wad::{compute_management_fee_shares, Wad, YEAR_NS};
use crate::state::op_state::{AllocatingState, WithdrawingState};
use crate::state::queue::{DEFAULT_COOLDOWN_NS, MAX_PENDING};
use crate::state::vault::{FeeAccrualAnchor, VaultConfig, VaultState};
use crate::Number;

fn addr(tag: u8) -> Address {
    [tag; 32]
}

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

#[test]
fn action_builder_and_metadata_helpers() {
    let action = KernelAction::finish_allocating(42, 1_000);
    assert!(matches!(action, KernelAction::FinishAllocating { .. }));
    assert_eq!(action.op_id(), Some(42));
    assert_eq!(action.timestamp_ns(), Some(1_000));

    let pause = KernelAction::pause(true);
    assert!(matches!(pause, KernelAction::Pause { .. }));
    assert_eq!(pause.op_id(), None);
    assert_eq!(pause.timestamp_ns(), None);

    let atomic = KernelAction::atomic_withdraw(addr(1), addr(2), addr(3), 11, 1_000);
    assert!(matches!(atomic, KernelAction::AtomicWithdraw { .. }));
    assert_eq!(atomic.op_id(), None);
    assert_eq!(atomic.timestamp_ns(), Some(1_000));

    let settle = KernelAction::settle_payout(
        7,
        PayoutOutcome::Failure {
            restore_idle: 10,
            refund_shares: 20,
        },
    );
    assert!(matches!(settle, KernelAction::SettlePayout { .. }));
    assert_eq!(settle.op_id(), Some(7));
    assert_eq!(settle.timestamp_ns(), None);
}

#[test]
fn invalid_max_pending_rejected() {
    let state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let mut config = test_config();
    config.max_pending_withdrawals = (MAX_PENDING as u32).saturating_add(1);

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::Pause { paused: false },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidConfig(
            InvalidConfigCode::MaxPendingWithdrawalsExceedsLimit
        ))
    ));
}

#[test]
fn request_withdraw_enqueues_and_emits_event() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
            now_ns: 0,
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

#[test]
fn execute_withdraw_idle_starts_withdrawal() {
    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();
    let owner = addr(3);
    let receiver = addr(4);

    let _ = state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: DEFAULT_COOLDOWN_NS + 1,
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

#[test]
fn execute_withdraw_withdrawing_advances_index() {
    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();
    let owner = addr(5);
    let receiver = addr(6);

    let _ = state
        .withdraw_queue
        .enqueue(owner, receiver, 200, 200, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 7,
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
        KernelAction::ExecuteWithdraw { now_ns: 0 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::ExecuteWithdrawRequiresIdleUseCallbacks
        ))
    ));
}

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
            now_ns: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::Restricted(RestrictionKind::Paused))
    ));
}

#[test]
fn request_withdraw_blocked_by_blacklist() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();
    let restrictions = Restrictions::Blacklist(alloc::vec![addr(9)]);

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
            now_ns: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::Restricted(RestrictionKind::Blacklisted))
    ));
}

#[test]
fn deposit_success() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
            now_ns: 0,
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

#[test]
fn deposit_emits_transfer_assets_from_owner() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
            now_ns: 0,
        },
    )
    .unwrap();

    let transfer = result.effects.iter().find_map(|effect| match effect {
        KernelEffect::TransferAssetsFrom { from, to, amount } => Some((*from, *to, *amount)),
        _ => None,
    });

    assert_eq!(transfer, Some((owner, self_id, 250)));
}

#[test]
fn deposit_zero_assets_fails_slippage() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
            now_ns: 0,
        },
    );

    assert!(matches!(result, Err(KernelError::ZeroAmount)));
}

#[test]
fn deposit_slippage_check_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
            min_shares_out: 1_000_000, // Way more than we can get
            now_ns: 0,
        },
    );

    assert!(matches!(result, Err(KernelError::Slippage { .. })));
}

#[test]
fn deposit_not_idle_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![(0, 500)],
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
            now_ns: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::DepositRequiresIdle
        ))
    ));
}

#[test]
fn atomic_withdraw_success_emits_burn_and_transfer() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
            amount: 100,
            kind: AtomicPayoutKind::Withdraw,
            now_ns: 0,
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

#[test]
fn atomic_redeem_delegated_operator_uses_burn_from_effect() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);
    let operator = addr(9);

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AtomicWithdraw {
            owner,
            receiver,
            operator,
            amount: 100,
            kind: AtomicPayoutKind::Redeem,
            now_ns: 0,
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
}

#[test]
fn atomic_withdraw_not_idle_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![(0, 500)],
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
            amount: 100,
            kind: AtomicPayoutKind::Withdraw,
            now_ns: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AtomicWithdrawRequiresIdle
        ))
    ));
}

#[test]
fn atomic_withdraw_exceeding_idle_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 250, 750, 0);
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
            amount: 300,
            kind: AtomicPayoutKind::Withdraw,
            now_ns: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AtomicWithdrawExceedsIdleAssets
        ))
    ));
}

#[test]
fn refresh_fees_then_atomic_withdraw_succeeds() {
    let mut state = VaultState::with_initial(1_500, 1_000, 1_500, 0, 0);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);
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
        KernelAction::RefreshFees { now_ns: 100 },
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
            amount: 500,
            kind: AtomicPayoutKind::Withdraw,
            now_ns: 100,
        },
    )
    .unwrap();

    assert_eq!(withdrawn.state.total_assets, 1_000);
    assert_eq!(withdrawn.state.idle_assets, 1_000);
    assert!(withdrawn.state.total_shares < refreshed_total_shares);
}

#[test]
fn request_withdraw_zero_shares_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
            now_ns: 0,
        },
    );

    assert!(matches!(result, Err(KernelError::ZeroAmount)));
}

#[test]
fn request_withdraw_slippage_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
            min_assets_out: 1_000_000, // Way more than we can get
            now_ns: 0,
        },
    );

    assert!(matches!(result, Err(KernelError::Slippage { .. })));
}

#[test]
fn request_withdraw_min_withdrawal_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
            now_ns: 0,
        },
    );

    assert!(matches!(result, Err(KernelError::MinWithdrawal { .. })));
}

#[test]
fn request_withdraw_not_idle_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![(0, 500)],
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
            now_ns: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::RequestWithdrawRequiresIdle
        ))
    ));
}

#[test]
fn request_withdraw_queue_full_fails() {
    let mut state = VaultState::with_initial(10_000, 10_000, 10_000, 0, 0);
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
            0,
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
            0,
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
            now_ns: 0,
        },
    );

    assert!(matches!(result, Err(KernelError::QueueFull { .. })));
}

#[test]
fn execute_withdraw_empty_queue_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw {
            now_ns: DEFAULT_COOLDOWN_NS + 1,
        },
    );

    assert!(matches!(result, Err(KernelError::NoPendingWithdrawals)));
}

#[test]
fn execute_withdraw_cooldown_fails() {
    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    state
        .withdraw_queue
        .enqueue(
            addr(1),
            addr(2),
            100,
            100,
            1_000_000,
            config.max_pending_withdrawals,
        )
        .unwrap();

    // Not enough time passed
    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw { now_ns: 1_000_000 },
    );

    assert!(matches!(result, Err(KernelError::Cooldown { .. })));
}

#[test]
fn execute_withdraw_wrong_state_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![(0, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::ExecuteWithdraw { now_ns: 0 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::ExecuteWithdrawRequiresIdle
        ))
    ));
}

#[test]
fn execute_withdraw_queue_head_mismatch_fails() {
    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
            0,
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 7,
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
        KernelAction::ExecuteWithdraw { now_ns: 0 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::ExecuteWithdrawRequiresIdleUseCallbacks
        ))
    ));
}

#[test]
fn begin_allocating_success() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::BeginAllocating {
            op_id: 1,
            plan: vec![(1, 500)],
            now_ns: 0,
        },
    )
    .unwrap();

    assert!(result.state.op_state.as_allocating().is_some());
    // idle_assets must be decremented by allocation total
    assert_eq!(result.state.idle_assets, 500);
    assert_eq!(result.state.total_assets, 500);
    assert!(result.state.check_invariant());
}

#[test]
fn begin_allocating_exceeds_idle() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::BeginAllocating {
            op_id: 1,
            plan: vec![(1, 1_500)], // exceeds idle_assets of 1_000
            now_ns: 0,
        },
    );

    assert!(matches!(result, Err(KernelError::InvalidState(_))));
}

#[test]
fn finish_allocating_success() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 1,
        remaining: 0,
        plan: vec![(1, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::FinishAllocating {
            op_id: 1,
            now_ns: 0,
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
}

#[test]
fn finish_allocating_with_pending_withdrawal() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let owner = addr(10);
    let receiver = addr(11);
    let config = test_config();

    // Add a pending withdrawal that's past cooldown
    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 5,
        index: 1,
        remaining: 0,
        plan: vec![(1, 500)],
    });

    // now_ns is past cooldown (DEFAULT_COOLDOWN_NS + request time of 0)
    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::FinishAllocating {
            op_id: 5,
            now_ns: DEFAULT_COOLDOWN_NS + 1,
        },
    )
    .unwrap();

    // Should transition to Withdrawing instead of Idle
    assert!(result.state.op_state.as_withdrawing().is_some());
}

#[test]
fn finish_allocating_with_pending_withdrawal_not_past_cooldown() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
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
            DEFAULT_COOLDOWN_NS,
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 6,
        index: 1,
        remaining: 0,
        plan: vec![(1, 500)],
    });

    // now_ns is not past cooldown
    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::FinishAllocating {
            op_id: 6,
            now_ns: DEFAULT_COOLDOWN_NS,
        },
    )
    .unwrap();

    // Should transition to Idle since withdrawal is not ready
    assert!(result.state.is_idle());
}

#[test]
fn execute_withdraw_withdrawing_empty_queue() {
    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();

    // State is Withdrawing but queue is empty (shouldn't happen in practice)
    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 8,
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
        KernelAction::ExecuteWithdraw { now_ns: 0 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::ExecuteWithdrawRequiresIdleUseCallbacks
        ))
    ));
}

#[test]
fn begin_refreshing_success() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::BeginRefreshing {
            op_id: 1,
            plan: vec![1],
            now_ns: 0,
        },
    )
    .unwrap();

    assert!(result.state.op_state.as_refreshing().is_some());
}

#[test]
fn finish_refreshing_success() {
    use crate::state::op_state::RefreshingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
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
            now_ns: 0,
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
}

#[test]
fn sync_external_assets_allocating() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 3,
        index: 0,
        remaining: 500,
        plan: vec![(1, 500)],
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
            now_ns: 0,
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

#[test]
fn sync_external_assets_withdrawing() {
    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 4,
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
            new_external_assets: 400,
            op_id: 4,
            now_ns: 0,
        },
    )
    .unwrap();

    assert_eq!(result.state.external_assets, 400);
    assert_eq!(result.state.total_assets, 900);
}

#[test]
fn sync_external_assets_refreshing() {
    use crate::state::op_state::RefreshingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
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
            new_external_assets: 600,
            op_id: 5,
            now_ns: 0,
        },
    )
    .unwrap();

    assert_eq!(result.state.external_assets, 600);
}

#[test]
fn sync_external_assets_idle_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 500,
            op_id: 1,
            now_ns: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::SyncExternalRequiresActiveOp
        ))
    ));
}

#[test]
fn sync_external_assets_op_id_mismatch_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 10,
        index: 0,
        remaining: 500,
        plan: vec![(1, 500)],
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
            now_ns: 0,
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

#[test]
fn sync_external_assets_payout_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    state.op_state = OpState::Payout(PayoutState {
        op_id: 6,
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
            now_ns: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::SyncExternalRequiresAllowedStates
        ))
    ));
}

#[test]
fn sync_external_assets_rejects_doubling() {
    use crate::state::op_state::AllocatingState;
    // total_assets = 1000; trying to set external to 2001 would make new total > 2x
    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![(0, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 2_001,
            op_id: 1,
            now_ns: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::SyncExternalWouldMoreThanDoubleTotalAssets
        ))
    ));
}

#[test]
fn sync_external_assets_allows_up_to_double() {
    use crate::state::op_state::AllocatingState;
    // total_assets = 1000; setting external to 1000 with idle=1000 => new total=2000 = 2x, OK
    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 500,
        plan: vec![(0, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SyncExternalAssets {
            new_external_assets: 1_000,
            op_id: 1,
            now_ns: 0,
        },
    );

    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.state.total_assets, 2_000);
}

#[test]
fn rebalance_withdraw_moves_assets_from_external_to_idle() {
    let state = VaultState::with_initial(1_000, 1_000, 200, 800, 0);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RebalanceWithdraw {
            op_id: 0,
            amount: 300,
            now_ns: 0,
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.idle_assets, 500);
    assert_eq!(result.state.external_assets, 500);
    assert_eq!(result.state.total_assets, 1_000);
}

#[test]
fn rebalance_withdraw_allows_allocating_with_matching_op_id() {
    let mut state = VaultState::with_initial(1_000, 1_000, 200, 800, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 7,
        index: 0,
        remaining: 100,
        plan: vec![(1, 100)],
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
            now_ns: 0,
        },
    )
    .unwrap();

    assert!(result.state.op_state.is_allocating());
    assert_eq!(result.state.idle_assets, 250);
    assert_eq!(result.state.external_assets, 750);
    assert_eq!(result.state.total_assets, 1_000);
}

#[test]
fn rebalance_withdraw_rejects_amount_above_external_assets() {
    let state = VaultState::with_initial(1_000, 1_000, 200, 800, 0);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RebalanceWithdraw {
            op_id: 0,
            amount: 801,
            now_ns: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::RebalanceWithdrawExceedsExternalAssets
        ))
    ));
}

#[test]
fn rebalance_withdraw_requires_matching_op_id_when_allocating() {
    let mut state = VaultState::with_initial(1_000, 1_000, 200, 800, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 7,
        index: 0,
        remaining: 100,
        plan: vec![(1, 100)],
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
            now_ns: 0,
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

#[test]
fn abort_refreshing_success() {
    use crate::state::op_state::RefreshingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
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

#[test]
fn abort_refreshing_wrong_state_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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

#[test]
fn abort_refreshing_op_id_mismatch_fails() {
    use crate::state::op_state::RefreshingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
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

#[test]
fn abort_refreshing_wrong_op_type_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 10,
        index: 0,
        remaining: 500,
        plan: vec![(1, 500)],
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

#[test]
fn abort_allocating_success() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(800, 1_000, 300, 500, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 8,
        index: 0,
        remaining: 200,
        plan: vec![(1, 200)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortAllocating {
            op_id: 8,
            restore_idle: 200,
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.idle_assets, 500); // 300 + 200 restored
    assert_eq!(result.state.total_assets, 1000); // 500 idle + 500 external
}

#[test]
fn abort_allocating_wrong_state_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortAllocating {
            op_id: 1,
            restore_idle: 0,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AbortAllocatingRequiresAllocating
        ))
    ));
}

#[test]
fn abort_allocating_op_id_mismatch_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 10,
        index: 0,
        remaining: 500,
        plan: vec![(1, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortAllocating {
            op_id: 99,
            restore_idle: 500,
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

#[test]
fn abort_allocating_restore_mismatch_fails() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 10,
        index: 0,
        remaining: 500,
        plan: vec![(1, 500)],
    });
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortAllocating {
            op_id: 10,
            restore_idle: 999, // Wrong amount
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AbortAllocatingRestoreIdleMismatch
        ))
    ));
}

#[test]
fn abort_withdrawing_success() {
    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 9,
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
        KernelAction::AbortWithdrawing {
            op_id: 9,
            refund_shares: 100,
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.withdraw_queue.len(), 0);
}

#[test]
fn abort_withdrawing_wrong_state_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::AbortWithdrawing {
            op_id: 1,
            refund_shares: 100,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AbortWithdrawingRequiresWithdrawing
        ))
    ));
}

#[test]
fn abort_withdrawing_op_id_mismatch_fails() {
    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 10,
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
        KernelAction::AbortWithdrawing {
            op_id: 99,
            refund_shares: 100,
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

#[test]
fn abort_withdrawing_refund_mismatch_fails() {
    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 10,
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
        KernelAction::AbortWithdrawing {
            op_id: 10,
            refund_shares: 999,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::AbortWithdrawingRefundMismatch
        ))
    ));
}

#[test]
fn abort_withdrawing_queue_head_mismatch_fails() {
    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();

    // Queue has different user
    state
        .withdraw_queue
        .enqueue(
            addr(99),
            addr(99),
            100,
            100,
            0,
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 10,
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
        KernelAction::AbortWithdrawing {
            op_id: 10,
            refund_shares: 100,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::WithdrawalQueueHeadMismatch
        ))
    ));
}

#[test]
fn abort_withdrawing_empty_queue_fails() {
    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 10,
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
        KernelAction::AbortWithdrawing {
            op_id: 10,
            refund_shares: 100,
        },
    );

    assert!(matches!(result, Err(KernelError::NoPendingWithdrawals)));
}

#[test]
fn settle_payout_success_burn_only() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 11,
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
            outcome: PayoutOutcome::Success {
                burn_shares: 100,
                refund_shares: 0,
            },
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
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

#[test]
fn settle_payout_success_partial_refund() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 12,
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
            outcome: PayoutOutcome::Success {
                burn_shares: 50,
                refund_shares: 50,
            },
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
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

#[test]
fn settle_payout_failure() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(900, 1_000, 400, 500, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 13,
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
            outcome: PayoutOutcome::Failure {
                restore_idle: 100,
                refund_shares: 100,
            },
        },
    )
    .unwrap();

    assert!(result.state.is_idle());
    assert_eq!(result.state.idle_assets, 500); // 400 + 100 restored
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

#[test]
fn settle_payout_wrong_state_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 1,
            outcome: PayoutOutcome::Success {
                burn_shares: 100,
                refund_shares: 0,
            },
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::SettlePayoutRequiresPayout
        ))
    ));
}

#[test]
fn settle_payout_op_id_mismatch_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 20,
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
            outcome: PayoutOutcome::Success {
                burn_shares: 100,
                refund_shares: 0,
            },
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

#[test]
fn settle_payout_empty_queue_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 20,
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
            outcome: PayoutOutcome::Success {
                burn_shares: 100,
                refund_shares: 0,
            },
        },
    );

    assert!(matches!(result, Err(KernelError::NoPendingWithdrawals)));
}

#[test]
fn settle_payout_queue_head_mismatch_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();

    state
        .withdraw_queue
        .enqueue(
            addr(99),
            addr(99),
            100,
            100,
            0,
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 20,
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
            outcome: PayoutOutcome::Success {
                burn_shares: 100,
                refund_shares: 0,
            },
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::WithdrawalQueueHeadMismatch
        ))
    ));
}

#[test]
fn settle_payout_success_settlement_mismatch_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 20,
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
            op_id: 20,
            outcome: PayoutOutcome::Success {
                burn_shares: 50,
                refund_shares: 10, // 50 + 10 != 100 escrow
            },
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::PayoutSuccessSettlementMismatch
        ))
    ));
}

#[test]
fn settle_payout_success_settlement_overflow_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1, u128::MAX, 1, 0, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            u128::MAX,
            1,
            0,
            config.max_pending_withdrawals,
        )
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 22,
        owner,
        receiver,
        amount: 1,
        escrow_shares: u128::MAX,
        burn_shares: u128::MAX,
    });

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 22,
            outcome: PayoutOutcome::Success {
                burn_shares: u128::MAX,
                refund_shares: u128::MAX,
            },
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::PayoutSuccessSettlementMismatch
        ))
    ));
}

#[test]
fn settle_payout_failure_settlement_mismatch_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 20,
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
            op_id: 20,
            outcome: PayoutOutcome::Failure {
                restore_idle: 100,
                refund_shares: 50, // Should be 100
            },
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::PayoutFailureSettlementMismatch
        ))
    ));
}

#[test]
fn settle_payout_failure_restore_idle_mismatch_fails() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let config = test_config();
    let owner = addr(1);
    let receiver = addr(2);

    state
        .withdraw_queue
        .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 21,
        owner,
        receiver,
        amount: 100,
        escrow_shares: 100,
        burn_shares: 100,
    });

    // restore_idle: 200 doesn't match payout.amount: 100
    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::SettlePayout {
            op_id: 21,
            outcome: PayoutOutcome::Failure {
                restore_idle: 200, // Should be 100
                refund_shares: 100,
            },
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::PayoutFailureRestoreIdleMismatch
        ))
    ));
}

// =========================================================================
// Pause action tests
// =========================================================================

#[test]
fn pause_action() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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

// =========================================================================
// RefreshFees action tests
// =========================================================================

#[test]
fn refresh_fees_action_zero_fees() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config(); // fees: FeesSpec::zero()

    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees { now_ns: 12345 },
    )
    .unwrap();

    assert_eq!(result.state.fee_anchor.total_assets, 1_000);
    assert_eq!(result.state.fee_anchor.timestamp_ns, 12345);
    assert_eq!(result.state.total_shares, 1_000); // No fee shares minted
    assert_eq!(result.effects.len(), 1); // Only FeesRefreshed event
    assert!(matches!(
        result.effects.first(),
        Some(KernelEffect::EmitEvent {
            event: KernelEvent::FeesRefreshed { now_ns: 12345, .. }
        })
    ));
}

#[test]
fn refresh_fees_mints_performance_fee_shares() {
    use crate::math::wad::YEAR_NS;
    // Setup: vault started with 1000 assets/shares, now has 1500 assets (profit)
    let mut state = VaultState::with_initial(1_500, 1_000, 1_500, 0, 0);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, 0); // anchor at 1000 assets, time 0

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
        KernelAction::RefreshFees { now_ns: YEAR_NS },
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

#[test]
fn refresh_fees_mints_management_fee_shares() {
    use crate::math::wad::YEAR_NS;
    // Setup: 1000 assets/shares, no profit, full year elapsed
    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);

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
        KernelAction::RefreshFees { now_ns: YEAR_NS },
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

#[test]
fn refresh_fees_mints_both_management_and_performance() {
    use crate::math::wad::compute_fee_shares_from_assets;
    use crate::math::wad::YEAR_NS;

    let mut state = VaultState::with_initial(1_500, 1_000, 1_500, 0, 0);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);

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
        KernelAction::RefreshFees { now_ns: YEAR_NS },
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

#[test]
fn refresh_fees_no_profit_skips_performance() {
    use crate::math::wad::YEAR_NS;
    // No profit (assets unchanged from anchor)
    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);

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
        KernelAction::RefreshFees { now_ns: YEAR_NS },
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

#[test]
fn refresh_fees_max_rate_caps_fee_accrual() {
    use crate::math::wad::YEAR_NS;
    // 1000 -> 2000 (100% profit), but max_rate = 20% per year
    let mut state = VaultState::with_initial(2_000, 1_000, 2_000, 0, 0);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);

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
        KernelAction::RefreshFees { now_ns: half_year },
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

#[test]
fn refresh_fees_rejects_backwards_time() {
    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    state.fee_anchor.timestamp_ns = 10000; // Current anchor at 10000
    let config = test_config();

    // Try to refresh with earlier timestamp
    let result = apply_action(
        state,
        &config,
        None,
        &addr(0xFF),
        KernelAction::RefreshFees { now_ns: 5000 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::FeeRefreshTimestampMustAdvance
        ))
    ));
}

#[test]
fn refresh_fees_requires_idle_state() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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
        KernelAction::RefreshFees { now_ns: 12_345 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::RefreshFeesRequiresIdle
        ))
    ));
}

// =========================================================================
// Helper function tests
// =========================================================================

#[test]
fn effective_totals_adds_virtual() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let mut config = test_config();
    config.virtual_shares = 100;
    config.virtual_assets = 200;

    let totals = effective_totals(&state, &config);
    assert_eq!(totals.supply, 1_000 + 100); // shares + max(virtual, 1)
    assert_eq!(totals.assets, 1_000 + 200); // assets + max(virtual, 1)
}

#[test]
fn convert_to_shares_works() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    // With 1:1 ratio (plus virtual adjustments)
    let shares = convert_to_shares(&state, &config, 500);
    // shares = 500 * (1001) / (1001) = 500
    assert_eq!(shares, 500);
}

#[test]
fn convert_to_assets_works() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();

    let assets = convert_to_assets(&state, &config, 500);
    assert_eq!(assets, 500);
}

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

// EmergencyReset action tests

#[test]
fn emergency_reset_from_idle_fails() {
    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
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

#[test]
fn emergency_reset_from_refreshing() {
    use crate::state::op_state::RefreshingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
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

#[test]
fn emergency_reset_from_allocating_restores_idle() {
    use crate::state::op_state::AllocatingState;

    let mut state = VaultState::with_initial(1_000, 1_000, 200, 800, 0);
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 10,
        index: 1,
        remaining: 300,
        plan: vec![(1, 500), (2, 500)],
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

#[test]
fn emergency_reset_from_withdrawing_refunds_shares() {
    let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
    let owner = addr(1);
    let receiver = addr(2);
    let _ = state
        .withdraw_queue
        .enqueue(owner, receiver, 200, 200, 0, 10)
        .unwrap();

    state.op_state = OpState::Withdrawing(WithdrawingState {
        op_id: 20,
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

#[test]
fn emergency_reset_from_payout_refunds_and_restores() {
    use crate::state::op_state::PayoutState;

    let mut state = VaultState::with_initial(1_000, 1_000, 400, 600, 0);
    let owner = addr(3);
    let receiver = addr(4);
    let _ = state
        .withdraw_queue
        .enqueue(owner, receiver, 300, 300, 0, 10)
        .unwrap();

    state.op_state = OpState::Payout(PayoutState {
        op_id: 30,
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

// =========================================================================
// Conversion ceil/floor tests (added post-refactor)
// =========================================================================

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

fn base_state(total_assets: u128, total_shares: u128) -> VaultState {
    let mut state = VaultState::new();
    state.total_assets = total_assets;
    state.total_shares = total_shares;
    state.idle_assets = total_assets;
    state
}

#[test]
fn convert_to_shares_ceil_matches_floor_on_exact_multiple() {
    let config = base_config();
    let state = base_state(100, 100);
    let assets = 40;
    let floor = convert_to_shares(&state, &config, assets);
    let ceil = convert_to_shares_ceil(&state, &config, assets);
    assert_eq!(floor, ceil);
}

#[test]
fn convert_to_shares_ceil_rounds_up_on_fractional() {
    let config = base_config();
    let state = base_state(3, 2);
    let assets = 1;
    let floor = convert_to_shares(&state, &config, assets);
    let ceil = convert_to_shares_ceil(&state, &config, assets);
    assert_eq!(ceil, floor.saturating_add(1));
}

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

#[test]
fn convert_to_assets_ceil_matches_floor_on_exact_multiple() {
    let config = base_config();
    let state = base_state(100, 100);
    let shares = 25;
    let floor = convert_to_assets(&state, &config, shares);
    let ceil = convert_to_assets_ceil(&state, &config, shares);
    assert_eq!(floor, ceil);
}

#[test]
fn convert_to_assets_ceil_rounds_up_on_fractional() {
    let config = base_config();
    let state = base_state(2, 3);
    let shares = 1;
    let floor = convert_to_assets(&state, &config, shares);
    let ceil = convert_to_assets_ceil(&state, &config, shares);
    assert_eq!(ceil, floor.saturating_add(1));
}

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

#[test]
fn deposit_overflow_total_assets_rejected() {
    let config = base_config();
    let mut state = base_state(u128::MAX - 5, 1);
    state.idle_assets = state.total_assets;
    let result = apply_action(
        state,
        &config,
        None,
        &[0u8; 32],
        KernelAction::Deposit {
            owner: [1u8; 32],
            receiver: [2u8; 32],
            assets_in: 10,
            min_shares_out: 0,
            now_ns: 0,
        },
    );
    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::DepositOverflowTotalAssets
        ))
    ));
}

#[test]
fn deposit_overflow_total_shares_rejected() {
    let config = base_config();
    let mut state = base_state(u128::MAX - 1, u128::MAX);
    state.idle_assets = state.total_assets;
    let result = apply_action(
        state,
        &config,
        None,
        &[0u8; 32],
        KernelAction::Deposit {
            owner: [1u8; 32],
            receiver: [2u8; 32],
            assets_in: 1,
            min_shares_out: 0,
            now_ns: 0,
        },
    );
    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::MintOverflowTotalShares
        ))
    ));
}

#[test]
fn refresh_fees_overflow_total_supply_rejected() {
    let mut config = base_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 2, [9u8; 32]),
        FeeSlot::new(Wad::zero(), [8u8; 32]),
        None,
    );
    let mut state = base_state(1_000, u128::MAX - 1);
    state.fee_anchor = FeeAccrualAnchor::new(0, 0);

    let result = apply_action(
        state,
        &config,
        None,
        &[0u8; 32],
        KernelAction::RefreshFees { now_ns: 1 },
    );
    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::FeeMintOverflowTotalSupply
        ))
    ));
}

#[test]
fn execute_withdraw_skips_zero_expected_assets() {
    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let owner = [3u8; 32];
    let receiver = [4u8; 32];
    let escrow_shares = 500;

    state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            escrow_shares,
            0,
            0,
            config.max_pending_withdrawals,
        )
        .expect("enqueue");

    let self_id = [9u8; 32];
    let result = apply_action(
        state,
        &config,
        None,
        &self_id,
        KernelAction::ExecuteWithdraw { now_ns: 0 },
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

#[test]
fn execute_withdraw_skips_restricted_head_and_processes_next() {
    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let restricted_owner = [3u8; 32];
    let first_receiver = [4u8; 32];
    let next_owner = [5u8; 32];
    let next_receiver = [6u8; 32];
    let self_id = [9u8; 32];

    state
        .withdraw_queue
        .enqueue(
            restricted_owner,
            first_receiver,
            500,
            100,
            0,
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
            0,
            config.max_pending_withdrawals,
        )
        .expect("enqueue second");

    let restrictions = Restrictions::Blacklist(vec![restricted_owner]);
    let result = apply_action(
        state,
        &config,
        Some(&restrictions),
        &self_id,
        KernelAction::ExecuteWithdraw {
            now_ns: DEFAULT_COOLDOWN_NS + 1,
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

#[test]
fn execute_withdraw_skips_zero_expected_head_then_waits_for_cooldown() {
    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let skipped_owner = [3u8; 32];
    let skipped_receiver = [4u8; 32];
    let waiting_owner = [5u8; 32];
    let waiting_receiver = [6u8; 32];
    let self_id = [9u8; 32];

    state
        .withdraw_queue
        .enqueue(
            skipped_owner,
            skipped_receiver,
            500,
            0,
            0,
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
            1,
            config.max_pending_withdrawals,
        )
        .expect("enqueue cooling-down head");

    let result = apply_action(
        state,
        &config,
        None,
        &self_id,
        KernelAction::ExecuteWithdraw { now_ns: 0 },
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

#[test]
fn finish_allocating_skips_restricted_head_and_chains_next() {
    use crate::state::op_state::AllocatingState;

    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let restricted_owner = [3u8; 32];
    let first_receiver = [4u8; 32];
    let next_owner = [5u8; 32];
    let next_receiver = [6u8; 32];
    let self_id = [9u8; 32];

    state
        .withdraw_queue
        .enqueue(
            restricted_owner,
            first_receiver,
            500,
            100,
            0,
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
            0,
            config.max_pending_withdrawals,
        )
        .expect("enqueue second");
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 77,
        index: 1,
        remaining: 0,
        plan: vec![(1, 500)],
    });

    let restrictions = Restrictions::Blacklist(vec![restricted_owner]);
    let result = apply_action(
        state,
        &config,
        Some(&restrictions),
        &self_id,
        KernelAction::FinishAllocating {
            op_id: 77,
            now_ns: DEFAULT_COOLDOWN_NS + 1,
        },
    )
    .expect("finish_allocating");

    let withdrawing = result.state.op_state.as_withdrawing().expect("withdrawing");
    assert_eq!(withdrawing.owner, next_owner);
    assert_eq!(withdrawing.receiver, next_receiver);
    assert!(result.effects.iter().any(|effect| {
        matches!(
            effect,
            KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalSkipped {
                    owner,
                    reason: WithdrawalSkipReason::Restricted,
                    ..
                },
            } if *owner == restricted_owner
        )
    }));
}

#[test]
fn finish_allocating_skips_restricted_head_then_waits_for_cooldown() {
    use crate::state::op_state::AllocatingState;

    let config = base_config();
    let mut state = base_state(1_000, 1_000);
    let restricted_owner = [3u8; 32];
    let first_receiver = [4u8; 32];
    let waiting_owner = [5u8; 32];
    let waiting_receiver = [6u8; 32];
    let self_id = [9u8; 32];

    state
        .withdraw_queue
        .enqueue(
            restricted_owner,
            first_receiver,
            500,
            100,
            0,
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
            1,
            config.max_pending_withdrawals,
        )
        .expect("enqueue cooling-down head");
    state.op_state = OpState::Allocating(AllocatingState {
        op_id: 77,
        index: 1,
        remaining: 0,
        plan: vec![(1, 500)],
    });

    let restrictions = Restrictions::Blacklist(vec![restricted_owner]);
    let result = apply_action(
        state,
        &config,
        Some(&restrictions),
        &self_id,
        KernelAction::FinishAllocating {
            op_id: 77,
            now_ns: 0,
        },
    )
    .expect("finish_allocating");

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
                    reason: WithdrawalSkipReason::Restricted,
                    ..
                },
            } if *owner == restricted_owner
        )
    }));
}

#[test]
fn execute_withdraw_respects_paused_restrictions() {
    let config = base_config();
    let mut state = base_state(1_000, 1_000);

    state
        .withdraw_queue
        .enqueue(
            [1u8; 32],
            [2u8; 32],
            100,
            100,
            0,
            config.max_pending_withdrawals,
        )
        .expect("enqueue");

    let result = apply_action(
        state,
        &config,
        Some(&Restrictions::Paused),
        &[9u8; 32],
        KernelAction::ExecuteWithdraw {
            now_ns: DEFAULT_COOLDOWN_NS + 1,
        },
    );

    assert!(matches!(
        result,
        Err(KernelError::Restricted(RestrictionKind::Paused))
    ));
}

fn minted_shares_for(effects: &[KernelEffect], owner: [u8; 32]) -> u128 {
    effects
        .iter()
        .filter_map(|effect| match effect {
            KernelEffect::MintShares { owner: who, shares } if *who == owner => Some(*shares),
            _ => None,
        })
        .sum()
}

#[test]
fn refresh_fees_respects_growth_rate_cap_with_both_fee_types() {
    let management_recipient = [9u8; 32];
    let performance_recipient = [8u8; 32];

    let mut config = base_config();
    config.fees = FeesSpec::new(
        FeeSlot::new(Wad::one() / 5, performance_recipient),
        FeeSlot::new(Wad::one() / 10, management_recipient),
        Some(Wad::one() / 10),
    );

    let mut state = base_state(2_000, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);

    let result = apply_action(
        state,
        &config,
        None,
        &[0u8; 32],
        KernelAction::RefreshFees { now_ns: YEAR_NS },
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

#[test]
fn refresh_fees_rejects_non_advancing_timestamp() {
    let mut config = base_config();
    config.fees = FeesSpec::zero();
    let mut state = base_state(1_000, 1_000);
    state.fee_anchor = FeeAccrualAnchor::new(1_000, 500);

    let result = apply_action(
        state,
        &config,
        None,
        &[0u8; 32],
        KernelAction::RefreshFees { now_ns: 500 },
    );

    assert!(matches!(
        result,
        Err(KernelError::InvalidState(
            InvalidStateCode::FeeRefreshTimestampMustAdvance
        ))
    ));
}
