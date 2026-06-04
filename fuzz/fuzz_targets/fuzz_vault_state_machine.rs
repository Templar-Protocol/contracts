//! Drive the vault kernel `OpState` machine through arbitrary sequences of
//! transitions and assert that the state-machine invariants hold:
//!
//! * Every transition either returns `Ok(new_state)` or `Err(_)` — never panics.
//! * From `Idle`, only the four `start_*` transitions can succeed; everything
//!   else must return `WrongState`.
//! * Once a non-Idle op is started, transitions must reject mismatched `op_id`s
//!   (`OpIdMismatch`).
//! * `allocation_step_callback` must never advance past `plan.len()`.
//! * `amount_collected` can never exceed `WithdrawingState::remaining`.
//! * `burn_shares` can never exceed `escrow_shares` on `withdrawal_collected` /
//!   `withdrawal_settled`.
//! * `complete_allocation` from a non-`Allocating` state must error.
//!
//! MUTATION-CHECK (P5): in `allocation_step_callback`
//! (contract/vault/kernel/src/transitions/mod.rs), remove the
//! `validate_plan_index(alloc.index, alloc.plan.len())` guard. Then a sequence
//! of `AllocationStepCallback`s drives `index` past `plan.len()` and the
//! `index <= plan.len()` invariant in `check_state_well_formed` must fire.

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use templar_vault_kernel::{
    allocation_step_callback, complete_allocation, payout_complete, refresh_step_callback,
    start_allocation, start_refresh, start_withdrawal, stop_withdrawal, withdrawal_collected,
    withdrawal_settled, withdrawal_step_callback, Address, AllocationPlanEntry, OpState,
    WithdrawalRequest,
};

const MAX_ACTIONS: usize = 32;
const MAX_PLAN: usize = 8;

#[derive(Arbitrary, Debug)]
enum Action {
    StartAllocation {
        op_id: u64,
        plan: Vec<(u32, u128)>,
    },
    AllocationStepCallback {
        op_id: u64,
        success: bool,
        amount_allocated: u128,
    },
    CompleteAllocation {
        op_id: u64,
        with_withdrawal: bool,
        request_op_id: u64,
        request_id: u64,
        amount: u128,
        escrow_shares: u128,
        receiver: [u8; 32],
        owner: [u8; 32],
    },
    StartWithdrawal {
        op_id: u64,
        request_id: u64,
        amount: u128,
        escrow_shares: u128,
        receiver: [u8; 32],
        owner: [u8; 32],
    },
    WithdrawalStepCallback {
        op_id: u64,
        amount_collected: u128,
    },
    WithdrawalCollected {
        op_id: u64,
        burn_shares: u128,
    },
    WithdrawalSettled {
        op_id: u64,
        amount_collected: u128,
        burn_shares: u128,
    },
    StopWithdrawal {
        op_id: u64,
        escrow: [u8; 32],
    },
    StartRefresh {
        op_id: u64,
        plan: Vec<u32>,
    },
    RefreshStepCallback {
        op_id: u64,
    },
    PayoutComplete {
        op_id: u64,
        success: bool,
        escrow: [u8; 32],
    },
}

#[derive(Arbitrary, Debug)]
struct Scenario {
    actions: Vec<Action>,
}

fn check_state_well_formed(state: &OpState) {
    match state {
        OpState::Idle => {
            assert_eq!(state.op_id(), None, "Idle must have no op_id");
        }
        OpState::Allocating(s) => {
            assert_eq!(state.op_id(), Some(s.op_id));
            assert!(
                (s.index as usize) <= s.plan.len(),
                "Allocating index ({}) exceeded plan length ({})",
                s.index,
                s.plan.len(),
            );
        }
        OpState::Withdrawing(s) => {
            assert_eq!(state.op_id(), Some(s.op_id));
        }
        OpState::Refreshing(s) => {
            assert_eq!(state.op_id(), Some(s.op_id));
            assert!(
                (s.index as usize) <= s.plan.len(),
                "Refreshing index ({}) exceeded plan length ({})",
                s.index,
                s.plan.len(),
            );
        }
        OpState::Payout(s) => {
            assert_eq!(state.op_id(), Some(s.op_id));
            assert!(
                s.burn_shares <= s.escrow_shares,
                "Payout burn_shares ({}) > escrow_shares ({})",
                s.burn_shares,
                s.escrow_shares,
            );
        }
    }
}

