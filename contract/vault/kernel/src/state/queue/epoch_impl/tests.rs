//! Tests for the unpriced, epoch-bound withdrawal queue.
//!
//! Every settlement assertion recomputes claims from an accepted epoch
//! settlement; no test can express a pre-settlement fixed claim because the
//! entry type has no asset field.

use super::*;
use crate::state::settlement::{EpochPhase, SettlementRejection, ValuationReportRef};
use alloc::vec;

const TEST_MAX_PENDING: u32 = MAX_PENDING as u32;

fn owner_addr(seed: u64) -> Address {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&seed.to_le_bytes());
    Address::from_bytes(bytes)
}

fn ts(ns: u64) -> TimestampNs {
    TimestampNs::from_nanos(ns)
}

fn new_request(owner: u64, shares: u128, epoch: EpochId) -> PendingWithdrawal {
    PendingWithdrawal::new(
        owner_addr(owner),
        owner_addr(owner),
        shares,
        0,
        ts(1_000),
        epoch,
    )
    .expect("valid unpriced request")
}

fn genesis_epoch() -> EpochState {
    EpochState::genesis()
}

fn cutoff_state() -> EpochState {
    genesis_epoch()
        .begin_cutoff(ts(1_000))
        .expect("cutoff from genesis")
}

fn report(seq: u64, as_of: u64) -> ValuationReportRef {
    ValuationReportRef {
        report_seq: seq,
        as_of_ns: ts(as_of),
        report_hash: [seq as u8; 32],
    }
}

/// Settle the first epoch at a 1:1 NAV for supply 1_000_000 and return the
/// post-settlement epoch state.
fn settled_state() -> EpochState {
    let cutoff = cutoff_state();
    let snapshot = cutoff
        .build_settlement_snapshot(&report(1, 1_000), 1_000_000, 1_000_000, ts(1_500), 2_000)
        .expect("accepted settlement");
    cutoff.apply_settled(&snapshot).expect("settlement applied")
}

fn enqueue_epoch1(queue: &mut WithdrawQueue, owner: u64, shares: u128) -> u64 {
    let request = new_request(owner, shares, EpochId::FIRST_SETTLEMENT);
    queue
        .enqueue_withdrawal(request, TEST_MAX_PENDING)
        .expect("enqueue accepted")
}

#[test]
fn new_unpriced_request_has_no_asset_claim_fields() {
    let w = new_request(1, 100, EpochId::FIRST_SETTLEMENT);
    // Identity and escrow are preserved; the only asset figure is the
    // caller-declared slippage bound, and settlement has not occurred.
    assert_eq!(w.owner, owner_addr(1));
    assert_eq!(w.receiver, owner_addr(1));
    assert_eq!(w.escrow_shares, 100);
    assert_eq!(w.min_assets_out, 0);
    assert_eq!(w.requested_at_ns, ts(1_000));
    assert_eq!(w.epoch_id, EpochId::FIRST_SETTLEMENT);
    assert!(!w.is_migrated_legacy());

    let open = genesis_epoch();
    assert_eq!(settled_claim(&w, &open), None);
    assert!(!can_satisfy_withdrawal(&w, &open, u128::MAX));
    assert!(!can_partially_satisfy(&w, &open, u128::MAX));
    assert_eq!(compute_full_withdrawal(&w, &open, u128::MAX), None);
    let partial = compute_partial_withdrawal(&w, &open, 5_000);
    assert_eq!(partial.assets_out, 0);
    assert_eq!(partial.settlement.refund, w.escrow_shares);
    assert_eq!(partial.settlement.to_burn, 0);
}

#[test]
fn zero_escrow_and_migration_epoch_rejected_at_construction() {
    assert_eq!(
        PendingWithdrawal::new(
            owner_addr(1),
            owner_addr(1),
            0,
            0,
            ts(1_000),
            EpochId::FIRST_SETTLEMENT
        ),
        Err(RequestError::ZeroEscrowShares)
    );
    assert_eq!(
        PendingWithdrawal::new(
            owner_addr(1),
            owner_addr(1),
            100,
            0,
            ts(1_000),
            EpochId::MIGRATION_INTAKE
        ),
        Err(RequestError::InvalidEpoch),
        "new intake cannot masquerade as migration intake"
    );
}

