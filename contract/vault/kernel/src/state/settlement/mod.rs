//! Chain-agnostic epoch settlement primitives and immutable snapshot state.
//!
//! This module implements the ENG-697 fixed settlement model:
//!
//! - Epoch identifiers are monotonic `u64` counters.
//! - Requests are priced only when an epoch settles against a fresh, accepted
//!   custodial valuation report satisfying
//!   `cutoff <= report.as_of <= settle_now` and
//!   `settle_now - report.as_of <= max_age`.
//! - Settlement output is an immutable [`EpochSnapshot`] binding report
//!   sequence, report hash, valuation time, settlement NAV, eligible supply,
//!   and the epoch cutoff. Claims are never persisted per request; execution
//!   recomputes `floor(escrow_shares * settlement_nav / eligible_supply)`
//!   from the bound snapshot.
//! - No pre-settlement asset figure exists for any queued request.
//!
//! All functions here are pure constructors or invariant checks. Runtime
//! orchestration (admission, payout execution, storage paging) lives outside
//! the state layer.

use crate::math::number::Number;
use crate::types::TimestampNs;

#[cfg(feature = "borsh-schema")]
use alloc::string::ToString;

/// Migration-intake epoch marker. Requests produced by the legacy migration
/// are bound to this epoch and carry no fixed asset claim. Epoch `0` is never
/// a settlement epoch; migrated requests settle at the first settled epoch
/// (>= [`EpochId::FIRST_SETTLEMENT`]) from an accepted snapshot.
pub const MIGRATION_INTAKE_EPOCH: u64 = 0;

/// The first epoch that can be settled and priced.
pub const FIRST_SETTLEMENT_EPOCH: u64 = 1;

/// Monotonic epoch identifier.
#[repr(transparent)]
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde)]
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EpochId(pub u64);

impl EpochId {
    /// Migration-intake marker (never a settlement epoch).
    pub const MIGRATION_INTAKE: Self = Self(MIGRATION_INTAKE_EPOCH);
    /// The first settleable epoch.
    pub const FIRST_SETTLEMENT: Self = Self(FIRST_SETTLEMENT_EPOCH);

    /// Wrap a raw epoch counter.
    #[inline]
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the raw counter value.
    #[inline]
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// True when this id can host new intake or settlement (never the
    /// migration-intake marker).
    #[inline]
    #[must_use]
    pub const fn is_settlement_epoch(self) -> bool {
        self.0 >= FIRST_SETTLEMENT_EPOCH
    }

    /// Checked successor for monotonic epoch advancement.
    #[inline]
    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }
}

/// Lifecycle phase of an epoch.
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum EpochPhase {
    /// Intake is open: new unpriced requests may bind to this epoch.
    #[default]
    Open,
    /// Intake closed at a cutoff; settlement is pending an accepted report.
    Cutoff,
    /// The epoch has settled; an immutable snapshot is bound to it.
    Settled,
}

/// Rejection classification for epoch settlement construction and transitions.
///
/// The runtime lane maps these variants to typed kernel errors; the state
/// layer never aborts on untrusted input.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettlementRejection {
    /// Epoch state is not in [`EpochPhase::Cutoff`].
    PhaseNotCutoff,
    /// Cutoff timestamp missing while in [`EpochPhase::Cutoff`].
    CutoffUnset,
    /// Report valuation time precedes the epoch cutoff.
    ReportBeforeCutoff,
    /// Report valuation time is in the future relative to settlement time.
    ReportFutureDated,
    /// Report is older than the allowed maximum age at settlement time.
    ReportTooOld,
    /// Report sequence does not exceed the last settled report sequence.
    ReportSequenceNonMonotonic,
    /// Epoch already settled or id does not match the intake epoch.
    EpochNotSettleable,
    /// Epoch counter exhausted.
    EpochIdExhausted,
    /// Epoch id is not a settlement epoch (migration marker used wrongly).
    InvalidEpoch,
    /// Eligible supply must be positive for a priced settlement.
    ZeroEligibleSupply,
    /// Snapshot does not correspond to the epoch state being advanced.
    SnapshotMismatch,
    /// Request epoch does not match the settlement snapshot epoch.
    EpochMismatch,
}

/// Reference to an accepted custodial valuation report.
///
/// This is a binding reference only (sequence, hash, valuation time). The
/// signed report envelope itself lives in the custodial adapter; the vault
/// never accepts report payloads or asset claims from callers.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ValuationReportRef {
    /// Monotonically increasing report sequence for the custodial route.
    pub report_seq: u64,
    /// Valuation time asserted by the signed report (not submission time).
    pub as_of_ns: TimestampNs,
    /// Digest binding the signed report envelope bytes.
    pub report_hash: [u8; 32],
}

