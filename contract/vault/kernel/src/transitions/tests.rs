use super::*;
use crate::test_utils::{owner_addr, receiver_addr};

// -------------------------------------------------------------------------
// Allocation Tests
// -------------------------------------------------------------------------

#[test]
fn test_start_allocation_from_idle() {
    let state = OpState::Idle;
    let plan = vec![(0, 500), (1, 500)];
    let op_id = 1;

    let result = start_allocation(state, plan.clone(), op_id).unwrap();

    assert!(result.new_state.is_allocating());
    let alloc = result.new_state.as_allocating().unwrap();
    assert_eq!(alloc.op_id, op_id);
    assert_eq!(alloc.index, 0);
    assert_eq!(alloc.remaining, 1000);
    assert_eq!(alloc.plan, plan);
}

#[test]
fn test_start_allocation_not_idle_error() {
    let state = OpState::Refreshing(RefreshingState {
        op_id: 1,
        index: 0,
        plan: vec![0],
    });
    let plan = vec![(0, 500)];

    let result = start_allocation(state, plan, 2);

    assert!(matches!(result, Err(TransitionError::WrongState { .. })));
}

#[test]
fn test_start_allocation_empty_plan_error() {
    let state = OpState::Idle;
    let plan = vec![];

    let result = start_allocation(state, plan, 1);

    assert!(matches!(result, Err(TransitionError::EmptyAllocationPlan)));
}

#[test]
fn test_allocation_step_callback_success() {
    let state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 1000,
        plan: vec![(0, 500), (1, 500)],
    });

    let result = allocation_step_callback(state, true, 500, 1).unwrap();

    let alloc = result.new_state.as_allocating().unwrap();
    assert_eq!(alloc.index, 1);
    assert_eq!(alloc.remaining, 500);
}

#[test]
fn test_allocation_step_callback_invalid_index() {
    let state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 1,
        remaining: 500,
        plan: vec![(0, 500)],
    });

    let result = allocation_step_callback(state, true, 100, 1);

    assert!(matches!(result, Err(TransitionError::InvalidIndex { .. })));
}

#[test]
fn test_allocation_step_callback_failure_returns_idle() {
    let state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 1000,
        plan: vec![(0, 500)],
    });

    let result = allocation_step_callback(state, false, 0, 1).unwrap();

    assert!(result.new_state.is_idle());
}

#[test]
fn test_allocation_step_callback_wrong_op_id() {
    let state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 1000,
        plan: vec![(0, 500)],
    });

    let result = allocation_step_callback(state, true, 500, 999);

    assert!(matches!(
        result,
        Err(TransitionError::OpIdMismatch {
            expected: 1,
            actual: 999
        })
    ));
}

#[test]
fn test_allocation_step_callback_zero_amount_rejected() {
    let state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 1000,
        plan: vec![(0, 500)],
    });

    // Zero allocation on success should be rejected
    let result = allocation_step_callback(state, true, 0, 1);

    assert!(matches!(result, Err(TransitionError::ZeroAllocationAmount)));
}

#[test]
fn test_complete_allocation_to_idle() {
    let state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 2,
        remaining: 0,
        plan: vec![(0, 500), (1, 500)],
    });

    let result = complete_allocation(state, 1, None).unwrap();

    assert!(result.new_state.is_idle());
}

#[test]
fn test_complete_allocation_to_withdrawing() {
    let state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 2,
        remaining: 0,
        plan: vec![(0, 500)],
    });

    let request = WithdrawalRequest {
        op_id: 2,
        amount: 300,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 100,
    };

    let result = complete_allocation(state, 1, Some(request)).unwrap();

    assert!(result.new_state.is_withdrawing());
    let withdraw = result.new_state.as_withdrawing().unwrap();
    assert_eq!(withdraw.op_id, 2);
    assert_eq!(withdraw.remaining, 300);
    assert_eq!(withdraw.receiver, receiver_addr(1));
}

// -------------------------------------------------------------------------
// Withdrawal Tests
// -------------------------------------------------------------------------

#[test]
fn test_start_withdrawal_from_idle() {
    let state = OpState::Idle;
    let request = WithdrawalRequest {
        op_id: 1,
        amount: 1000,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 500,
    };

    let result = start_withdrawal(state, request).unwrap();

    assert!(result.new_state.is_withdrawing());
    let withdraw = result.new_state.as_withdrawing().unwrap();
    assert_eq!(withdraw.op_id, 1);
    assert_eq!(withdraw.remaining, 1000);
    assert_eq!(withdraw.collected, 0);
    assert_eq!(withdraw.escrow_shares, 500);
}