#[test]
fn migrated_legacy_entries_keep_identity_and_carry_no_quote() {
    let migrated = PendingWithdrawal::migrated_legacy(
        owner_addr(7),
        owner_addr(9),
        500,
        ts(42),
    )
    .expect("migrated entry");
    assert_eq!(migrated.owner, owner_addr(7));
    assert_eq!(migrated.receiver, owner_addr(9));
    assert_eq!(migrated.escrow_shares, 500);
    assert_eq!(migrated.requested_at_ns, ts(42));
    assert!(migrated.is_migrated_legacy());
    assert_eq!(migrated.epoch_id, EpochId::MIGRATION_INTAKE);
    // The legacy fixed claim is pinned away, not merely unrecommended.
    assert_eq!(migrated.min_assets_out, 0);

    assert_eq!(
        PendingWithdrawal::migrated_legacy(owner_addr(7), owner_addr(9), 0, ts(42)),
        Err(RequestError::ZeroEscrowShares)
    );
}

#[test]
fn settlement_pricing_only_after_accepted_epoch_snapshot() {
    let pre = genesis_epoch();
    let w = new_request(2, 400, EpochId::FIRST_SETTLEMENT);
    assert_eq!(settled_claim(&w, &pre), None);

    let settled = settled_state();
    // floor(400 * 1_000_000 / 1_000_000) = 400, recomputed on demand.
    assert_eq!(settled_claim(&w, &settled), Some(400));
    assert!(can_satisfy_withdrawal(&w, &settled, 400));
    assert!(!can_satisfy_withdrawal(&w, &settled, 399));
    assert!(can_partially_satisfy(&w, &settled, 150));
    let full = compute_full_withdrawal(&w, &settled, 400).expect("claim payable");
    assert_eq!(full.assets_out, 400);
    assert_eq!(full.settlement.to_burn, 400);
    assert_eq!(full.settlement.refund, 0);
}

#[test]
fn floor_rounding_and_overflow_are_checked() {
    let settled = settled_state();
    let w = new_request(3, 999, EpochId::FIRST_SETTLEMENT);
    // 999 * 1_000_000 / 1_000_000 = 999 exactly (1:1). Floor behaviour is
    // exercised through a fractional snapshot below.
    assert_eq!(settled_claim(&w, &settled), Some(999));

    let cutoff = cutoff_state();
    let frac = cutoff
        .build_settlement_snapshot(&report(1, 1_000), 1_000_000, 3, ts(1_500), 2_000)
        .expect("fractional settlement");
    // floor(1 * 1_000_000 / 3) = 333_333
    assert_eq!(frac.claim_for(1), Some(333_333));
    // floor(2 * 1_000_000 / 3) = 666_666 (never rounded up).
    assert_eq!(frac.claim_for(2), Some(666_666));

    // Product beyond u128 surfaces as None, never as wrapped arithmetic.
    let cutoff2 = cutoff_state();
    let huge = cutoff2
        .build_settlement_snapshot(
            &report(1, 1_000),
            u128::MAX,
            1,
            ts(1_500),
            2_000,
        )
        .expect("huge-nav settlement");
    assert_eq!(huge.claim_for(2), None);
}

#[test]
fn epoch_binding_isolates_requests_per_epoch() {
    let settled = settled_state(); // only epoch 1 has settled (NAV 1:1)

    let epoch1 = new_request(4, 250, EpochId::FIRST_SETTLEMENT);
    assert_eq!(settled_claim(&epoch1, &settled), Some(250));

    let epoch2 = new_request(5, 250, settled.intake_epoch);
    assert_eq!(epoch2.epoch_id, EpochId::FIRST_SETTLEMENT.checked_next().unwrap());
    assert_eq!(
        settled_claim(&epoch2, &settled),
        None,
        "an epoch-2 request must not price against the epoch-1 snapshot"
    );
}

