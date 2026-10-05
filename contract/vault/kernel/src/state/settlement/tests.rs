use super::*;
use crate::types::TimestampNs;

fn ts(ns: u64) -> TimestampNs {
    TimestampNs::from_nanos(ns)
}

fn report(seq: u64, as_of: u64) -> ValuationReportRef {
    ValuationReportRef {
        report_seq: seq,
        as_of_ns: ts(as_of),
        report_hash: [seq as u8; 32],
    }
}

fn cutoff_state() -> EpochState {
    EpochState::genesis()
        .begin_cutoff(ts(1_000))
        .expect("cutoff from genesis")
}

fn settle_now() -> TimestampNs {
    ts(1_500)
}

const MAX_AGE: u64 = 2_000;

#[test]
fn genesis_opens_first_settlement_epoch() {
    let state = EpochState::genesis();
    assert_eq!(state.phase, EpochPhase::Open);
    assert_eq!(state.intake_epoch, EpochId::FIRST_SETTLEMENT);
    assert!(state.is_accepting_requests());
    assert_eq!(state.intake_epoch_id(), Some(EpochId::FIRST_SETTLEMENT));
    assert!(state.check_invariants());
}

#[test]
fn epoch_ids_are_monotonic_and_bounded() {
    assert_eq!(
        EpochId(u64::MAX).checked_next(),
        None,
        "epoch counter must not wrap"
    );
    assert_eq!(
        EpochId::FIRST_SETTLEMENT.checked_next(),
        Some(EpochId::new(2))
    );
    assert!(!EpochId::MIGRATION_INTAKE.is_settlement_epoch());
    assert!(EpochId::FIRST_SETTLEMENT.is_settlement_epoch());
}

#[test]
fn cutoff_closes_intake() {
    let before = EpochState::genesis();
    let after = before
        .begin_cutoff(ts(1_000))
        .expect("cutoff accepted");
    assert_eq!(after.phase, EpochPhase::Cutoff);
    assert_eq!(after.intake_epoch, before.intake_epoch);
    assert!(!after.is_accepting_requests());
    assert_eq!(after.intake_epoch_id(), None);
    assert!(after.check_invariants());
}

#[test]
fn snapshot_binds_cutoff_sequence_asof_nav_and_supply() {
    let state = cutoff_state();
    let snapshot = state
        .build_settlement_snapshot(
            &report(7, 1_000),
            1_000_000,
            100_000,
            settle_now(),
            MAX_AGE,
        )
        .expect("fresh report accepted");
    assert_eq!(snapshot.epoch_id(), EpochId::FIRST_SETTLEMENT);
    assert_eq!(snapshot.report_seq(), 7);
    assert_eq!(snapshot.report_hash(), &[7u8; 32]);
    assert_eq!(snapshot.as_of_ns(), ts(1_000));
    assert_eq!(snapshot.cutoff_ns(), ts(1_000));
    assert_eq!(snapshot.settlement_nav(), 1_000_000);
    assert_eq!(snapshot.eligible_supply(), 100_000);
    assert!(snapshot.covers_cutoff(ts(1_000)));
    assert!(snapshot.is_fresh_at(settle_now(), MAX_AGE));
}

#[test]
fn settlement_rejects_reports_violating_the_law() {
    let state = cutoff_state();
    assert_eq!(
        state.build_settlement_snapshot(&report(1, 500), 1, 1, settle_now(), MAX_AGE),
        Err(SettlementRejection::ReportBeforeCutoff),
        "as_of before cutoff cannot price the cutoff"
    );
    assert_eq!(
        state.build_settlement_snapshot(&report(1, 2_000), 1, 1, settle_now(), MAX_AGE),
        Err(SettlementRejection::ReportFutureDated),
        "as_of after settle_now is future-dated"
    );
    assert_eq!(
        state.build_settlement_snapshot(&report(1, 1_000), 1, 1, ts(10_000), MAX_AGE),
        Err(SettlementRejection::ReportTooOld),
        "stale covering report rejected by max age"
    );
}

#[test]
fn zero_supply_and_migration_epoch_bind_rejected() {
    let r = report(1, 1_000);
    assert_eq!(
        EpochSnapshot::bind(EpochId::FIRST_SETTLEMENT, ts(1_000), &r, 5, 0),
        Err(SettlementRejection::ZeroEligibleSupply)
    );
    assert_eq!(
        EpochSnapshot::bind(EpochId::MIGRATION_INTAKE, ts(1_000), &r, 5, 10),
        Err(SettlementRejection::InvalidEpoch),
        "migration-intake id must never bind a priced snapshot"
    );
    assert_eq!(
        EpochSnapshot::bind(EpochId::FIRST_SETTLEMENT, ts(2_000), &r, 5, 10),
        Err(SettlementRejection::ReportBeforeCutoff),
        "cutoff after as_of must not bind"
    );
}

