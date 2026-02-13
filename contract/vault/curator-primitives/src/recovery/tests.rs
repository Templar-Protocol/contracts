use super::*;
use alloc::string::String;
use alloc::vec;
use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};

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