/// Immutable settled epoch snapshot.
///
/// A snapshot binds the accepted report's sequence, hash, and valuation time
/// to the settlement NAV, eligible supply, and cutoff of exactly one epoch.
/// It is constructed only through [`EpochSnapshot::bind`], which validates
/// the settlement law, and exposes no mutators. All payout math must
/// recompute claims from this binding at execution time.
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct EpochSnapshot {
    epoch_id: EpochId,
    report_seq: u64,
    report_hash: [u8; 32],
    as_of_ns: TimestampNs,
    settlement_nav: u128,
    eligible_supply: u128,
    cutoff_ns: TimestampNs,
}

impl EpochSnapshot {
    /// Validate the settlement law and bind an immutable snapshot.
    ///
    /// Returns [`SettlementRejection`] when the epoch is not a settlement
    /// epoch, the eligible supply is zero, or the report does not cover the
    /// cutoff (`cutoff <= as_of`).
    #[must_use = "return value binds or validates settlement law; ignoring it can admit an unpriced claim"]
    pub fn bind(
        epoch_id: EpochId,
        cutoff_ns: TimestampNs,
        report: &ValuationReportRef,
        settlement_nav: u128,
        eligible_supply: u128,
    ) -> Result<Self, SettlementRejection> {
        if !epoch_id.is_settlement_epoch() {
            return Err(SettlementRejection::InvalidEpoch);
        }
        if report.report_seq == 0 {
            return Err(SettlementRejection::ReportSequenceNonMonotonic);
        }
        if eligible_supply == 0 {
            return Err(SettlementRejection::ZeroEligibleSupply);
        }
        if cutoff_ns > report.as_of_ns {
            return Err(SettlementRejection::ReportBeforeCutoff);
        }
        Ok(Self {
            epoch_id,
            report_seq: report.report_seq,
            report_hash: report.report_hash,
            as_of_ns: report.as_of_ns,
            settlement_nav,
            eligible_supply,
            cutoff_ns,
        })
    }

    /// Epoch this snapshot settles.
    #[inline]
    #[must_use]
    pub const fn epoch_id(&self) -> EpochId {
        self.epoch_id
    }

    /// Sequence of the accepted report backing this settlement.
    #[inline]
    #[must_use]
    pub const fn report_seq(&self) -> u64 {
        self.report_seq
    }

    /// Digest of the accepted report backing this settlement.
    #[inline]
    #[must_use]
    pub const fn report_hash(&self) -> &[u8; 32] {
        &self.report_hash
    }

    /// Valuation time bound into this settlement.
    #[inline]
    #[must_use]
    pub const fn as_of_ns(&self) -> TimestampNs {
        self.as_of_ns
    }

    /// Settlement NAV bound into this settlement.
    #[inline]
    #[must_use]
    pub const fn settlement_nav(&self) -> u128 {
        self.settlement_nav
    }

    /// Eligible supply bound into this settlement.
    #[inline]
    #[must_use]
    pub const fn eligible_supply(&self) -> u128 {
        self.eligible_supply
    }

    /// Cutoff bound into this settlement.
    #[inline]
    #[must_use]
    pub const fn cutoff_ns(&self) -> TimestampNs {
        self.cutoff_ns
    }

    /// True if this snapshot's cutoff and valuation time cover `cut_at_ns`
    /// (`cutoff <= cut_at_ns <= as_of` is implied by the binding law).
    #[inline]
    #[must_use]
    pub fn covers_cutoff(&self, cut_at_ns: TimestampNs) -> bool {
        self.cutoff_ns <= cut_at_ns && cut_at_ns <= self.as_of_ns
    }

    /// Freshness test for settlement use: `as_of <= settle_now` and
    /// `settle_now - as_of <= max_age_ns`.
    #[must_use]
    pub fn is_fresh_at(&self, settle_now_ns: TimestampNs, max_age_ns: u64) -> bool {
        if self.as_of_ns > settle_now_ns {
            return false;
        }
        settle_now_ns
            .as_u64()
            .checked_sub(self.as_of_ns.as_u64())
            .is_some_and(|age| age <= max_age_ns)
    }

    /// Execution-time claim: `floor(escrow_shares * settlement_nav /
    /// eligible_supply)` using checked wide arithmetic.
    ///
    /// Returns `None` only when the exact product/quotient exceeds `u128`.
    /// Zero supply is impossible for bound snapshots (construction rejects
    /// it), and is handled defensively here.
    #[must_use]
    pub fn claim_for(&self, escrow_shares: u128) -> Option<u128> {
        if self.eligible_supply == 0 {
            return None;
        }
        let claim = Number::mul_div_floor(
            Number::from(escrow_shares),
            Number::from(self.settlement_nav),
            Number::from(self.eligible_supply),
        );
        if claim > Number::from(u128::MAX) {
            return None;
        }
        Some(claim.as_u128_trunc())
    }
}