#[test]
fn test_start_withdrawal_zero_amount_error() {
    let state = OpState::Idle;
    let request = WithdrawalRequest {
        op_id: 1,
        amount: 0,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 100,
    };

    let result = start_withdrawal(state, request);

    assert!(matches!(result, Err(TransitionError::ZeroWithdrawalAmount)));
}

#[test]
fn test_start_withdrawal_zero_escrow_error() {
    let state = OpState::Idle;
    let request = WithdrawalRequest {
        op_id: 1,
        amount: 1000,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 0,
    };

    let result = start_withdrawal(state, request);

    assert!(matches!(result, Err(TransitionError::ZeroEscrowShares)));
}

#[test]
fn test_withdrawal_step_callback() {
    let state = OpState::Withdrawing(WithdrawingState {
        op_id: 1,
        index: 0,
        remaining: 1000,
        collected: 0,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 500,
    });

    let result = withdrawal_step_callback(state, 1, 400).unwrap();

    let withdraw = result.new_state.as_withdrawing().unwrap();
    assert_eq!(withdraw.index, 1);
    assert_eq!(withdraw.remaining, 600);
    assert_eq!(withdraw.collected, 400);
}

#[test]
fn test_withdrawal_collected_to_payout() {
    let state = OpState::Withdrawing(WithdrawingState {
        op_id: 1,
        index: 2,
        remaining: 0,
        collected: 1000,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 500,
    });

    let result = withdrawal_collected(state, 1, 400).unwrap();

    assert!(result.new_state.is_payout());
    let payout = result.new_state.as_payout().unwrap();
    assert_eq!(payout.amount, 1000);
    assert_eq!(payout.burn_shares, 400);
    assert_eq!(payout.escrow_shares, 500);
}

#[test]
fn test_withdrawal_collected_burn_exceeds_escrow_error() {
    let state = OpState::Withdrawing(WithdrawingState {
        op_id: 1,
        index: 0,
        remaining: 0,
        collected: 1000,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 500,
    });

    let result = withdrawal_collected(state, 1, 600);

    assert!(matches!(
        result,
        Err(TransitionError::BurnExceedsEscrow {
            burn: 600,
            escrow: 500
        })
    ));
}

#[test]
fn test_withdrawal_collected_incomplete_fails() {
    let state = OpState::Withdrawing(WithdrawingState {
        op_id: 1,
        index: 1,
        remaining: 200,
        collected: 800,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 500,
    });

    let result = withdrawal_collected(state, 1, 400);

    assert!(matches!(
        result,
        Err(TransitionError::WithdrawalIncomplete {
            remaining: 200,
            collected: 800
        })
    ));
}

#[test]
fn test_stop_withdrawal_refunds_shares() {
    let state = OpState::Withdrawing(WithdrawingState {
        op_id: 1,
        index: 1,
        remaining: 500,
        collected: 500,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 100,
    });

    let escrow_address = owner_addr(99);
    let result = stop_withdrawal(state, 1, escrow_address).unwrap();

    assert!(result.new_state.is_idle());
    // Should have a TransferShares effect for refund
    assert!(result
        .effects
        .iter()
        .any(|e| matches!(e, KernelEffect::TransferShares { shares: 100, .. })));
}

// -------------------------------------------------------------------------
// Refresh Tests
// -------------------------------------------------------------------------

#[test]
fn test_start_refresh_from_idle() {
    let state = OpState::Idle;
    let plan = vec![0, 1, 2];

    let result = start_refresh(state, plan.clone(), 1).unwrap();

    assert!(result.new_state.is_refreshing());
    let refresh = result.new_state.as_refreshing().unwrap();
    assert_eq!(refresh.op_id, 1);
    assert_eq!(refresh.index, 0);
    assert_eq!(refresh.plan, plan);
}

#[test]
fn test_start_refresh_empty_plan_error() {
    let state = OpState::Idle;

    let result = start_refresh(state, vec![], 1);

    assert!(matches!(result, Err(TransitionError::EmptyRefreshPlan)));
}

