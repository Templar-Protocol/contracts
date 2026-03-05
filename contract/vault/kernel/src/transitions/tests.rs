use super::*;
use crate::effects::{KernelEffect, KernelEvent};
use crate::test_utils::{owner_addr, receiver_addr};
use alloc::vec;

fn first_event(result: &TransitionResult) -> Option<&KernelEvent> {
    match result.effects.first() {
        Some(KernelEffect::EmitEvent { event }) => Some(event),
        _ => None,
    }
}

#[test]
fn complete_allocation_skips_zero_amount_pending_withdrawal() {
    let alloc = start_allocation(OpState::Idle, vec![(1, 100)], 10).unwrap();
    let alloc = allocation_step_callback(alloc.new_state, true, 100, 10).unwrap();

    let result = complete_allocation(
        alloc.new_state,
        10,
        Some(WithdrawalRequest {
            op_id: 11,
            amount: 0,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 25,
        }),
    )
    .unwrap();

    assert!(result.new_state.is_idle());
    assert_eq!(
        first_event(&result),
        Some(&KernelEvent::AllocationCompleted {
            op_id: 10,
            has_withdrawal: false,
        })
    );
}

#[test]
fn complete_allocation_rejects_nonzero_withdrawal_with_zero_escrow() {
    let alloc = start_allocation(OpState::Idle, vec![(1, 100)], 20).unwrap();
    let alloc = allocation_step_callback(alloc.new_state, true, 100, 20).unwrap();

    let err = complete_allocation(
        alloc.new_state,
        20,
        Some(WithdrawalRequest {
            op_id: 21,
            amount: 50,
            receiver: receiver_addr(2),
            owner: owner_addr(2),
            escrow_shares: 0,
        }),
    );

    assert!(matches!(err, Err(TransitionError::ZeroEscrowShares)));
}