#[test]
fn migrated_entries_repay_at_first_settlement_only() {
    let migrated =
        PendingWithdrawal::migrated_legacy(owner_addr(8), owner_addr(8), 300, ts(10)).unwrap();

    // Before the first settlement: no claim, queue must keep its head.
    let pre = cutoff_state();
    assert_eq!(settled_claim(&migrated, &pre), None);

    // After the first accepted settlement: repaid from the bound snapshot.
    let settled = settled_state();
    assert_eq!(settled_claim(&migrated, &settled), Some(300));
    assert!(can_satisfy_withdrawal(&migrated, &settled, 300));
}

#[test]
fn min_assets_out_slippage_bound_governs_payout() {
    let mut w = new_request(6, 500, EpochId::FIRST_SETTLEMENT);
    w.min_assets_out = 600;
    let settled = settled_state();
    // Recomputed claim is 500 < 600 floor: not payable; refund preserves shares.
    assert_eq!(settled_claim(&w, &settled), Some(500));
    assert!(!can_satisfy_withdrawal(&w, &settled, u128::MAX));
    assert_eq!(compute_full_withdrawal(&w, &settled, u128::MAX), None);
}

#[test]
fn fifo_head_cannot_be_skipped_across_unsettled_epochs() {
    let mut queue = WithdrawQueue::new();
    let head = enqueue_epoch1(&mut queue, 11, 100);
    let later_request = new_request(12, 100, EpochId::FIRST_SETTLEMENT.checked_next().unwrap());
    let later = queue
        .enqueue_withdrawal(later_request, TEST_MAX_PENDING)
        .expect("later epoch accepted");

    let settled = settled_state();
    assert_eq!(queue.head(), Some((head, queue.get(head).unwrap())));
    assert_eq!(queue.settled_head_claim(&settled), Some(100));

    // Head claim exists and is payable in FIFO order.
    let claims: Vec<_> = queue
        .iter()
        .map(|(_, w)| settled_claim(w, &settled))
        .collect();
    assert_eq!(claims, vec![Some(100), None]);
    let count = queue
        .iter()
        .map(|(_, w)| w)
        .collect::<Vec<_>>();
    let (counted, spent) = count_satisfiable(count.iter().copied(), &settled, 1_000);
    assert_eq!((counted, spent), (1, 100), "head pays, epoch-2 tail waits");

    let (id, withdrawn) = queue.dequeue().expect("dequeue head");
    assert_eq!(id, head);
    assert_eq!(withdrawn.escrow_shares, 100);
    assert_eq!(queue.head(), Some((later, queue.get(later).unwrap())));
    assert_eq!(queue.settled_head_claim(&settled), None);
    assert!(queue.check_invariants());
}

#[test]
fn queue_status_is_share_denominated() {
    let mut queue = WithdrawQueue::new();
    enqueue_epoch1(&mut queue, 21, 100);
    enqueue_epoch1(&mut queue, 22, 200);
    enqueue_epoch1(&mut queue, 23, 300);

    let status = queue.status();
    assert_eq!(status.length, 3);
    assert_eq!(status.total_escrow_shares, 600);
    assert_eq!(queue.total_escrow_shares(), 600);
    // `QueueStatus` has no asset field: the type cannot leak a payout sum.
    assert!(queue.check_invariants());
}

#[test]
fn find_request_status_reports_share_depth() {
    let mut queue = WithdrawQueue::new();
    enqueue_epoch1(&mut queue, 31, 100);
    enqueue_epoch1(&mut queue, 32, 200);

    let status = find_request_status(queue.iter().map(|(_, w)| w), &owner_addr(32))
        .expect("status found");
    assert_eq!(status.index, 1);
    assert_eq!(status.depth_escrow_shares, 100);
    assert_eq!(status.withdrawal.owner, owner_addr(32));
    assert!(find_request_status(queue.iter().map(|(_, w)| w), &owner_addr(99)).is_none());
}

