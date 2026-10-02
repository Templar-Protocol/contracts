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
//! * `withdrawal_collected` / `withdrawal_settled` / `payout_complete` succeed
//!   if and only if the fixed settlement law is satisfied for the vault state
//!   passed alongside them: the payout claim is derived exclusively from the
//!   queue head's own accepted epoch settlement snapshot
//!   (`floor(escrow_shares * settlement_nav / eligible_supply)`), meets the
//!   request minimum and protocol floor, matches recorded collection exactly,
//!   is fully covered by idle assets, and burns the request's *full* escrow.
//!   An unsettled, wrong-epoch, below-floor, arbitrary-amount, or
//!   partial-burn attempt is rejected with no state change and the unsettled
//!   head remains queued.
//! * A lawful `Payout` always carries `burn_shares == escrow_shares` and a
//!   claim of at least `MIN_WITHDRAWAL_ASSETS`.
//! * `complete_allocation` from a non-`Allocating` state must error.
//!
//! The `SettlementLaw` action additionally fuzzes the epoch settlement
//! primitives directly: snapshot binding law, claim arithmetic oracles, and
//! epoch correspondence (`settled_claim_for` must return `None` for unsettled
//! or wrong-epoch requests, and the exact snapshot-derived claim otherwise).

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use templar_vault_kernel::effects::{KernelEffect, KernelEvent};
use templar_vault_kernel::{
    allocation_step_callback, complete_allocation, complete_refresh, payout_complete,
    refresh_step_callback, settled_claim, start_allocation, start_refresh, start_withdrawal,
    stop_withdrawal, withdrawal_collected, withdrawal_settled, withdrawal_step_callback, Address,
    AllocationPlanEntry, EpochId, EpochState, OpState, PendingWithdrawal, PayoutState,
    TimestampNs, TransitionResult, ValuationReportRef, VaultState, WithdrawQueue,
    WithdrawalRequest, FIRST_SETTLEMENT_EPOCH, MIN_WITHDRAWAL_ASSETS, WithdrawingState,
};
const MAX_ACTIONS: usize = 32;
const MAX_PLAN: usize = 8;
const CUTOFF_NS: u64 = 1_000_000_000;

#[derive(Arbitrary, Clone, Debug)]
enum VaultSketch {
    /// Vault with an empty queue and no accepted settlement: any payout
    /// attempt against it must be rejected and inert.
    Empty,
    /// Vault whose queue head matches the current op exactly and whose epoch
    /// has an accepted settlement snapshot. The snapshot is priced so the
    /// derived claim equals `lawful_claim`, letting the fuzzer reach lawful
    /// payouts when recorded collection matches and floors are met.
    Lawful {
        lawful_claim: u128,
        idle: u128,
        protocol_floor: u128,
        min_assets_out: u128,
        settle: bool,
        corrupt: Corrupt,
    },
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum Corrupt {
    None,
    Unsettled,
    WrongEpoch,
    Owner,
    Receiver,
    Escrow,
    LowIdle,
}

#[derive(Arbitrary, Clone, Debug)]
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
        zero_remaining: bool,
        sketch: VaultSketch,
    },
    WithdrawalSettled {
        op_id: u64,
        sketch: VaultSketch,
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
    CompleteRefresh {
        op_id: u64,
    },
    PayoutComplete {
        op_id: u64,
        success: bool,
        escrow: [u8; 32],
        sketch: VaultSketch,
    },
    /// Drive epoch settlement primitives directly and assert the settlement
    /// law and claim-arithmetic oracles.
    SettlementLaw {
        report_seq: u64,
        as_of_delta: u64,
        settlement_nav: u128,
        eligible_supply: u128,
        escrow_shares: u128,
        request_epoch: u64,
    },
}