#[test]
fn settlement_applies_once_and_reopens_next_epoch() {
    let state = cutoff_state();
    let snapshot = state
        .build_settlement_snapshot(&report(3, 1_200), 500_000, 100, settle_now(), MAX_AGE)
        .expect("fresh covering report");
    let advanced = state.apply_settled(&snapshot).expect("settlement applied");
    assert_eq!(advanced.intake_epoch, EpochId::FIRST_SETTLEMENT.checked_next().unwrap());
    assert_eq!(advanced.phase, EpochPhase::Open);
    assert_eq!(advanced.cutoff_ns, None);
    assert_eq!(advanced.last_settled, Some(snapshot));
    assert!(advanced.check_invariants());

    // The bound snapshot cannot be replayed into a later cutoff epoch at a
    // non-monotonic sequence.
    let next_cutoff = advanced
        .begin_cutoff(ts(5_000))
        .expect("next cutoff opens");
    let replay = EpochSnapshot::bind(
        next_cutoff.intake_epoch,
        ts(5_000),
        &report(3, 5_000),
        1,
        10,
    )
    .expect("snapshot constructed for replay attempt");
    assert_eq!(
        next_cutoff.apply_settled(&replay),
        Err(SettlementRejection::ReportSequenceNonMonotonic)
    );
}

#[test]
fn settled_claims_recompute_from_bound_snapshot() {
    let state = cutoff_state();
    let snapshot = state
        .build_settlement_snapshot(
            &report(9, 1_000),
            1_000_000,
            1_000_000,
            settle_now(),
            MAX_AGE,
        )
        .expect("settlement bound");
    let advanced = state.apply_settled(&snapshot).expect("settled");

    // Floor division, never a stored per-request figure.
    assert_eq!(advanced.settled_claim_for(EpochId::new(2), 7), None);
    assert_eq!(
        advanced.settled_claim_for(EpochId::FIRST_SETTLEMENT, 123_456),
        Some(123_456)
    );
    assert_eq!(advanced.settled_claim_for(EpochId::FIRST_SETTLEMENT, 0), Some(0));
}

#[test]
fn migration_intake_requests_price_only_at_first_settlement() {
    let advanced = cutoff_state()
        .build_settlement_snapshot(
            &report(4, 1_000),
            1_000_000,
            1_000_000,
            settle_now(),
            MAX_AGE,
        )
        .and_then(|s| cutoff_state().apply_settled(&s))
        .expect("settled");

    // A migrated (epoch-0) request may settle against the first settled epoch.
    assert_eq!(
        advanced.settled_claim_for(EpochId::MIGRATION_INTAKE, 100),
        Some(100)
    );
    // Before any settlement, migrated requests have no claim at all.
    let pre = cutoff_state();
    assert_eq!(
        pre.settled_claim_for(EpochId::MIGRATION_INTAKE, 100),
        None,
        "no accepted report means no materialized claim"
    );
}

#[test]
fn claims_use_checked_wide_arithmetic() {
    // nav = u128::MAX, supply = 1: exact intermediate values exceed u128 and
    // the claim must surface None rather than wrap.
    let snap = EpochSnapshot::bind(
        EpochId::FIRST_SETTLEMENT,
        ts(10),
        &report(1, 10),
        u128::MAX,
        1,
    )
    .expect("bind");
    assert_eq!(snap.claim_for(2), None);
    // nav = u128::MAX, supply = 2, shares = 2: exact quotient fits u128.
    let snap2 = EpochSnapshot::bind(
        EpochId::FIRST_SETTLEMENT,
        ts(10),
        &report(1, 10),
        u128::MAX,
        2,
    )
    .expect("bind");
    assert_eq!(snap2.claim_for(2), Some(u128::MAX));
    // Rounding always floors.
    assert_eq!(snap2.claim_for(1), Some(u128::MAX / 2));
}