#[test]
fn test_refresh_step_callback() {
    let state = OpState::Refreshing(RefreshingState {
        op_id: 1,
        index: 0,
        plan: vec![0, 1],
    });

    let result = refresh_step_callback(state, 1).unwrap();

    let refresh = result.new_state.as_refreshing().unwrap();
    assert_eq!(refresh.index, 1);
}

#[test]
fn test_refresh_step_callback_invalid_index() {
    let state = OpState::Refreshing(RefreshingState {
        op_id: 1,
        index: 1,
        plan: vec![0],
    });

    let result = refresh_step_callback(state, 1);

    assert!(matches!(result, Err(TransitionError::InvalidIndex { .. })));
}

#[test]
fn test_complete_refresh_to_idle() {
    let state = OpState::Refreshing(RefreshingState {
        op_id: 1,
        index: 2,
        plan: vec![0, 1],
    });

    let result = complete_refresh(state, 1).unwrap();

    assert!(result.new_state.is_idle());
}

// -------------------------------------------------------------------------
// Payout Tests
// -------------------------------------------------------------------------

#[test]
fn test_payout_complete_success() {
    let escrow_address = owner_addr(99);
    let state = OpState::Payout(PayoutState {
        op_id: 1,
        receiver: receiver_addr(1),
        amount: 1000,
        owner: owner_addr(1),
        escrow_shares: 500,
        burn_shares: 400,
    });

    let result = payout_complete(state, true, 1, escrow_address).unwrap();

    assert!(result.new_state.is_idle());

    // Should have BurnShares effect
    let (burn_owner, burn_shares) = result
        .effects
        .iter()
        .find_map(|e| match e {
            KernelEffect::BurnShares { owner, shares } => Some((*owner, *shares)),
            _ => None,
        })
        .expect("missing BurnShares effect");
    assert_eq!(burn_owner, escrow_address);
    assert_eq!(burn_shares, 400);

    // Should have TransferShares effect for refund (500 - 400 = 100)
    assert!(result
        .effects
        .iter()
        .any(|e| matches!(e, KernelEffect::TransferShares { shares: 100, .. })));
}

#[test]
fn test_payout_complete_failure_refunds_all() {
    let escrow_address = owner_addr(99);
    let state = OpState::Payout(PayoutState {
        op_id: 1,
        receiver: receiver_addr(1),
        amount: 1000,
        owner: owner_addr(1),
        escrow_shares: 500,
        burn_shares: 400,
    });

    let result = payout_complete(state, false, 1, escrow_address).unwrap();

    assert!(result.new_state.is_idle());

    // Should NOT have BurnShares effect
    assert!(!result
        .effects
        .iter()
        .any(|e| matches!(e, KernelEffect::BurnShares { .. })));

    // Should have TransferShares effect for full refund (500 shares)
    assert!(result
        .effects
        .iter()
        .any(|e| matches!(e, KernelEffect::TransferShares { shares: 500, .. })));
}

#[test]
fn test_payout_complete_wrong_state_error() {
    let state = OpState::Idle;

    let escrow_address = owner_addr(99);
    let result = payout_complete(state, true, 1, escrow_address);

    assert!(matches!(result, Err(TransitionError::WrongState { .. })));
}

#[test]
fn test_payout_complete_wrong_op_id_error() {
    let escrow_address = owner_addr(99);
    let state = OpState::Payout(PayoutState {
        op_id: 1,
        receiver: receiver_addr(1),
        amount: 1000,
        owner: owner_addr(1),
        escrow_shares: 500,
        burn_shares: 400,
    });

    let result = payout_complete(state, true, 999, escrow_address);

    assert!(matches!(
        result,
        Err(TransitionError::OpIdMismatch {
            expected: 1,
            actual: 999
        })
    ));
}

// -------------------------------------------------------------------------
// State Machine Flow Tests
// -------------------------------------------------------------------------

#[test]
fn test_full_allocation_flow() {
    // Start from Idle
    let state = OpState::Idle;

    // Start allocation
    let result = start_allocation(state, vec![(0, 500), (1, 500)], 1).unwrap();
    assert!(result.new_state.is_allocating());

    // First step callback
    let result = allocation_step_callback(result.new_state, true, 500, 1).unwrap();
    assert!(result.new_state.is_allocating());

    // Second step callback
    let result = allocation_step_callback(result.new_state, true, 500, 1).unwrap();
    assert!(result.new_state.is_allocating());

    // Complete allocation, no pending withdrawal
    let result = complete_allocation(result.new_state, 1, None).unwrap();
    assert!(result.new_state.is_idle());
}