/// Current epoch lifecycle state for a vault.
///
/// # Invariants
///
/// - `intake_epoch` is monotonically increasing across settlements.
/// - A settlement snapshot, once bound, is immutable and only advances
///   `last_settled_epoch`.
/// - New intake binds to `intake_epoch` only while [`EpochPhase::Open`].
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct EpochState {
    /// Lifecycle phase of the current intake epoch.
    pub phase: EpochPhase,
    /// Epoch that new unpriced requests bind to while intake is open.
    pub intake_epoch: EpochId,
    /// Cutoff bound while in [`EpochPhase::Cutoff`].
    pub cutoff_ns: Option<TimestampNs>,
    /// Last bound settlement snapshot (immutable once set).
    pub last_settled: Option<EpochSnapshot>,
}

impl EpochState {
    /// Genesis state: intake is open at the first settlement epoch.
    #[must_use]
    pub const fn genesis() -> Self {
        Self {
            phase: EpochPhase::Open,
            intake_epoch: EpochId::FIRST_SETTLEMENT,
            cutoff_ns: None,
            last_settled: None,
        }
    }

    /// True when new intake may bind to the current epoch.
    #[inline]
    #[must_use]
    pub fn is_accepting_requests(&self) -> bool {
        self.phase == EpochPhase::Open && self.intake_epoch.is_settlement_epoch()
    }

    /// Epoch that new unpriced requests must bind to, if intake is open.
    #[inline]
    #[must_use]
    pub fn intake_epoch_id(&self) -> Option<EpochId> {
        if self.is_accepting_requests() {
            Some(self.intake_epoch)
        } else {
            None
        }
    }

    /// Check whether a cutoff transition is valid for this state.
    #[must_use]
    pub fn can_begin_cutoff(&self, cutoff_ns: TimestampNs) -> bool {
        if self.phase != EpochPhase::Open {
            return false;
        }
        if let Some(prev) = self.cutoff_ns {
            if cutoff_ns < prev {
                return false;
            }
        }
        true
    }

    /// Close intake at `cutoff_ns` for the current epoch.
    #[must_use = "return value binds or validates settlement law; ignoring it can admit an unpriced claim"]
    pub fn begin_cutoff(&self, cutoff_ns: TimestampNs) -> Result<Self, SettlementRejection> {
        if self.phase != EpochPhase::Open {
            return Err(SettlementRejection::PhaseNotCutoff);
        }
        if let Some(prev) = self.cutoff_ns {
            if cutoff_ns < prev {
                return Err(SettlementRejection::CutoffUnset);
            }
        }
        Ok(Self {
            phase: EpochPhase::Cutoff,
            intake_epoch: self.intake_epoch,
            cutoff_ns: Some(cutoff_ns),
            last_settled: self.last_settled.clone(),
        })
    }

    /// Validate an accepted report reference and settlement outcome against
    /// this epoch state, returning the immutable snapshot or a typed
    /// rejection. Never mutates state.
    #[must_use = "return value binds or validates settlement law; ignoring it can admit an unpriced claim"]
    pub fn build_settlement_snapshot(
        &self,
        report: &ValuationReportRef,
        settlement_nav: u128,
        eligible_supply: u128,
        settle_now_ns: TimestampNs,
        max_age_ns: u64,
    ) -> Result<EpochSnapshot, SettlementRejection> {
        if self.phase != EpochPhase::Cutoff {
            return Err(SettlementRejection::PhaseNotCutoff);
        }
        let cutoff = self.cutoff_ns.ok_or(SettlementRejection::CutoffUnset)?;
        if report.as_of_ns < cutoff {
            return Err(SettlementRejection::ReportBeforeCutoff);
        }
        if report.as_of_ns > settle_now_ns {
            return Err(SettlementRejection::ReportFutureDated);
        }
        if settle_now_ns
            .as_u64()
            .checked_sub(report.as_of_ns.as_u64())
            .is_none_or(|age| age > max_age_ns)
        {
            return Err(SettlementRejection::ReportTooOld);
        }
        if let Some(prev) = &self.last_settled {
            if report.report_seq <= prev.report_seq() {
                return Err(SettlementRejection::ReportSequenceNonMonotonic);
            }
        }
        let snapshot = EpochSnapshot::bind(
            self.intake_epoch,
            cutoff,
            report,
            settlement_nav,
            eligible_supply,
        )?;
        if snapshot.cutoff_ns() != cutoff {
            return Err(SettlementRejection::SnapshotMismatch);
        }
        Ok(snapshot)
    }