#[test]
fn snapshots_have_no_mutators() {
    let snap = EpochSnapshot::bind(
        EpochId::FIRST_SETTLEMENT,
        ts(10),
        &report(1, 10),
        500,
        1_000,
    )
    .expect("bind");
    let before = snap.clone();
    // No &mut self methods exist; only the binding constructor and immutable
    // accessors are exposed. This compile-time property is the invariant:
    // re-reading through every accessor after any use yields identical bytes.
    assert_eq!(snap.epoch_id(), before.epoch_id());
    assert_eq!(snap.report_seq(), before.report_seq());
    assert_eq!(snap.report_hash(), before.report_hash());
    assert_eq!(snap.as_of_ns(), before.as_of_ns());
    assert_eq!(snap.settlement_nav(), before.settlement_nav());
    assert_eq!(snap.eligible_supply(), before.eligible_supply());
    assert_eq!(snap.cutoff_ns(), before.cutoff_ns());
    assert_eq!(snap.claim_for(400), Some(200));
}

#[test]
fn stalled_detection_and_invariant_rejects() {
    let state = cutoff_state();
    assert!(state.is_settlement_stalled(ts(3_000), MAX_AGE));
    assert!(!state.is_settlement_stalled(ts(2_500), MAX_AGE));

    let mut broken = state.clone();
    broken.cutoff_ns = None;
    assert!(!broken.check_invariants());
    broken.phase = EpochPhase::Open;
    assert!(broken.check_invariants());

    let mut bogus = EpochState::genesis();
    bogus.intake_epoch = EpochId::MIGRATION_INTAKE;
    assert!(!bogus.check_invariants());
}

#[test]
fn settlement_requires_cutoff_phase() {
    let open = EpochState::genesis();
    assert_eq!(
        open.build_settlement_snapshot(&report(1, 1_000), 1, 1, settle_now(), MAX_AGE),
        Err(SettlementRejection::PhaseNotCutoff)
    );
    let snap = EpochSnapshot::bind(EpochId::FIRST_SETTLEMENT, ts(10), &report(1, 10), 1, 1)
        .expect("bind");
    assert_eq!(
        open.apply_settled(&snap),
        Err(SettlementRejection::PhaseNotCutoff)
    );
}

#[test]
fn epoch_snapshots_are_immutable_and_monotonic() {
    let cutoff = cutoff_state();
    let snapshot = cutoff
        .build_settlement_snapshot(
            &report(1, 1_000),
            1_000_000,
            1_000_000,
            settle_now(),
            MAX_AGE,
        )
        .expect("settlement");
    let settled = cutoff.apply_settled(&snapshot).expect("settled");

    // Replaying the same settlement into the next cutoff is rejected:
    // report sequence must strictly advance across settlements.
    let next_cutoff = settled.begin_cutoff(ts(6_000)).expect("next cutoff");
    let replay = EpochSnapshot::bind(
        next_cutoff.intake_epoch,
        ts(6_000),
        &report(1, 6_000),
        9_000_000,
        1_000_000,
    )
    .expect("replay snapshot constructed");
    assert_eq!(
        next_cutoff.apply_settled(&replay),
        Err(SettlementRejection::ReportSequenceNonMonotonic)
    );

    // The bound snapshot still describes the original settlement exactly;
    // nothing about it changed when the epoch advanced.
    let bound = settled.last_settled.expect("snapshot present");
    assert_eq!(bound.epoch_id(), EpochId::FIRST_SETTLEMENT);
    assert_eq!(bound.report_seq(), 1);
    assert_eq!(bound.as_of_ns(), ts(1_000));
    assert_eq!(bound.settlement_nav(), 1_000_000);
    assert_eq!(bound.eligible_supply(), 1_000_000);
    assert_eq!(bound.cutoff_ns(), ts(1_000));
    assert_eq!(bound.claim_for(123_456), Some(123_456));
}

#[test]
fn zero_nav_snapshots_pay_nothing_and_only_refund() {
    let cutoff = cutoff_state();
    let snapshot = cutoff
        .build_settlement_snapshot(&report(1, 1_000), 0, 1_000_000, settle_now(), MAX_AGE)
        .expect("zero-nav settlement binds");
    assert_eq!(snapshot.claim_for(123_456), Some(0));
    let settled = cutoff.apply_settled(&snapshot).expect("settled");
    assert_eq!(settled.settled_claim_for(EpochId::FIRST_SETTLEMENT, 500), Some(0));
}

#[test]
fn claim_rounding_floors_never_rounds_up() {
    let snap = EpochSnapshot::bind(
        EpochId::FIRST_SETTLEMENT,
        ts(10),
        &report(1, 10),
        1_000_000,
        3,
    )
    .expect("bind");
    // 1 * 1_000_000 / 3 and 2 * 1_000_000 / 3 must floor.
    assert_eq!(snap.claim_for(1), Some(333_333));
    assert_eq!(snap.claim_for(2), Some(666_666));
    assert_eq!(snap.claim_for(3), Some(1_000_000));
}