#[test]
fn migration_gate_helpers_are_exact() {
    let mut queue = WithdrawQueue::new();
    enqueue_epoch1(&mut queue, 41, 100);
    assert!(!queue.has_migrated_intake());
    assert!(!queue.has_intake_before(EpochId::FIRST_SETTLEMENT));

    let migrated =
        PendingWithdrawal::migrated_legacy(owner_addr(42), owner_addr(42), 100, ts(5)).unwrap();
    queue.enqueue_withdrawal(migrated, TEST_MAX_PENDING).unwrap();
    assert!(queue.has_migrated_intake());
    assert!(queue.has_intake_before(EpochId::FIRST_SETTLEMENT));
    assert!(queue.has_intake_before(EpochId::new(1)));
    assert!(queue.has_intake_before(EpochId::new(2)));
    assert!(queue.check_invariants());
}

#[test]
fn capacity_and_head_invariants_hold() {
    let mut queue = WithdrawQueue::new();
    enqueue_epoch1(&mut queue, 61, 100);
    let next = new_request(62, 100, EpochId::FIRST_SETTLEMENT);
    assert_eq!(
        queue.enqueue_withdrawal(next, 1),
        Err(QueueError::QueueFull { current: 1, max: 1 })
    );
    assert!(queue.check_invariants_with_max(TEST_MAX_PENDING));

    // Head removal repairs the head pointer; queue empties cleanly.
    let (id, _) = queue.dequeue().expect("dequeue");
    assert_eq!(id, 0);
    assert!(queue.is_empty());
    assert_eq!(queue.head(), None);
    assert_eq!(queue.total_escrow_shares(), 0);
    assert_eq!(queue.status().length, 0);
    assert!(queue.check_invariants());
}

#[test]
fn escrow_cache_tampering_is_detected() {
    let mut queue = WithdrawQueue::new();
    enqueue_epoch1(&mut queue, 71, 100);
    queue.cached_total_escrow = 999;
    assert!(!queue.check_invariants());
}

#[test]
fn settlement_law_rejects_adversarial_reports() {
    let cutoff = cutoff_state();
    // Future-dated relative to settlement time.
    assert_eq!(
        cutoff.build_settlement_snapshot(&report(1, 5_000), 1, 1, ts(1_500), 2_000),
        Err(SettlementRejection::ReportFutureDated)
    );
    // Stale: as_of older than max age at settlement time.
    assert_eq!(
        cutoff.build_settlement_snapshot(&report(1, 1_000), 1, 1, ts(10_000), 2_000),
        Err(SettlementRejection::ReportTooOld)
    );
    // Non-cutoff phase cannot settle.
    assert_eq!(
        genesis_epoch().build_settlement_snapshot(&report(1, 1_000), 1, 1, ts(1_500), 2_000),
        Err(SettlementRejection::PhaseNotCutoff)
    );

    let snapshot = cutoff
        .build_settlement_snapshot(&report(9, 1_200), 2_000_000, 1_000_000, ts(1_500), 2_000)
        .expect("valid settlement");
    let settled = cutoff.apply_settled(&snapshot).unwrap();
    assert_eq!(settled.phase, EpochPhase::Open);
    assert_eq!(settled.intake_epoch, EpochId::FIRST_SETTLEMENT.checked_next().unwrap());
    assert!(settled.check_invariants());
}

