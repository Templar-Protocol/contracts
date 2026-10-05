use super::*;
use crate::effects::{KernelEffect, KernelEvent};
#[cfg(feature = "action-epoch-settlement")]
use crate::state::queue::MIN_WITHDRAWAL_ASSETS;
#[cfg(feature = "action-epoch-settlement")]
use crate::state::settlement::{
    EpochId, EpochState, ValuationReportRef, FIRST_SETTLEMENT_EPOCH,
};
use crate::test_utils::{owner_addr, receiver_addr};
#[cfg(feature = "action-epoch-settlement")]
use crate::types::TimestampNs;
use alloc::vec;

fn first_event(result: &TransitionResult) -> Option<&KernelEvent> {
    match result.effects.first() {
        Some(KernelEffect::EmitEvent { event }) => Some(event),
        _ => None,
    }
}

#[test]
fn complete_allocation_skips_zero_amount_pending_withdrawal() {
    let alloc =
        start_allocation(OpState::Idle, vec![AllocationPlanEntry::new(1, 100)], 10).unwrap();
    let alloc = allocation_step_callback(alloc.new_state, true, 100, 10).unwrap();

    let result = complete_allocation(
        alloc.new_state,
        10,
        Some(WithdrawalRequest {
            op_id: 11,
            request_id: 11,
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
    let alloc =
        start_allocation(OpState::Idle, vec![AllocationPlanEntry::new(1, 100)], 20).unwrap();
    let alloc = allocation_step_callback(alloc.new_state, true, 100, 20).unwrap();

    let err = complete_allocation(
        alloc.new_state,
        20,
        Some(WithdrawalRequest {
            op_id: 21,
            request_id: 21,
            amount: 50,
            receiver: receiver_addr(2),
            owner: owner_addr(2),
            escrow_shares: 0,
        }),
    );

    assert!(matches!(err, Err(TransitionError::ZeroEscrowShares)));
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn withdrawal_settled_supports_partial_collection() {
    let withdraw = start_withdrawal(
        OpState::Idle,
        WithdrawalRequest {
            op_id: 7,
            request_id: 7,
            amount: 100,
            receiver: receiver_addr(7),
            owner: owner_addr(7),
            escrow_shares: 100,
        },
    )
    .unwrap();

    let result = withdrawal_settled(withdraw.new_state, 7, 40, 40).unwrap();

    assert_eq!(
        result.new_state,
        OpState::Payout(PayoutState {
            op_id: 7,
            request_id: 7,
            receiver: receiver_addr(7),
            amount: 40,
            owner: owner_addr(7),
            escrow_shares: 100,
            burn_shares: 40,
        })
    );
    assert_eq!(
        first_event(&result),
        Some(&KernelEvent::WithdrawalCollected {
            op_id: 7,
            burn_shares: 40,
            collected: 40,
        })
    );
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn withdrawal_settled_rejects_collection_overflow() {
    let withdraw = start_withdrawal(
        OpState::Idle,
        WithdrawalRequest {
            op_id: 8,
            request_id: 8,
            amount: 100,
            receiver: receiver_addr(8),
            owner: owner_addr(8),
            escrow_shares: 100,
        },
    )
    .unwrap();

    let err = withdrawal_settled(withdraw.new_state, 8, 101, 50);

    assert!(matches!(
        err,
        Err(TransitionError::CollectionOverflow {
            collected: 101,
            remaining: 100,
        })
    ));
}

#[cfg(feature = "action-epoch-settlement")]
const SETTLEMENT_NAV: u128 = 9_000;
#[cfg(feature = "action-epoch-settlement")]
const ELIGIBLE_SUPPLY: u128 = 10_000;
#[cfg(feature = "action-epoch-settlement")]
const HEAD_ESCROW: u128 = 10_000;
#[cfg(feature = "action-epoch-settlement")]
const DERIVED_CLAIM: u128 = 9_000;
#[cfg(feature = "action-epoch-settlement")]
const REPORT_SEQ: u64 = 1;
#[cfg(feature = "action-epoch-settlement")]
const CUTOFF_NS: u64 = 1_000_000_000;
#[cfg(feature = "action-epoch-settlement")]
const VALUATION_NS: u64 = 2_000_000_000;

#[cfg(feature = "action-epoch-settlement")]
fn accepted_report() -> ValuationReportRef {
    ValuationReportRef {
        report_seq: REPORT_SEQ,
        as_of_ns: TimestampNs::from_nanos(VALUATION_NS),
        report_hash: [7u8; 32],
    }
}

#[cfg(feature = "action-epoch-settlement")]
/// Drive the epoch through cutoff and settlement so the snapshot for epoch
/// 1 is accepted and bound as the epoch state's last settled snapshot.
fn settled_epoch() -> EpochState {
    let cutoff = EpochState::genesis()
        .begin_cutoff(TimestampNs::from_nanos(CUTOFF_NS))
        .expect("cutoff accepted");
    let snapshot = cutoff
        .build_settlement_snapshot(
            &accepted_report(),
            SETTLEMENT_NAV,
            ELIGIBLE_SUPPLY,
            TimestampNs::from_nanos(VALUATION_NS),
            VALUATION_NS - CUTOFF_NS,
        )
        .expect("settlement law accepts the valuation");
    let settled = cutoff
        .apply_settled(&snapshot)
        .expect("settled epoch applied");
    assert_eq!(settled.intake_epoch, EpochId::new(FIRST_SETTLEMENT_EPOCH + 1));
    assert_eq!(
        settled
            .last_settled
            .as_ref()
            .map(|snapshot| snapshot.epoch_id()),
        Some(EpochId::new(FIRST_SETTLEMENT_EPOCH))
    );
    settled
}

#[cfg(feature = "action-epoch-settlement")]
/// Queue head bound to an epoch that has no accepted settlement snapshot.
fn unsettled_head() -> EpochState {
    let unsettled = EpochState::genesis();
    assert_eq!(unsettled.last_settled, None);
    assert!(unsettled.check_invariants());
    unsettled
}

#[cfg(feature = "action-epoch-settlement")]
fn withdrawing_state(op_id: u64, request_id: u64, collected: u128, escrow_shares: u128) -> OpState {
    OpState::Withdrawing(WithdrawingState {
        op_id,
        request_id,
        index: 1,
        remaining: 0,
        collected,
        receiver: receiver_addr(request_id),
        owner: owner_addr(request_id),
        escrow_shares,
    })
}

#[cfg(feature = "action-epoch-settlement")]
fn forged_payout(
    op_id: u64,
    request_id: u64,
    amount: u128,
    escrow_shares: u128,
    burn_shares: u128,
) -> OpState {
    OpState::Payout(PayoutState {
        op_id,
        request_id,
        receiver: receiver_addr(request_id),
        amount,
        owner: owner_addr(request_id),
        escrow_shares,
        burn_shares,
    })
}

#[cfg(feature = "action-epoch-settlement")]
fn head_entry(
    escrow_shares: u128,
    min_assets_out: u128,
    epoch: EpochId,
) -> crate::state::queue::PendingWithdrawal {
    let mut head = crate::state::queue::PendingWithdrawal::new(
        owner_addr(1),
        receiver_addr(1),
        escrow_shares,
        min_assets_out,
        TimestampNs::from_nanos(CUTOFF_NS),
        epoch,
    )
    .expect("head escrows positive shares");
    if epoch == EpochId::MIGRATION_INTAKE {
        // Epoch-tagging a queued entry: construction law only guards the
        // intake path, the epoch binding below must still be unpinned.
        head.epoch_id = EpochId::MIGRATION_INTAKE;
    }
    head
}

#[cfg(feature = "action-epoch-settlement")]
/// Law-compliant vault: the head is settled at epoch 1, the accepted
/// snapshot prices that head at `DERIVED_CLAIM`, and idle assets cover the
/// claim in full.
fn settled_vault() -> VaultState {
    // Loss snapshot: settlement NAV 9_000 against 10_000 eligible supply,
    // so the only lawful payout is floor(escrow * 9_000 / 10_000) = 9_000.
    let mut vault = VaultState::with_initial(SETTLEMENT_NAV, ELIGIBLE_SUPPLY, SETTLEMENT_NAV, 0, TimestampNs::ZERO);
    vault.withdraw_queue = crate::state::queue::WithdrawQueue::with_state(
        [(
            1u64,
            head_entry(HEAD_ESCROW, 0, EpochId::new(FIRST_SETTLEMENT_EPOCH)),
        )],
        1,
        2,
    );
    vault.epoch = settled_epoch();
    assert!(vault.check_invariant());
    assert_eq!(
        settled_claim(vault.withdraw_queue.head().unwrap().1, &vault.epoch),
        Some(DERIVED_CLAIM)
    );
    vault
}

#[cfg(feature = "action-epoch-settlement")]
fn law_rejection_is_inert(
    vault: &VaultState,
    run: impl Fn(&VaultState) -> TransitionRes,
    expected: TransitionError,
) {
    let baseline = vault.clone();
    let attempt = run(vault);
    assert!(
        matches!(&attempt, Err(err) if err == &expected),
        "expected {expected:?}, got {attempt:?}"
    );
    if let Ok(result) = attempt {
        assert!(!result.new_state.is_payout());
    }
    assert!(
        vault == &baseline,
        "rejected attempt must leave vault state unchanged"
    );
    assert!(
        !vault.op_state.is_payout() && vault.withdraw_queue.head().is_some(),
        "the unsettled or rejected head must remain queued"
    );
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn withdrawal_settled_rejects_head_whose_epoch_has_no_accepted_settlement() {
    let mut vault = settled_vault();
    vault.epoch = unsettled_head();
    let op = withdrawing_state(101, 1, DERIVED_CLAIM, HEAD_ESCROW);
    law_rejection_is_inert(&vault, |vault| withdrawal_settled(op.clone(), vault, 101, 0), TransitionError::WithdrawalIncomplete { remaining: 0, collected: DERIVED_CLAIM });
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn withdrawal_settled_rejects_wrong_epoch_snapshot() {
    let mut vault = settled_vault();
    vault.withdraw_queue.remove_pending(1);
    vault.withdraw_queue = crate::state::queue::WithdrawQueue::with_state(
        [(1u64, head_entry(HEAD_ESCROW, 0, EpochId::new(FIRST_SETTLEMENT_EPOCH + 1)))],
        1,
        2,
    );
    let op = withdrawing_state(102, 1, DERIVED_CLAIM, HEAD_ESCROW);
    law_rejection_is_inert(&vault, |vault| withdrawal_settled(op.clone(), vault, 102, 0), TransitionError::WithdrawalIncomplete { remaining: 0, collected: DERIVED_CLAIM });
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn withdrawal_collected_rejects_wrong_epoch_head() {
    let mut vault = settled_vault();
    vault.withdraw_queue = crate::state::queue::WithdrawQueue::with_state(
        [(1u64, head_entry(HEAD_ESCROW, 0, EpochId::new(FIRST_SETTLEMENT_EPOCH + 1)))],
        1,
        2,
    );
    let op = withdrawing_state(103, 1, DERIVED_CLAIM, HEAD_ESCROW);
    law_rejection_is_inert(&vault, |vault| withdrawal_collected(op.clone(), vault, 103, 0), TransitionError::WithdrawalIncomplete { remaining: 0, collected: DERIVED_CLAIM });
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn withdrawal_collected_rejects_arbitrary_amount_not_derived_from_snapshot() {
    let vault = settled_vault();
    let op = withdrawing_state(104, 1, DERIVED_CLAIM + 1, HEAD_ESCROW);
    law_rejection_is_inert(&vault, |vault| withdrawal_collected(op.clone(), vault, 104, 0), TransitionError::WithdrawalIncomplete { remaining: 1, collected: DERIVED_CLAIM });
    let op = withdrawing_state(104, 1, DERIVED_CLAIM - 1, HEAD_ESCROW);
    law_rejection_is_inert(&vault, |vault| withdrawal_collected(op.clone(), vault, 104, 0), TransitionError::WithdrawalIncomplete { remaining: 0, collected: DERIVED_CLAIM });
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn withdrawal_settled_rejects_below_floor_claim() {
    let mut vault = settled_vault();
    vault.withdraw_queue.remove_pending(1);
    vault.withdraw_queue = crate::state::queue::WithdrawQueue::with_state(
        [(1u64, head_entry(HEAD_ESCROW, DERIVED_CLAIM + 1, EpochId::new(FIRST_SETTLEMENT_EPOCH)))],
        1,
        2,
    );
    let op = withdrawing_state(105, 1, DERIVED_CLAIM, HEAD_ESCROW);
    law_rejection_is_inert(&vault, |vault| withdrawal_settled(op.clone(), vault, 105, 0), TransitionError::WithdrawalIncomplete { remaining: 1, collected: DERIVED_CLAIM });
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn withdrawal_collected_rejects_protocol_floor_below_derived_claim() {
    let vault = settled_vault();
    let op = withdrawing_state(112, 1, DERIVED_CLAIM, HEAD_ESCROW);
    law_rejection_is_inert(
        &vault,
        |vault| {
            withdrawal_collected(
                op.clone(),
                vault,
                112,
                MIN_WITHDRAWAL_ASSETS + DERIVED_CLAIM,
            )
        },
        TransitionError::WithdrawalIncomplete {
            remaining: MIN_WITHDRAWAL_ASSETS,
            collected: DERIVED_CLAIM,
        },
    );
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_transitions_reject_partial_escrow_burn() {
    let vault = settled_vault();
    law_rejection_is_inert(
        &vault,
        |vault| {
            payout_complete(
                forged_payout(106, 1, DERIVED_CLAIM, HEAD_ESCROW, HEAD_ESCROW / 2),
                vault,
                true,
                106,
                receiver_addr(1),
                0,
            )
        },
        TransitionError::BurnExceedsEscrow {
            burn: HEAD_ESCROW / 2,
            escrow: HEAD_ESCROW,
        },
    );
    law_rejection_is_inert(
        &vault,
        |vault| {
            payout_complete(
                forged_payout(106, 1, DERIVED_CLAIM, HEAD_ESCROW, 0),
                vault,
                true,
                106,
                receiver_addr(1),
                0,
            )
        },
        TransitionError::BurnExceedsEscrow {
            burn: 0,
            escrow: HEAD_ESCROW,
        },
    );
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_complete_rejects_forged_amount_and_wrong_epoch_payout() {
    let vault = settled_vault();
    law_rejection_is_inert(
        &vault,
        |vault| {
            payout_complete(
                forged_payout(107, 1, 123_456_789, HEAD_ESCROW, HEAD_ESCROW),
                vault,
                true,
                107,
                receiver_addr(1),
                0,
            )
        },
        TransitionError::WithdrawalIncomplete {
            remaining: 123_456_789 - DERIVED_CLAIM,
            collected: DERIVED_CLAIM,
        },
    );
    let mut wrong_epoch = settled_vault();
    wrong_epoch.withdraw_queue.remove_pending(1);
    wrong_epoch.withdraw_queue = crate::state::queue::WithdrawQueue::with_state(
        [(1u64, head_entry(HEAD_ESCROW, 0, EpochId::new(FIRST_SETTLEMENT_EPOCH + 1)))],
        1,
        2,
    );
    law_rejection_is_inert(
        &wrong_epoch,
        |vault| {
            payout_complete(
                forged_payout(113, 1, DERIVED_CLAIM, HEAD_ESCROW, HEAD_ESCROW),
                vault,
                true,
                113,
                receiver_addr(1),
                0,
            )
        },
        TransitionError::WithdrawalIncomplete {
            remaining: 0,
            collected: DERIVED_CLAIM,
        },
    );
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_transitions_reject_payout_not_matching_queue_head() {
    let vault = settled_vault();
    law_rejection_is_inert(
        &vault,
        |vault| withdrawal_collected(withdrawing_state(108, 9, DERIVED_CLAIM, HEAD_ESCROW), vault, 108, 0),
        TransitionError::WrongState,
    );
    law_rejection_is_inert(
        &vault,
        |vault| {
            payout_complete(
                forged_payout(108, 9, DERIVED_CLAIM, HEAD_ESCROW, HEAD_ESCROW),
                vault,
                true,
                108,
                receiver_addr(1),
                0,
            )
        },
        TransitionError::WrongState,
    );
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_transitions_reject_insufficient_idle_liquidity_without_partial_payout() {
    let mut vault = settled_vault();
    vault.idle_assets = DERIVED_CLAIM - 1;
    vault.total_assets = vault.idle_assets;
    let op = withdrawing_state(109, 1, DERIVED_CLAIM, HEAD_ESCROW);
    law_rejection_is_inert(&vault, |vault| withdrawal_collected(op.clone(), vault, 109, 0), TransitionError::WithdrawalIncomplete { remaining: 1, collected: DERIVED_CLAIM });
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn unsettled_head_rejects_every_payout_entrypoint_without_state_change() {
    let mut vault = settled_vault();
    vault.epoch = unsettled_head();
    let op = withdrawing_state(114, 1, DERIVED_CLAIM, HEAD_ESCROW);
    let baseline = vault.clone();
    assert!(matches!(
        withdrawal_collected(op.clone(), &vault, 114, 0),
        Err(TransitionError::WithdrawalIncomplete { .. })
    ));
    assert!(matches!(
        withdrawal_settled(op.clone(), &vault, 114, 0),
        Err(TransitionError::WithdrawalIncomplete { .. })
    ));
    assert!(matches!(
        payout_complete(op.clone(), &vault, true, 114, receiver_addr(1), 0),
        Err(TransitionError::WrongState)
    ));
    assert!(matches!(
        payout_complete(op, &vault, false, 114, receiver_addr(1), 0),
        Err(TransitionError::WrongState)
    ));
    assert!(vault == baseline);
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_complete_burns_full_escrow_once_on_lawful_settlement() {
    let vault = settled_vault();
    let op = withdrawing_state(110, 1, DERIVED_CLAIM, HEAD_ESCROW);
    let payout = withdrawal_collected(op.clone(), &vault, 110, 0)
        .expect("law-compliant payout is authorized");
    assert_eq!(
        payout.new_state,
        OpState::Payout(PayoutState {
            op_id: 110,
            request_id: 1,
            receiver: receiver_addr(1),
            amount: DERIVED_CLAIM,
            owner: owner_addr(1),
            escrow_shares: HEAD_ESCROW,
            burn_shares: HEAD_ESCROW,
        })
    );
    assert_eq!(
        first_event(&payout),
        Some(&KernelEvent::WithdrawalCollected {
            op_id: 110,
            burn_shares: HEAD_ESCROW,
            collected: DERIVED_CLAIM,
        })
    );

    let settled = withdrawal_settled(op, &vault, 110, 0).expect("settled entry authorizes the same law");
    assert_eq!(settled.new_state, payout.new_state);

    let vault = settled_vault();
    let escrow_address = receiver_addr(1);
    let completed = payout_complete(payout.new_state.clone(), &vault, true, 110, escrow_address, 0)
        .expect("law-compliant payout settles");
    assert!(completed.new_state.is_idle());
    assert_eq!(
        completed.effects,
        vec![
            KernelEffect::BurnShares {
                owner: escrow_address,
                shares: HEAD_ESCROW,
            },
            KernelEffect::EmitEvent {
                event: KernelEvent::PayoutCompleted {
                    op_id: 110,
                    success: true,
                    burn_shares: HEAD_ESCROW,
                    refund_shares: 0,
                    amount: DERIVED_CLAIM,
                },
            },
        ],
        "a successful payout burns the request's full escrow exactly once and refunds nothing"
    );
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn failed_payout_refunds_full_escrow_without_burn() {
    let vault = settled_vault();
    let payout = forged_payout(111, 1, DERIVED_CLAIM, HEAD_ESCROW, HEAD_ESCROW);
    let escrow_address = receiver_addr(1);
    let completed = payout_complete(payout, &vault, false, 111, escrow_address, 0)
        .expect("failed payout refunds the escrow");
    assert!(completed.new_state.is_idle());
    assert_eq!(
        completed.effects,
        vec![
            KernelEffect::TransferShares {
                from: escrow_address,
                to: owner_addr(1),
                shares: HEAD_ESCROW,
            },
            KernelEffect::EmitEvent {
                event: KernelEvent::PayoutCompleted {
                    op_id: 111,
                    success: false,
                    burn_shares: 0,
                    refund_shares: HEAD_ESCROW,
                    amount: 0,
                },
            },
        ]
    );
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_transitions_reject_wrong_op_id() {
    let vault = settled_vault();
    let op = withdrawing_state(115, 1, DERIVED_CLAIM, HEAD_ESCROW);
    law_rejection_is_inert(&vault, |vault| withdrawal_collected(op.clone(), vault, 999, 0), TransitionError::OpIdMismatch { expected: 115, actual: 999 });
    law_rejection_is_inert(
        &vault,
        |vault| {
            payout_complete(
                forged_payout(115, 1, DERIVED_CLAIM, HEAD_ESCROW, HEAD_ESCROW),
                vault,
                true,
                999,
                receiver_addr(1),
                0,
            )
        },
        TransitionError::OpIdMismatch {
            expected: 115,
            actual: 999,
        },
    );
}