    /// Apply a previously built snapshot: records the settlement, reopens
    /// intake at the next monotonic epoch. Pure; the caller persists the
    /// returned state only after side effects are ordered.
    #[must_use = "return value binds or validates settlement law; ignoring it can admit an unpriced claim"]
    pub fn apply_settled(&self, snapshot: &EpochSnapshot) -> Result<Self, SettlementRejection> {
        if self.phase != EpochPhase::Cutoff {
            return Err(SettlementRejection::PhaseNotCutoff);
        }
        if snapshot.epoch_id() != self.intake_epoch {
            return Err(SettlementRejection::EpochMismatch);
        }
        if self.cutoff_ns != Some(snapshot.cutoff_ns()) {
            return Err(SettlementRejection::SnapshotMismatch);
        }
        if let Some(prev) = &self.last_settled {
            if snapshot.report_seq() <= prev.report_seq() {
                return Err(SettlementRejection::ReportSequenceNonMonotonic);
            }
        }
        let next_epoch = self
            .intake_epoch
            .checked_next()
            .ok_or(SettlementRejection::EpochIdExhausted)?;
        Ok(Self {
            phase: EpochPhase::Open,
            intake_epoch: next_epoch,
            cutoff_ns: None,
            last_settled: Some(snapshot.clone()),
        })
    }

    /// True when cutoff has passed without a fresh accepted report and the
    /// epoch is stale at `settle_now_ns`.
    #[must_use]
    pub fn is_settlement_stalled(
        &self,
        settle_now_ns: TimestampNs,
        max_age_ns: u64,
    ) -> bool {
        if self.phase != EpochPhase::Cutoff {
            return false;
        }
        let Some(cutoff) = self.cutoff_ns else {
            return true;
        };
        cutoff
            .as_u64()
            .checked_add(max_age_ns)
            .is_some_and(|deadline| settle_now_ns >= TimestampNs::from_nanos(deadline))
    }

    /// Recompute the execution-time claim for `escrow_shares` against the
    /// last settled snapshot, enforcing epoch correspondence:
    ///
    /// - Requests bound to [`EpochId::MIGRATION_INTAKE`] settle at the first
    ///   settled snapshot (epoch >= [`EpochId::FIRST_SETTLEMENT`]).
    /// - Requests bound to a settlement epoch settle only against the
    ///   snapshot for that exact epoch.
    ///
    /// Pre-settlement requests have no persisted claim; this is the only
    /// path from shares to assets.
    #[must_use]
    pub fn settled_claim_for(&self, request_epoch: EpochId, escrow_shares: u128) -> Option<u128> {
        let snapshot = self.last_settled.as_ref()?;
        let request_matches = if request_epoch == EpochId::MIGRATION_INTAKE {
            snapshot.epoch_id().is_settlement_epoch()
        } else {
            request_epoch == snapshot.epoch_id()
        };
        if !request_matches {
            return None;
        }
        snapshot.claim_for(escrow_shares)
    }

    /// Validate stored epoch state against the settlement law.
    #[must_use]
    pub fn check_invariants(&self) -> bool {
        if self.intake_epoch < EpochId::FIRST_SETTLEMENT {
            return false;
        }
        match self.phase {
            EpochPhase::Open => {
                if self.cutoff_ns.is_some() {
                    return false;
                }
            }
            EpochPhase::Cutoff => {
                if self.cutoff_ns.is_none() {
                    return false;
                }
            }
            EpochPhase::Settled => {}
        }
        if let Some(snapshot) = &self.last_settled {
            if !snapshot.epoch_id().is_settlement_epoch() {
                return false;
            }
            if snapshot.report_seq() == 0 || snapshot.eligible_supply() == 0 {
                return false;
            }
            if snapshot.cutoff_ns() > snapshot.as_of_ns() {
                return false;
            }
            // A bound snapshot never regresses below the intake epoch.
            if snapshot.epoch_id() >= self.intake_epoch && self.phase != EpochPhase::Settled {
                return false;
            }
        }
        true
    }
}


/// Rejection of an unpriced request record. New requests must escrow real
/// shares and bind to a settlement-eligible epoch; deposits must carry real
/// assets. No request ever carries a pre-settlement asset claim.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    /// Withdrawal escrow must cover at least one share.
    ZeroEscrowShares,
    /// Deposit liability must cover at least one asset unit.
    ZeroDepositAssets,
    /// Epoch id is not settlement-eligible for new intake.
    InvalidEpoch,
}
#[cfg(all(test, feature = "action-epoch-settlement"))]
mod tests;