#[test]
fn test_full_withdrawal_flow() {
    // Start from Idle
    let state = OpState::Idle;

    // Start withdrawal
    let request = WithdrawalRequest {
        op_id: 1,
        amount: 1000,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 500,
    };
    let result = start_withdrawal(state, request).unwrap();
    assert!(result.new_state.is_withdrawing());

    // Collect funds
    let result = withdrawal_step_callback(result.new_state, 1, 500).unwrap();
    assert!(result.new_state.is_withdrawing());

    let result = withdrawal_step_callback(result.new_state, 1, 500).unwrap();
    assert!(result.new_state.is_withdrawing());

    // Transition to payout
    let result = withdrawal_collected(result.new_state, 1, 400).unwrap();
    assert!(result.new_state.is_payout());

    // Complete payout
    let escrow_address = owner_addr(99);
    let result = payout_complete(result.new_state, true, 1, escrow_address).unwrap();
    assert!(result.new_state.is_idle());
}

#[test]
fn test_allocation_to_withdrawal_flow() {
    // Start allocation
    let state = OpState::Idle;
    let result = start_allocation(state, vec![(0, 1000)], 1).unwrap();

    // Complete step
    let result = allocation_step_callback(result.new_state, true, 1000, 1).unwrap();

    // Complete allocation with pending withdrawal
    let request = WithdrawalRequest {
        op_id: 2,
        amount: 500,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 250,
    };
    let result = complete_allocation(result.new_state, 1, Some(request)).unwrap();
    assert!(result.new_state.is_withdrawing());

    let withdraw = result.new_state.as_withdrawing().unwrap();
    assert_eq!(withdraw.op_id, 2);
}

use alloc::vec;
use alloc::vec::Vec;
use proptest::prelude::*;

fn arb_plan(max_len: usize) -> impl Strategy<Value = Vec<(TargetId, u128)>> {
    proptest::collection::vec((0u32..100u32, 1u128..=1_000_000_000u128), 1..=max_len)
}

fn arb_withdrawal_request() -> impl Strategy<Value = WithdrawalRequest> {
    (
        1u64..u64::MAX,
        1u128..=1_000_000_000u128,
        1u128..=1_000_000_000u128,
    )
        .prop_map(|(op_id, amount, escrow_shares)| WithdrawalRequest {
            op_id,
            amount,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares,
        })
}

