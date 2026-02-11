use super::*;
use crate::effects::KernelEvent;
use crate::fee::{FeeSlot, FeesSpec};
use crate::math::wad::Wad;
use crate::state::op_state::WithdrawingState;
use crate::state::queue::{DEFAULT_COOLDOWN_NS, MAX_PENDING};

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
            "max_pending_withdrawals exceeds MAX_PENDING"
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
            "execute_withdraw requires Idle (use withdrawal callbacks to advance)"
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
    use alloc::collections::BTreeSet;

    let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
    let config = test_config();
    let mut blacklist = BTreeSet::new();
    blacklist.insert(addr(9));
    let restrictions = Restrictions::Blacklist(blacklist);

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

// =========================================================================
// Deposit action tests
// =========================================================================

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
        Err(KernelError::InvalidState("deposit requires Idle"))
    ));
}

// =========================================================================
// RequestWithdraw action tests
// =========================================================================

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
        Err(KernelError::InvalidState("request_withdraw requires Idle"))
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

    assert!(matches!(result, Err(KernelError::QueueFull)));
}

// =========================================================================
// ExecuteWithdraw action tests
// =========================================================================

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

    assert!(matches!(result, Err(KernelError::EmptyQueue)));
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
        Err(KernelError::InvalidState("execute_withdraw requires Idle"))
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
            "execute_withdraw requires Idle (use withdrawal callbacks to advance)"
        ))
    ));
}

// =========================================================================
// BeginAllocating action tests
// =========================================================================

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

// =========================================================================
// FinishAllocating action tests
// =========================================================================

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
            "execute_withdraw requires Idle (use withdrawal callbacks to advance)"
        ))
    ));
}

// =========================================================================
// BeginRefreshing action tests
// =========================================================================

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

// =========================================================================
// FinishRefreshing action tests
// =========================================================================

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
            "sync_external_assets requires active op"
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
            "sync_external_assets requires Allocating/Withdrawing/Refreshing"
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
            "sync_external_assets would more than double total_assets"
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

// =========================================================================
// AbortRefreshing action tests
// =========================================================================

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
            "abort_refreshing requires active op"
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
            "abort_refreshing requires Refreshing"
        ))
    ));
}

// =========================================================================
// AbortAllocating action tests
// =========================================================================

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
            "abort_allocating requires Allocating"
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
            "abort_allocating restore_idle mismatch"
        ))
    ));
}

// =========================================================================
// AbortWithdrawing action tests
// =========================================================================

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
            "abort_withdrawing requires Withdrawing"
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
            "abort_withdrawing refund_shares mismatch"
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
        Err(KernelError::InvalidState("withdrawal queue head mismatch"))
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

    assert!(matches!(result, Err(KernelError::EmptyQueue)));
}

// =========================================================================
// SettlePayout action tests
// =========================================================================

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
        Err(KernelError::InvalidState("settle_payout requires Payout"))
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

    assert!(matches!(result, Err(KernelError::EmptyQueue)));
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
        Err(KernelError::InvalidState("withdrawal queue head mismatch"))
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
            "payout success settlement mismatch"
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
            "payout failure settlement mismatch"
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
            "payout failure restore_idle must equal payout.amount"
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
            "fee refresh timestamp cannot go backwards"
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
