use super::*;
use crate::test_utils::{owner_addr, receiver_addr};
use alloc::vec;

#[test]
fn test_idle_state_default() {
    let state = OpState::default();
    assert!(state.is_idle());
    assert!(state.as_idle().is_some());
    assert_eq!(state.op_id(), None);
}

#[test]
fn test_allocating_state() {
    let alloc = AllocatingState {
        op_id: 42,
        index: 0,
        remaining: 1000,
        plan: vec![
            AllocationPlanEntry::new(1, 500),
            AllocationPlanEntry::new(2, 500),
        ],
    };
    let state: OpState = alloc.clone().into();

    assert!(state.is_allocating());
    assert!(!state.is_idle());
    assert_eq!(state.op_id(), Some(42));

    let inner = state.as_allocating().unwrap();
    assert_eq!(inner.remaining, 1000);
    assert_eq!(inner.plan.len(), 2);
}

#[test]
fn test_withdrawing_state() {
    let withdraw = WithdrawingState {
        op_id: 100,
        request_id: 101,
        index: 1,
        remaining: 500,
        collected: 200,
        receiver: receiver_addr(1),
        owner: owner_addr(1),
        escrow_shares: 1000,
    };
    let state: OpState = withdraw.into();

    assert!(state.is_withdrawing());
    assert_eq!(state.op_id(), Some(100));

    let inner = state.as_withdrawing().unwrap();
    assert_eq!(inner.request_id, 101);
    assert_eq!(inner.receiver, receiver_addr(1));
    assert_eq!(inner.owner, owner_addr(1));
}

#[test]
fn test_refreshing_state() {
    let refresh = RefreshingState {
        op_id: 200,
        index: 0,
        plan: vec![1, 2, 3],
    };
    let state: OpState = refresh.into();

    assert!(state.is_refreshing());
    assert_eq!(state.op_id(), Some(200));

    let inner = state.as_refreshing().unwrap();
    assert_eq!(inner.plan, vec![1, 2, 3]);
}

#[test]
fn test_payout_state() {
    let payout = PayoutState {
        op_id: 300,
        request_id: 301,
        receiver: receiver_addr(1),
        amount: 1000,
        owner: owner_addr(1),
        escrow_shares: 500,
        burn_shares: 400,
    };
    let state: OpState = payout.into();

    assert!(state.is_payout());
    assert_eq!(state.op_id(), Some(300));

    let inner = state.as_payout().unwrap();
    assert_eq!(inner.request_id, 301);
    assert_eq!(inner.amount, 1000);
    assert_eq!(inner.burn_shares, 400);
}

#[test]
fn test_from_impls() {
    // Test From<IdleState>
    let state: OpState = IdleState.into();
    assert!(state.is_idle());

    // Test From<AllocatingState>
    let alloc = AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 100,
        plan: vec![AllocationPlanEntry::new(0, 100)],
    };
    let state: OpState = alloc.into();
    assert!(state.is_allocating());

    // Test From<WithdrawingState>
    let withdraw = WithdrawingState {
        op_id: 2,
        request_id: 20,
        index: 0,
        remaining: 50,
        collected: 0,
        receiver: receiver_addr(2),
        owner: owner_addr(2),
        escrow_shares: 100,
    };
    let state: OpState = withdraw.into();
    assert!(state.is_withdrawing());
    assert_eq!(state.as_withdrawing().unwrap().request_id, 20);

    // Test From<RefreshingState>
    let refresh = RefreshingState {
        op_id: 3,
        index: 0,
        plan: vec![0],
    };
    let state: OpState = refresh.into();
    assert!(state.is_refreshing());

    // Test From<PayoutState>
    let payout = PayoutState {
        op_id: 4,
        request_id: 40,
        receiver: receiver_addr(3),
        amount: 100,
        owner: owner_addr(3),
        escrow_shares: 100,
        burn_shares: 100,
    };
    let state: OpState = payout.into();
    assert!(state.is_payout());
    assert_eq!(state.as_payout().unwrap().request_id, 40);
}