proptest! {
    #[test]
    fn prop_start_allocation_from_idle_succeeds(
        plan in arb_plan(10),
        op_id in 1u64..=u64::MAX,
    ) {
        let result = start_allocation(OpState::Idle, plan.clone(), op_id);
        prop_assert!(result.is_ok());

        let result = result.unwrap();
        prop_assert!(result.new_state.is_allocating());

        let alloc = result.new_state.as_allocating().unwrap();
        prop_assert_eq!(alloc.op_id, op_id);
        prop_assert_eq!(alloc.index, 0);

        let expected_remaining: u128 = plan.iter().map(|(_, amt)| amt).sum();
        prop_assert_eq!(alloc.remaining, expected_remaining);
    }

    #[test]
    fn prop_cannot_double_start_allocation(
        plan1 in arb_plan(5),
        plan2 in arb_plan(5),
        op_id1 in 1u64..=u64::MAX / 2,
        op_id2 in u64::MAX / 2 + 1..=u64::MAX,
    ) {
        let result1 = start_allocation(OpState::Idle, plan1, op_id1).unwrap();
        prop_assert!(result1.new_state.is_allocating());

        let result2 = start_allocation(result1.new_state, plan2, op_id2);
        prop_assert!(result2.is_err());
        let is_not_idle = matches!(result2, Err(TransitionError::WrongState { .. }));
        prop_assert!(is_not_idle, "expected WrongState error");
    }

    #[test]
    fn prop_start_withdrawal_from_idle_succeeds(
        request in arb_withdrawal_request(),
    ) {
        let result = start_withdrawal(OpState::Idle, request.clone());
        prop_assert!(result.is_ok());

        let result = result.unwrap();
        prop_assert!(result.new_state.is_withdrawing());

        let withdraw = result.new_state.as_withdrawing().unwrap();
        prop_assert_eq!(withdraw.op_id, request.op_id);
        prop_assert_eq!(withdraw.remaining, request.amount);
        prop_assert_eq!(withdraw.collected, 0);
        prop_assert_eq!(withdraw.escrow_shares, request.escrow_shares);
    }

    #[test]
    fn prop_cannot_double_start_withdrawal(
        request1 in arb_withdrawal_request(),
        request2 in arb_withdrawal_request(),
    ) {
        let result1 = start_withdrawal(OpState::Idle, request1).unwrap();
        prop_assert!(result1.new_state.is_withdrawing());

        let result2 = start_withdrawal(result1.new_state, request2);
        prop_assert!(result2.is_err());
        let is_not_idle = matches!(result2, Err(TransitionError::WrongState { .. }));
        prop_assert!(is_not_idle, "expected WrongState error");
    }

    #[test]
    fn prop_start_refresh_from_idle_succeeds(
        targets in proptest::collection::vec(0u32..100u32, 1..10),
        op_id in 1u64..=u64::MAX,
    ) {
        let result = start_refresh(OpState::Idle, targets.clone(), op_id);
        prop_assert!(result.is_ok());

        let result = result.unwrap();
        prop_assert!(result.new_state.is_refreshing());

        let refresh = result.new_state.as_refreshing().unwrap();
        prop_assert_eq!(refresh.op_id, op_id);
        prop_assert_eq!(refresh.index, 0);
        prop_assert_eq!(refresh.plan.clone(), targets);
    }

    #[test]
    fn prop_allocation_step_advances_correctly(
        plan in arb_plan(5),
        op_id in 1u64..=u64::MAX,
        allocated in 1u128..=1_000_000u128,
    ) {
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        let alloc = result.new_state.as_allocating().unwrap();
        let initial_remaining = alloc.remaining;
        prop_assume!(allocated <= initial_remaining);

        let step_result = allocation_step_callback(result.new_state, true, allocated, op_id);
        prop_assert!(step_result.is_ok());

        let step_result = step_result.unwrap();
        let new_alloc = step_result.new_state.as_allocating().unwrap();

        prop_assert_eq!(new_alloc.index, 1);
        prop_assert_eq!(new_alloc.remaining, initial_remaining.saturating_sub(allocated));
    }

    #[test]
    fn prop_allocation_failure_returns_to_idle(
        plan in arb_plan(5),
        op_id in 1u64..=u64::MAX,
    ) {
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        let step_result = allocation_step_callback(result.new_state, false, 0, op_id);

        prop_assert!(step_result.is_ok());
        prop_assert!(step_result.unwrap().new_state.is_idle());
    }

    #[test]
    fn prop_op_id_mismatch_rejected(
        plan in arb_plan(3),
        op_id in 1u64..=u64::MAX / 2,
        wrong_op_id in u64::MAX / 2 + 1..=u64::MAX,
    ) {
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        let step_result = allocation_step_callback(result.new_state, true, 100, wrong_op_id);

        prop_assert!(step_result.is_err());
        let is_op_id_mismatch = matches!(step_result, Err(TransitionError::OpIdMismatch { .. }));
        prop_assert!(is_op_id_mismatch, "expected OpIdMismatch error");
    }

    #[test]
    fn prop_complete_allocation_to_idle(
        plan in arb_plan(3),
        op_id in 1u64..=u64::MAX,
    ) {
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        let complete = complete_allocation(result.new_state, op_id, None);

        prop_assert!(complete.is_ok());
        prop_assert!(complete.unwrap().new_state.is_idle());
    }

    #[test]
    fn prop_complete_allocation_to_withdrawing(
        plan in arb_plan(3),
        op_id in 1u64..=u64::MAX / 2,
        pending in arb_withdrawal_request(),
    ) {
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        let complete = complete_allocation(result.new_state, op_id, Some(pending.clone()));

        prop_assert!(complete.is_ok());
        let new_state = complete.unwrap().new_state;
        prop_assert!(new_state.is_withdrawing());

        let withdraw = new_state.as_withdrawing().unwrap();
        prop_assert_eq!(withdraw.op_id, pending.op_id);
    }

    #[test]
    fn prop_withdrawal_step_accumulates_collected(
        request in arb_withdrawal_request(),
        collected1 in 1u128..=1_000_000u128,
        collected2 in 1u128..=1_000_000u128,
    ) {
        let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();

        prop_assume!(request.amount > 0);
        let remaining1 = request.amount;
        let mut bounded1 = collected1 % remaining1;
        if bounded1 == 0 {
            bounded1 = 1;
        }

        let step1 =
            withdrawal_step_callback(result.new_state, request.op_id, bounded1).unwrap();
        let w1 = step1.new_state.as_withdrawing().unwrap();
        prop_assert_eq!(w1.collected, bounded1);
        prop_assert_eq!(w1.index, 1);

        let remaining2 = remaining1.saturating_sub(bounded1);
        prop_assume!(remaining2 > 0);
        let mut bounded2 = collected2 % remaining2;
        if bounded2 == 0 {
            bounded2 = 1;
        }

        let step2 =
            withdrawal_step_callback(step1.new_state, request.op_id, bounded2).unwrap();
        let w2 = step2.new_state.as_withdrawing().unwrap();
        prop_assert_eq!(w2.collected, bounded1.saturating_add(bounded2));
        prop_assert_eq!(w2.index, 2);
    }

    #[test]
    fn prop_withdrawal_collected_validates_burn(
        request in arb_withdrawal_request(),
        excess in 1u128..=1_000_000u128,
    ) {
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: request.op_id,
            index: 1,
            remaining: 0,
            collected: request.amount,
            receiver: request.receiver,
            owner: request.owner,
            escrow_shares: request.escrow_shares,
        });
        let burn_shares = request.escrow_shares.saturating_add(excess);

        let collected = withdrawal_collected(state, request.op_id, burn_shares);
        prop_assert!(collected.is_err());
        let is_burn_exceeds = matches!(collected, Err(TransitionError::BurnExceedsEscrow { .. }));
        prop_assert!(is_burn_exceeds, "expected BurnExceedsEscrow error");
    }

    #[test]
    fn prop_stop_withdrawal_returns_to_idle(
        request in arb_withdrawal_request(),
    ) {
        let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();
        let escrow_address = owner_addr(99);
        let stop = stop_withdrawal(result.new_state, request.op_id, escrow_address);

        prop_assert!(stop.is_ok());
        prop_assert!(stop.unwrap().new_state.is_idle());
    }

    #[test]
    fn prop_complete_refresh_returns_to_idle(
        targets in proptest::collection::vec(0u32..100u32, 1..10),
        op_id in 1u64..=u64::MAX,
    ) {
        let result = start_refresh(OpState::Idle, targets, op_id).unwrap();
        let complete = complete_refresh(result.new_state, op_id);

        prop_assert!(complete.is_ok());
        prop_assert!(complete.unwrap().new_state.is_idle());
    }

    #[test]
    fn prop_payout_complete_returns_to_idle(
        op_id in 1u64..=u64::MAX,
        amount in 1u128..=1_000_000_000u128,
        escrow_shares in 1u128..=1_000_000_000u128,
        burn_pct in 0u8..=100u8,
        success in proptest::bool::ANY,
    ) {
        let burn_shares = (escrow_shares as u128 * burn_pct as u128) / 100;
        let payout = PayoutState {
            op_id,
            receiver: receiver_addr(1),
            amount,
            owner: owner_addr(1),
            escrow_shares,
            burn_shares,
        };
        let state = OpState::Payout(payout);

        let escrow_address = owner_addr(99);
        let result = payout_complete(state, success, op_id, escrow_address);
        prop_assert!(result.is_ok());
        prop_assert!(result.unwrap().new_state.is_idle());
    }

    #[test]
    fn prop_zero_withdrawal_amount_rejected(
        op_id in 1u64..=u64::MAX,
        escrow_shares in 1u128..=1_000_000u128,
    ) {
        let request = WithdrawalRequest {
            op_id,
            amount: 0,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares,
        };
        let result = start_withdrawal(OpState::Idle, request);

        prop_assert!(result.is_err());
        prop_assert!(matches!(result, Err(TransitionError::ZeroWithdrawalAmount)));
    }

    #[test]
    fn prop_zero_escrow_shares_rejected(
        op_id in 1u64..=u64::MAX,
        amount in 1u128..=1_000_000u128,
    ) {
        let request = WithdrawalRequest {
            op_id,
            amount,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 0,
        };
        let result = start_withdrawal(OpState::Idle, request);

        prop_assert!(result.is_err());
        prop_assert!(matches!(result, Err(TransitionError::ZeroEscrowShares)));
    }

    #[test]
    fn prop_empty_allocation_plan_rejected(
        op_id in 1u64..=u64::MAX,
    ) {
        let result = start_allocation(OpState::Idle, vec![], op_id);

        prop_assert!(result.is_err());
        prop_assert!(matches!(result, Err(TransitionError::EmptyAllocationPlan)));
    }

    #[test]
    fn prop_empty_refresh_plan_rejected(
        op_id in 1u64..=u64::MAX,
    ) {
        let result = start_refresh(OpState::Idle, vec![], op_id);

        prop_assert!(result.is_err());
        prop_assert!(matches!(result, Err(TransitionError::EmptyRefreshPlan)));
    }
}