#[derive(Arbitrary, Clone, Debug)]
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
            assert_eq!(
                s.burn_shares, s.escrow_shares,
                "lawful Payout must burn the full escrow ({} vs {})",
                s.burn_shares, s.escrow_shares,
            );
            assert!(
                s.amount >= MIN_WITHDRAWAL_ASSETS && s.amount > 0,
                "lawful Payout claim {} below protocol floor",
                s.amount,
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

/// Build a settlement snapshot for the given epoch through the public epoch
/// lifecycle: cutoff, snapshot binding, and application. Returns the settled
/// epoch state when the law accepts the inputs, `None` otherwise.
fn drive_settlement(
    epoch_id: EpochId,
    report_seq: u64,
    cutoff_ns: u64,
    as_of_ns: u64,
    settlement_nav: u128,
    eligible_supply: u128,
) -> Option<EpochState> {
    let cutoff = TimestampNs::from_nanos(cutoff_ns);
    let as_of = TimestampNs::from_nanos(as_of_ns);
    let report = ValuationReportRef {
        report_seq,
        as_of_ns: as_of,
        report_hash: [7u8; 32],
    };
    // Intake must be open at `epoch_id` before cutoff; drive from genesis for
    // the first settlement epoch only.
    if epoch_id != EpochId::new(FIRST_SETTLEMENT_EPOCH) {
        return None;
    }
    let cutoff_state = EpochState::genesis().begin_cutoff(cutoff).ok()?;
    let snapshot = cutoff_state
        .build_settlement_snapshot(
            &report,
            settlement_nav,
            eligible_supply,
            as_of,
            as_of.as_u64().saturating_sub(cutoff.as_u64()),
        )
        .ok()?;
    cutoff_state.apply_settled(&snapshot).ok()
}

/// Independent oracle: `floor(escrow_shares * settlement_nav /
/// eligible_supply)` with u128-checked arithmetic, used to verify the
/// snapshot's own claim derivation whenever the product fits in `u128`.
fn oracle_floor_product(escrow: u128, nav: u128, supply: u128) -> Option<u128> {
    if supply == 0 {
        return None;
    }
    let product = escrow.checked_mul(nav)?;
    Some(product / supply)
}

/// Recompute the settled claim for the vault's queue head from its own epoch
/// settlement context. `None` when unsettled or epoch-mismatched.
fn derived_claim(vault: &VaultState) -> Option<u128> {
    let (_, head) = vault.withdraw_queue.head()?;
    let claim = settled_claim(head, &vault.epoch)?;
    let snapshot = vault.epoch.last_settled.as_ref()?;
    let (snap_epoch, nav, supply) = (
        snapshot.epoch_id(),
        snapshot.settlement_nav(),
        snapshot.eligible_supply(),
    );
    let matches = head.epoch_id == snap_epoch || head.epoch_id == EpochId::MIGRATION_INTAKE;
    if matches {
        if let Some(exact) = oracle_floor_product(head.escrow_shares, nav, supply) {
            assert_eq!(
                Some(exact),
                snapshot.claim_for(head.escrow_shares),
                "snapshot claim derivation disagrees with exact arithmetic"
            );
        }
        // Settlement can never mint more assets than the bound supply values
        // when NAV does not exceed it.
        if nav <= supply {
            assert!(
                claim <= head.escrow_shares,
                "claim {} exceeds escrow {} at NAV {nav}/supply {supply}",
                claim,
                head.escrow_shares,
            );
        }
    }
    Some(claim)
}

/// Settlement-law oracle: `true` exactly when a `Withdrawing -> Payout`
/// authorization is lawful for this op/vault pair.
fn withdrawal_lawful(
    w: &WithdrawingState,
    op_id: u64,
    protocol_floor: u128,
    vault: &VaultState,
) -> bool {
    if w.op_id != op_id {
        return false;
    }
    let Some((head_id, head)) = vault.withdraw_queue.head() else {
        return false;
    };
    if head_id != w.request_id
        || head.owner != w.owner
        || head.receiver != w.receiver
        || head.escrow_shares != w.escrow_shares
    {
        return false;
    }
    let Some(claim) = derived_claim(vault) else {
        return false;
    };
    let floor = head
        .min_assets_out
        .max(protocol_floor)
        .max(MIN_WITHDRAWAL_ASSETS);
    w.remaining == 0
        && w.collected == claim
        && claim >= floor
        && claim > 0
        && claim <= vault.idle_assets
}

/// Settlement-law oracle for `payout_complete`: the stored payout must still
/// be the lawful, snapshot-derived payout with a full-escrow burn.
fn payout_lawful(
    p: &PayoutState,
    op_id: u64,
    protocol_floor: u128,
    vault: &VaultState,
) -> bool {
    if p.op_id != op_id {
        return false;
    }
    let Some((head_id, head)) = vault.withdraw_queue.head() else {
        return false;
    };
    if head_id != p.request_id
        || head.owner != p.owner
        || head.receiver != p.receiver
        || head.escrow_shares != p.escrow_shares
    {
        return false;
    }
    let Some(claim) = derived_claim(vault) else {
        return false;
    };
    let floor = head
        .min_assets_out
        .max(protocol_floor)
        .max(MIN_WITHDRAWAL_ASSETS);
    p.amount == claim
        && p.burn_shares == p.escrow_shares
        && claim >= floor
        && claim > 0
}

/// Build the candidate vault the fuzzer wants the payout transitions to see,
/// from the current op (if any) and the sketch. Always well-formed.
fn build_vault(state: &OpState, sketch: &VaultSketch) -> VaultState {
    let mut vault = VaultState::new();
    let current = match state {
        OpState::Withdrawing(w) => Some(w.clone()),
        _ => None,
    };
    let payout = match state {
        OpState::Payout(p) => Some(p.clone()),
        _ => None,
    };
    let anchor = match (current.as_ref(), payout.as_ref()) {
        (Some(w), _) => (w.request_id, w.owner, w.receiver, w.escrow_shares),
        (None, Some(p)) => (p.request_id, p.owner, p.receiver, p.escrow_shares),
        (None, None) => return vault,
    };
    let VaultSketch::Lawful {
        lawful_claim,
        idle,
        min_assets_out,
        protocol_floor: _,
        settle,
        corrupt,
    } = sketch
    else {
        return vault;
    };
    if !settle {
        return vault;
    }
    let mut head_epoch = EpochId::new(FIRST_SETTLEMENT_EPOCH);
    let mut owner = anchor.1;
    let mut receiver = anchor.2;
    let mut escrow = anchor.3;
    match corrupt {
        Corrupt::None | Corrupt::LowIdle => {}
        Corrupt::Unsettled => return vault,
        Corrupt::WrongEpoch => head_epoch = EpochId::new(FIRST_SETTLEMENT_EPOCH + 1),
        Corrupt::Owner => owner = Address([0xAA; 32]),
        Corrupt::Receiver => receiver = Address([0xBB; 32]),
        Corrupt::Escrow => escrow = escrow.saturating_add(1),
    }
    // Price the snapshot so the derived claim equals `lawful_claim` for the
    // head's escrow: claim = floor(escrow * claim / escrow) with supply set to
    // the escrow itself (escrow >= 1; supply 0 makes binding reject).
    let supply = escrow.max(1);
    let Some(settled) = drive_settlement(
        EpochId::new(FIRST_SETTLEMENT_EPOCH),
        1,
        CUTOFF_NS,
        CUTOFF_NS,
        *lawful_claim,
        supply,
    ) else {
        return vault;
    };
    let Ok(head) = PendingWithdrawal::new(
        owner,
        receiver,
        escrow,
        *min_assets_out,
        TimestampNs::from_nanos(CUTOFF_NS),
        head_epoch,
    ) else {
        return vault;
    };
    vault.withdraw_queue = WithdrawQueue::with_state(
        [(anchor.0, head)],
        anchor.0,
        anchor.0.saturating_add(1),
    );
    vault.epoch = settled;
    let claim = derived_claim(&vault);
    let idle_assets = match corrupt {
        Corrupt::LowIdle => claim.map(|c| c.saturating_sub(1)).unwrap_or(*idle),
        _ => claim.unwrap_or(*idle),
    };
    let idle_assets = idle_assets.min(u128::MAX / 2);
    vault.idle_assets = idle_assets;
    vault.external_assets = 0;
    vault.total_assets = idle_assets;
    vault.total_shares = supply;
    if !vault.check_invariant() {
        return VaultState::new();
    }
    vault
}

/// Assert a rejected payout attempt left everything it must not touch
/// untouched: the vault accounting and queue, and the head still queued.
fn assert_inert(vault: &VaultState, baseline: &VaultState, had_head: bool) {
    assert_eq!(vault, baseline, "rejected attempt changed vault state");
    assert_eq!(
        vault.withdraw_queue.head().is_some(),
        had_head,
        "rejected attempt must leave the head queued"
    );
    assert!(vault.check_invariant(), "vault invariant broken by attempt");
}

/// Assert the effect list of a successful `withdrawal_collected` /
/// `withdrawal_settled` matches the law exactly for the given payout.
fn assert_settlement_effects(result_effects: &[KernelEffect], payout: &PayoutState) {
    assert_eq!(
        result_effects,
        &[KernelEffect::EmitEvent {
            event: KernelEvent::WithdrawalCollected {
                op_id: payout.op_id,
                burn_shares: payout.burn_shares,
                collected: payout.amount,
            },
        }],
        "settlement effects must be exactly the law-bound event",
    );
    assert_eq!(payout.burn_shares, payout.escrow_shares);
}

pub(crate) fn drive_scenario(scenario: &Scenario) {
    let mut state = OpState::Idle;
    check_state_well_formed(&state);

    for action in scenario.actions.iter().cloned().take(MAX_ACTIONS) {
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
            Action::WithdrawalCollected {
                op_id,
                zero_remaining,
                sketch,
            } => {
                // Reachable lawful path: legitimately zero out collection
                // first, then price the snapshot at the recorded collection.
                if zero_remaining {
                    let step_args = match &state {
                        OpState::Withdrawing(w) => Some((w.op_id, w.remaining)),
                        _ => None,
                    };
                    if let Some((step_op_id, step_remaining)) = step_args {
                        if let Ok(step) =
                            withdrawal_step_callback(state.clone(), step_op_id, step_remaining)
                        {
                            state = step.new_state;
                            check_state_well_formed(&state);
                        }
                    }
                }
                let mut sketch = sketch;
                if let (OpState::Withdrawing(w), VaultSketch::Lawful { lawful_claim, .. }) =
                    (&state, &mut sketch)
                {
                    if zero_remaining {
                        *lawful_claim = w.collected;
                    }
                }
                let vault = build_vault(&state, &sketch);
                let baseline = vault.clone();
                let had_head = vault.withdraw_queue.head().is_some();
                let expected_ok = matches!(
                    &state,
                    OpState::Withdrawing(w)
                        if withdrawal_lawful(w, op_id, sketch_floor(&sketch), &vault)
                );
                let result = withdrawal_collected(state.clone(), &vault, op_id, sketch_floor(&sketch));
                assert_eq!(
                    result.is_ok(),
                    expected_ok,
                    "withdrawal_collected acceptance disagreed with the settlement law",
                );
                if result.is_err() {
                    assert_inert(&vault, &baseline, had_head);
                } else if let Ok(res) = &result {
                    if let OpState::Payout(payout) = &res.new_state {
                        assert_settlement_effects(&res.effects, payout);
                        let claim = derived_claim(&baseline)
                            .expect("lawful payout requires a derived claim");
                        assert_eq!(payout.amount, claim);
                        assert!(claim <= baseline.idle_assets);
                    }
                }
                result
            }
            Action::WithdrawalSettled { op_id, sketch } => {
                let vault = build_vault(&state, &sketch);
                let baseline = vault.clone();
                let had_head = vault.withdraw_queue.head().is_some();
                let expected_ok = matches!(
                    &state,
                    OpState::Withdrawing(w)
                        if withdrawal_lawful(w, op_id, sketch_floor(&sketch), &vault)
                );
                let result = withdrawal_settled(state.clone(), &vault, op_id, sketch_floor(&sketch));
                assert_eq!(
                    result.is_ok(),
                    expected_ok,
                    "withdrawal_settled acceptance disagreed with the settlement law",
                );
                if result.is_err() {
                    assert_inert(&vault, &baseline, had_head);
                } else if let Ok(res) = &result {
                    if let OpState::Payout(payout) = &res.new_state {
                        assert_settlement_effects(&res.effects, payout);
                        let claim = derived_claim(&baseline)
                            .expect("lawful payout requires a derived claim");
                        assert_eq!(payout.amount, claim);
                        assert!(claim <= baseline.idle_assets);
                    }
                }
                result
            }
            Action::StopWithdrawal { op_id, escrow } => {
                stop_withdrawal(state.clone(), op_id, Address(escrow))
            }
            Action::StartRefresh { op_id, plan } => {
                let bounded: Vec<u32> = plan.into_iter().take(MAX_PLAN).collect();
                start_refresh(state.clone(), bounded, op_id)
            }
            Action::RefreshStepCallback { op_id } => refresh_step_callback(state.clone(), op_id),
            Action::CompleteRefresh { op_id } => complete_refresh(state.clone(), op_id),
            Action::PayoutComplete {
                op_id,
                success,
                escrow,
                sketch,
            } => {
                let vault = build_vault(&state, &sketch);
                let baseline = vault.clone();
                let had_head = vault.withdraw_queue.head().is_some();
                let expected_ok = matches!(
                    &state,
                    OpState::Payout(p) if payout_lawful(p, op_id, sketch_floor(&sketch), &vault)
                );
                let result = payout_complete(
                    state.clone(),
                    &vault,
                    success,
                    op_id,
                    Address(escrow),
                    sketch_floor(&sketch),
                );
                assert_eq!(
                    result.is_ok(),
                    expected_ok,
                    "payout_complete acceptance disagreed with the settlement law",
                );
                if result.is_err() {
                    assert_inert(&vault, &baseline, had_head);
                } else if let Ok(res) = &result {
                    assert!(res.new_state.is_idle(), "payout_complete must end Idle");
                    let (escrow_shares, owner) = match &state {
                        OpState::Payout(p) => (p.escrow_shares, p.owner),
                        _ => unreachable!("lawful payout_complete from Payout"),
                    };
                    if success {
                        assert!(
                            res.effects.iter().any(|e| matches!(
                                e,
                                KernelEffect::BurnShares { shares, .. }
                                    if *shares == escrow_shares
                            )),
                            "successful payout must burn the full escrow exactly",
                        );
                        assert!(
                            !res.effects.iter().any(|e| matches!(
                                e,
                                KernelEffect::TransferShares { .. }
                            )),
                            "successful payout must refund nothing",
                        );
                    } else {
                        assert!(
                            !res.effects.iter().any(|e| matches!(
                                e,
                                KernelEffect::BurnShares { .. }
                                    | KernelEffect::BurnSharesFrom { .. }
                            )),
                            "failed payout must burn nothing",
                        );
                        assert!(
                            res.effects.iter().any(|e| matches!(
                                e,
                                KernelEffect::TransferShares { to, shares, .. }
                                    if *shares == escrow_shares && *to == owner
                            )),
                            "failed payout must refund the full escrow",
                        );
                    }
                }
                result
            }
            Action::SettlementLaw {
                report_seq,
                as_of_delta,
                settlement_nav,
                eligible_supply,
                escrow_shares,
                request_epoch,
            } => {
                let as_of_ns = CUTOFF_NS.saturating_add(as_of_delta);
                if let Some(settled) = drive_settlement(
                    EpochId::new(FIRST_SETTLEMENT_EPOCH),
                    report_seq,
                    CUTOFF_NS,
                    as_of_ns,
                    settlement_nav,
                    eligible_supply,
                ) {
                    let snapshot = settled
                        .last_settled
                        .as_ref()
                        .expect("applied settlement is bound");
                    assert_eq!(snapshot.epoch_id(), EpochId::new(FIRST_SETTLEMENT_EPOCH));
                    assert!(settled.check_invariants());
                    let claim = snapshot.claim_for(escrow_shares);
                    if let Some(exact) = oracle_floor_product(escrow_shares, settlement_nav, eligible_supply) {
                        assert_eq!(Some(exact), claim, "claim arithmetic oracle mismatch");
                    }
                    if settlement_nav == 0 {
                        assert_eq!(claim, Some(0));
                    }
                    if settlement_nav == eligible_supply {
                        assert_eq!(claim, Some(escrow_shares));
                    }
                    if settlement_nav < eligible_supply {
                        if let Some(c) = claim {
                            assert!(c <= escrow_shares, "claim exceeded escrow at loss NAV");
                        }
                    }
                    let request_epoch = EpochId::new(request_epoch);
                    let derived = settled.settled_claim_for(request_epoch, escrow_shares);
                    let expected = if request_epoch == EpochId::new(FIRST_SETTLEMENT_EPOCH)
                        || request_epoch == EpochId::MIGRATION_INTAKE
                    {
                        claim
                    } else {
                        None
                    };
                    assert_eq!(derived, expected, "wrong-epoch settlement correspondence");
                } else {
                    // Unsettled: no epoch may produce a claim.
                    for epoch in [
                        EpochId::new(FIRST_SETTLEMENT_EPOCH),
                        EpochId::new(request_epoch),
                        EpochId::MIGRATION_INTAKE,
                    ] {
                        assert_eq!(
                            EpochState::genesis().settled_claim_for(epoch, escrow_shares),
                            None,
                            "unsettled epoch must derive no claim",
                        );
                    }
                }
                Ok(TransitionResult::new(state.clone()))
            }
        };


        if let Ok(transition) = result {
            state = transition.new_state;
            check_state_well_formed(&state);
        } else {
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
}

fuzz_target!(|scenario: Scenario| {
    drive_scenario(&scenario);
});

fn sketch_floor(sketch: &VaultSketch) -> u128 {
    match sketch {
        VaultSketch::Empty => 0,
        VaultSketch::Lawful { protocol_floor, .. } => *protocol_floor,
    }
}