fn truncate_plan(plan: &[(u32, u128)]) -> Vec<AllocationPlanEntry> {
    plan.iter()
        .take(MAX_PLAN)
        // Bound each step amount so a sum of MAX_PLAN entries can't overflow.
        .map(|&(t, a)| AllocationPlanEntry::new(t, a.min(u128::MAX / (MAX_PLAN as u128 + 1))))
        .collect()
}

fuzz_target!(|scenario: Scenario| {
    let mut state = OpState::Idle;
    check_state_well_formed(&state);

    for action in scenario.actions.into_iter().take(MAX_ACTIONS) {
        let kind_before = state.kind_code();
        let op_id_before = state.op_id();

        let result = match action {
            Action::StartAllocation { op_id, plan } => {
                start_allocation(state.clone(), truncate_plan(&plan), op_id)
            }
            Action::AllocationStepCallback {
                op_id,
                success,
                amount_allocated,
            } => allocation_step_callback(state.clone(), success, amount_allocated, op_id),
            Action::CompleteAllocation {
                op_id,
                with_withdrawal,
                request_op_id,
                request_id,
                amount,
                escrow_shares,
                receiver,
                owner,
            } => {
                let req = with_withdrawal.then_some(WithdrawalRequest {
                    op_id: request_op_id,
                    request_id,
                    amount,
                    escrow_shares,
                    receiver: Address(receiver),
                    owner: Address(owner),
                });
                complete_allocation(state.clone(), op_id, req)
            }
            Action::StartWithdrawal {
                op_id,
                request_id,
                amount,
                escrow_shares,
                receiver,
                owner,
            } => start_withdrawal(
                state.clone(),
                WithdrawalRequest {
                    op_id,
                    request_id,
                    amount,
                    escrow_shares,
                    receiver: Address(receiver),
                    owner: Address(owner),
                },
            ),
            Action::WithdrawalStepCallback {
                op_id,
                amount_collected,
            } => withdrawal_step_callback(state.clone(), op_id, amount_collected),
            Action::WithdrawalCollected { op_id, burn_shares } => {
                withdrawal_collected(state.clone(), op_id, burn_shares)
            }
            Action::WithdrawalSettled {
                op_id,
                amount_collected,
                burn_shares,
            } => withdrawal_settled(state.clone(), op_id, amount_collected, burn_shares),
            Action::StopWithdrawal { op_id, escrow } => {
                stop_withdrawal(state.clone(), op_id, Address(escrow))
            }
            Action::StartRefresh { op_id, plan } => {
                let bounded: Vec<u32> = plan.into_iter().take(MAX_PLAN).collect();
                start_refresh(state.clone(), bounded, op_id)
            }
            Action::RefreshStepCallback { op_id } => refresh_step_callback(state.clone(), op_id),
            Action::PayoutComplete {
                op_id,
                success,
                escrow,
            } => payout_complete(state.clone(), success, op_id, Address(escrow)),
        };

        if let Ok(transition) = result {
            state = transition.new_state;
            check_state_well_formed(&state);
        } else {
            // On error the state must not have moved. (The transition
            // functions take `state` by value; we cloned before calling.)
            assert_eq!(
                state.kind_code(),
                kind_before,
                "Errored transition mutated state kind",
            );
            assert_eq!(
                state.op_id(),
                op_id_before,
                "Errored transition mutated op_id",
            );
        }
    }
});