fn extract_event(effects: &[KernelEffect]) -> Option<&KernelEvent> {
    effects.iter().find_map(|effect| {
        if let KernelEffect::EmitEvent { event } = effect {
            Some(event)
        } else {
            None
        }
    })
}

#[test]
fn start_allocation_emits_event() {
    let plan = vec![(0, 100), (1, 200)];
    let result = start_allocation(OpState::Idle, plan.clone(), 7).unwrap();
    let event = extract_event(&result.effects).expect("event");

    assert!(matches!(
        event,
        KernelEvent::AllocationStarted {
            op_id: 7,
            total: 300,
            plan_len: 2
        }
    ));
}

#[test]
fn complete_allocation_emits_event() {
    let state = OpState::Allocating(AllocatingState {
        op_id: 9,
        index: 0,
        remaining: 0,
        plan: vec![(0, 1)],
    });

    let result = complete_allocation(state, 9, None).unwrap();
    let event = extract_event(&result.effects).expect("event");
    assert!(matches!(
        event,
        KernelEvent::AllocationCompleted {
            op_id: 9,
            has_withdrawal: false
        }
    ));
}

#[test]
fn withdrawal_events_emitted() {
    let request = WithdrawalRequest {
        op_id: 3,
        amount: 500,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 250,
    };
    let result = start_withdrawal(OpState::Idle, request).unwrap();
    let event = extract_event(&result.effects).expect("event");
    assert!(matches!(
        event,
        KernelEvent::WithdrawalStarted {
            op_id: 3,
            amount: 500,
            escrow_shares: 250,
            ..
        }
    ));

    let state = OpState::Withdrawing(WithdrawingState {
        op_id: 3,
        index: 0,
        remaining: 0,
        collected: 500,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 250,
    });
    let result = withdrawal_collected(state, 3, 200).unwrap();
    let event = extract_event(&result.effects).expect("event");
    assert!(matches!(
        event,
        KernelEvent::WithdrawalCollected {
            op_id: 3,
            burn_shares: 200,
            collected: 500
        }
    ));
}