#[test]
fn epoch_snapshots_are_immutable_and_monotonic() {
    let cutoff = cutoff_state();
    let snapshot = cutoff
        .build_settlement_snapshot(&report(1, 1_000), 1_000_000, 1_000_000, ts(1_500), 2_000)
        .expect("settlement");
    let settled = cutoff.apply_settled(&snapshot).unwrap();

    // Replaying the same settlement into the next cutoff is rejected:
    // report sequence must strictly advance across settlements.
    let next_cutoff = settled.begin_cutoff(ts(6_000)).unwrap();
    let replay = next_cutoff.build_settlement_snapshot(
        &report(1, 6_000),
        9_000_000,
        1_000_000,
        ts(6_500),
        2_000,
    );
    assert_eq!(
        replay,
        Err(SettlementRejection::ReportSequenceNonMonotonic)
    );

    // The bound snapshot still describes the original settlement exactly.
    let bound = settled.last_settled.clone().expect("snapshot present");
    assert_eq!(bound.epoch_id(), EpochId::FIRST_SETTLEMENT);
    assert_eq!(bound.report_seq(), 1);
    assert_eq!(bound.as_of_ns(), ts(1_000));
    assert_eq!(bound.settlement_nav(), 1_000_000);
    assert_eq!(bound.eligible_supply(), 1_000_000);
    assert_eq!(bound.cutoff_ns(), ts(1_000));
    assert_eq!(bound.claim_for(123_456), Some(123_456));
}

#[test]
fn settled_epoch_state_check_invariants() {
    assert!(genesis_epoch().check_invariants());
    assert!(cutoff_state().check_invariants());
    assert!(settled_state().check_invariants());

    let mut broken = settled_state();
    broken.intake_epoch = EpochId::MIGRATION_INTAKE;
    assert!(!broken.check_invariants());
}

#[test]
fn legacy_migration_entries_cannot_enter_new_settlement_math() {
    // A migrated entry is only ever priced through `settled_claim_for` at
    // an accepted first-settled snapshot. Constructing it with any asset
    // quote is impossible: the constructor takes no assets parameter, and
    // `min_assets_out` is pinned to zero.
    let migrated =
        PendingWithdrawal::migrated_legacy(owner_addr(88), owner_addr(88), 700, ts(3)).unwrap();
    assert_eq!(migrated.min_assets_out, 0);
    assert_eq!(migrated.epoch_id, EpochId::MIGRATION_INTAKE);

    // Queue integrity: migration entries are indistinguishable storage-wise
    // from new entries except by the migration epoch tag, and the queue
    // exposes that tag as an explicit, gateable signal.
    let mut queue = WithdrawQueue::new();
    queue
        .enqueue_withdrawal(migrated, TEST_MAX_PENDING)
        .expect("migration intake enqueued");
    assert!(queue.has_migrated_intake());
    assert_eq!(queue.total_escrow_shares(), 700);
    assert!(queue.check_invariants());

    let settled = settled_state();
    assert_eq!(queue.settled_head_claim(&settled), Some(700));
}

#[test]
fn settlement_requires_idle_coverage_before_payout() {
    // Claim math is independent of liquidity; the execution lane must cap
    // payouts by recomputed claims and available idle assets. With zero
    // idle assets, even settled claims pay nothing and burn nothing.
    let settled = settled_state();
    let w = new_request(91, 500, EpochId::FIRST_SETTLEMENT);
    assert_eq!(compute_full_withdrawal(&w, &settled, 0), None);
    let result = compute_partial_withdrawal(&w, &settled, 0);
    assert_eq!(result.assets_out, 0);
    assert_eq!(result.settlement.refund, 500);
    assert_eq!(result.settlement.to_burn, 0);
}

#[test]
fn count_satisfiable_never_overspends_or_skips_heads() {
    let mut queue = WithdrawQueue::new();
    enqueue_epoch1(&mut queue, 101, 100);
    enqueue_epoch1(&mut queue, 102, 200);
    enqueue_epoch1(&mut queue, 103, 300);

    let settled = settled_state();
    let refs: Vec<&PendingWithdrawal> = queue.iter().map(|(_, w)| w).collect();

    assert_eq!(count_satisfiable(refs.iter().copied(), &settled, 0), (0, 0));
    assert_eq!(count_satisfiable(refs.iter().copied(), &settled, 150), (1, 100));
    assert_eq!(count_satisfiable(refs.iter().copied(), &settled, 299), (1, 100));
    assert_eq!(count_satisfiable(refs.iter().copied(), &settled, 300), (2, 300));
    assert_eq!(
        count_satisfiable(refs.iter().copied(), &settled, u128::MAX),
        (3, 600)
    );
}