#[test]
fn refresh_and_payout_events_emitted() {
    let result = start_refresh(OpState::Idle, vec![0, 1], 11).unwrap();
    let event = extract_event(&result.effects).expect("event");
    assert!(matches!(
        event,
        KernelEvent::RefreshStarted {
            op_id: 11,
            plan_len: 2
        }
    ));

    let state = OpState::Refreshing(RefreshingState {
        op_id: 11,
        index: 2,
        plan: vec![0, 1],
    });
    let result = complete_refresh(state, 11).unwrap();
    let event = extract_event(&result.effects).expect("event");
    assert!(matches!(event, KernelEvent::RefreshCompleted { op_id: 11 }));

    let state = OpState::Payout(PayoutState {
        op_id: 22,
        receiver: receiver_addr(2),
        amount: 100,
        owner: owner_addr(2),
        escrow_shares: 50,
        burn_shares: 50,
    });
    let escrow_address = owner_addr(99);
    let result = payout_complete(state, true, 22, escrow_address).unwrap();
    let event = extract_event(&result.effects).expect("event");
    assert!(matches!(
        event,
        KernelEvent::PayoutCompleted {
            op_id: 22,
            success: true,
            burn_shares: 50,
            refund_shares: 0,
            amount: 100
        }
    ));
}
