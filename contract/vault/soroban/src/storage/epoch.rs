//! Epoch settlement persistence for the Soroban vault runtime.
//!
//! This module stores the epoch lifecycle law in the versioned storage
//! envelope: epoch phase, the intake epoch new unpriced requests bind to,
//! the cutoff binding, and the immutable last-settled snapshot. Persisting
//! the snapshot is what makes accepted-report replay across settlement
//! impossible: settlement advances only through
//! [`templar_vault_kernel::state::settlement::EpochState::apply_settled`]
//! against this stored state, and every accepted valuation report reference
//! is tracked per market so a settlement can require a strictly newer report
//! sequence than the last one accepted.
//!
//! Epoch mode is disabled by default: with no configured maximum report age
//! the runtime fails closed and refuses epoch actions until governance
//! configures valuation freshness via [`SorobanStorage::save_max_report_age_ns`].
//! The epoch lifecycle record is written and validated exclusively through
//! the kernel settlement state.
//!
//! Records that are absent because the contract has not yet settled return
//! lawful defaults (genesis state, no accepted reports, epoch mode off).
//! Records that are present must decode and rebind exactly; corruption fails
//! closed and is never silently treated as absence.

use alloc::vec::Vec;

use super::{
    decode_epoch_state, encode_epoch_state, finish_decode, push_storage_header_version, push_u64,
    read_exact, read_u64, storage_payload, StorageKind, SorobanStorage, STORAGE_VERSION_CURRENT,
};
use crate::error::RuntimeError;
use templar_vault_kernel::{
    EpochId, EpochPhase, EpochState, TimestampNs, ValuationReportRef,
};

/// The last accepted valuation report reference for one settlement epoch.
///
/// Storage law binds an accepted report to exactly one epoch and enforces a
/// strictly increasing report sequence across epochs. Settlement must use
/// this stored reference: a snapshot that cites any other report, or a report
/// whose sequence is not newer than the last accepted sequence, cannot settle.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct AcceptedReportRecord {
    /// Epoch the accepted report was bound to.
    pub(crate) settlement_epoch: EpochId,
    /// The accepted report reference bound at acceptance time.
    pub(crate) report: ValuationReportRef,
}

impl<'a> SorobanStorage<'a> {
    /// Persist the epoch lifecycle state.
    ///
    /// The caller must hold the authority that the kernel `ApplyEpochSettlement`
    /// / `AdvanceEpoch` actions grant; storage enforces the state law itself,
    /// so a corrupted or tampered epoch record can never be written or read
    /// back as valid. The vault-state header mirror of the same fields must
    /// agree; divergence fails closed.
    pub(crate) fn save_epoch_state(&self, epoch: &EpochState) -> Result<(), RuntimeError> {
        let record = encode_epoch_record(epoch)?;
        if let Some(header_epoch) = self.state_header_epoch()? {
            if header_epoch != *epoch {
                return Err(RuntimeError::storage_error("epoch state mismatch"));
            }
        }
        self.save_versioned(StorageKind::EpochState, &record)
    }

    /// Load the epoch lifecycle state. An absent record is the lawful
    /// pre-settlement state: the vault defaults to the genesis epoch. Any
    /// present record must decode and rebind into a state that satisfies the
    /// kernel settlement law; corruption fails closed.
    pub(crate) fn load_epoch_state(&self) -> Result<EpochState, RuntimeError> {
        let epoch = match self.load_versioned(StorageKind::EpochState)? {
            Some(record) => decode_epoch_record(&record)?,
            None => EpochState::genesis(),
        };
        if let Some(header_epoch) = self.state_header_epoch()? {
            if header_epoch != epoch {
                return Err(RuntimeError::storage_error("epoch state mismatch"));
            }
        }
        Ok(epoch)
    }

    /// Record that a signed valuation report was accepted for settlement of
    /// `settlement_epoch`.
    ///
    /// Acceptance is only lawful while the vault is in the `Cutoff` phase for
    /// exactly that epoch and the valuation timestamp is at or after the
    /// cutoff. An accepted report can never be replaced for the same epoch,
    /// and its sequence must be strictly greater than the last accepted
    /// sequence; a replayed or stale report reference is rejected.
    #[allow(clippy::needless_pass_by_value, reason = "uniform kernel handler signature")]
    pub(crate) fn record_accepted_report(
        &self,
        settlement_epoch: EpochId,
        report: &ValuationReportRef,
    ) -> Result<(), RuntimeError> {
        if !self.epoch_mode_active() {
            return Err(RuntimeError::storage_error("epoch mode disabled"));
        }
        let epoch = self.load_epoch_state()?;
        if epoch.phase != EpochPhase::Cutoff {
            return Err(RuntimeError::storage_error("epoch not at cutoff"));
        }
        if settlement_epoch != epoch.intake_epoch || !settlement_epoch.is_settlement_epoch() {
            return Err(RuntimeError::storage_error("accepted report epoch mismatch"));
        }
        let cutoff = epoch
            .cutoff_ns
            .ok_or_else(|| RuntimeError::storage_error("epoch cutoff missing"))?;
        if report.as_of_ns < cutoff {
            return Err(RuntimeError::storage_error("accepted report before cutoff"));
        }
        let previous = self.load_accepted_report()?;
        if let Some(previous) = &previous {
            if previous.settlement_epoch == settlement_epoch {
                return Err(RuntimeError::storage_error("report already accepted"));
            }
            if report.report_seq <= previous.report.report_seq {
                return Err(RuntimeError::storage_error(
                    "accepted report sequence not increasing",
                ));
            }
        }
        let record = AcceptedReportRecord {
            settlement_epoch,
            report: *report,
        };
        let encoded = encode_accepted_report_record(&record)?;
        self.save_versioned(StorageKind::AcceptedReport, &encoded)
    }

    /// Load the last accepted valuation report reference, if any. Settlement
    /// queries must use this to enforce sequence and freshness law against the
    /// submitted snapshot: the snapshot must cite this accepted report and
    /// this epoch, and the previous accepted sequence bounds the minimum
    /// acceptable sequence for a new settlement.
    pub(crate) fn load_accepted_report(
        &self,
    ) -> Result<Option<AcceptedReportRecord>, RuntimeError> {
        let Some(bytes) = self.load_versioned(StorageKind::AcceptedReport)? else {
            return Ok(None);
        };
        Ok(Some(decode_accepted_report_record(&bytes)?))
    }

    /// Whether any settlement epoch already has an accepted report. Absence
    /// is a lawful "none accepted yet" answer; a corrupt record fails closed.
    pub(crate) fn any_accepted_reports(&self) -> Result<bool, RuntimeError> {
        Ok(self.load_accepted_report()?.is_some())
    }

    /// Set the maximum accepted report age (in nanoseconds), activating
    /// epoch settlement mode. The first governance write is permanent;
    /// replaying the same value is idempotent and reconfiguration fails
    /// closed so freshness cannot be weakened after custodial exposure.
    pub(crate) fn save_max_report_age_ns(&self, max_report_age_ns: u64) -> Result<(), RuntimeError> {
        let record = encode_u64_record(StorageKind::MaxReportAge, max_report_age_ns)?;
        match self.load_max_report_age_ns()? {
            None => self.save_versioned(StorageKind::MaxReportAge, &record),
            Some(existing) if existing == max_report_age_ns => Ok(()),
            Some(_) => Err(RuntimeError::storage_error("epoch freshness bound immutable")),
        }
    }

    /// Load the configured maximum accepted report age, if governance has
    /// activated epoch settlement mode. Absence means epoch mode is disabled;
    /// a corrupt or zero-valued record fails closed.
    pub(crate) fn load_max_report_age_ns(&self) -> Result<Option<u64>, RuntimeError> {
        let Some(record) = self.load_versioned(StorageKind::MaxReportAge)? else {
            return Ok(None);
        };
        Ok(Some(decode_u64_record(&record, StorageKind::MaxReportAge)?))
    }


    /// Whether epoch settlement mode is active: governance must have stored a
    /// positive maximum report age and that record must decode cleanly.
    /// Absent or corrupt configuration disables the mode (fail closed).
    pub(crate) fn epoch_mode_active(&self) -> bool {
        match self.load_max_report_age_ns() {
            Ok(Some(max_age)) => max_age > 0,
            _ => false,
        }
    }

    /// The epoch lifecycle state mirrored into the vault-state header, if a
    /// vault state is stored. The mirror must satisfy the same settlement law
    /// as the dedicated record; the header is not a place to smuggle a
    /// second epoch authority.
    fn state_header_epoch(&self) -> Result<Option<EpochState>, RuntimeError> {
        let Some(stored) = self.load_state_blob()? else {
            return Ok(None);
        };
        Ok(Some(super::epoch_state_from_state_header_blob(&stored)?))
    }
}

/// Encode the epoch lifecycle record for versioned persistence. The record
/// is written only after the kernel settlement law has accepted the state,
/// so no epoch authority enters storage without a lawful binding.
pub(crate) fn encode_epoch_record(epoch: &EpochState) -> Result<Vec<u8>, RuntimeError> {
    if !epoch.check_invariants() {
        return Err(RuntimeError::storage_error("epoch state invariant failed"));
    }
    let mut out = Vec::new();
    push_storage_header_version(
        &mut out,
        StorageKind::EpochState.tag(),
        STORAGE_VERSION_CURRENT,
    );
    encode_epoch_state(epoch, &mut out);
    Ok(out)
}

/// Decode the epoch lifecycle record. [`decode_epoch_state`] revalidates
/// phase/counter coherence, cutoff presence, and reconstructs settled
/// snapshots through [`templar_vault_kernel::state::settlement::EpochSnapshot::bind`].
/// This detects malformed or internally inconsistent storage. Authenticity
/// still relies on contract-controlled storage and the adapter-authenticated
/// report ingestion path; this record is not a cryptographic MAC.
pub(crate) fn decode_epoch_record(bytes: &[u8]) -> Result<EpochState, RuntimeError> {
    let (version, payload) = storage_payload(bytes, StorageKind::EpochState)?;
    if version != STORAGE_VERSION_CURRENT {
        return Err(RuntimeError::storage_error(
            "unsupported epoch storage version",
        ));
    }
    let mut cursor = 0usize;
    let epoch = decode_epoch_state(payload, &mut cursor)?;
    finish_decode(payload, cursor)?;
    Ok(epoch)
}

/// Encode an accepted-report record for versioned persistence. The record
/// binds one accepted valuation report reference to exactly one settlement
/// epoch, so replayed settlements for a different epoch can be detected.
pub(crate) fn encode_accepted_report_record(
    record: &AcceptedReportRecord,
) -> Result<Vec<u8>, RuntimeError> {
    if !record.settlement_epoch.is_settlement_epoch() {
        return Err(RuntimeError::storage_error(
            "accepted report epoch invalid",
        ));
    }
    if record.report.report_seq == 0 {
        return Err(RuntimeError::storage_error(
            "accepted report sequence invalid",
        ));
    }
    let mut out = Vec::new();
    push_storage_header_version(
        &mut out,
        StorageKind::AcceptedReport.tag(),
        STORAGE_VERSION_CURRENT,
    );
    push_u64(&mut out, record.settlement_epoch.as_u64());
    push_u64(&mut out, record.report.report_seq);
    push_u64(&mut out, record.report.as_of_ns.as_u64());
    out.extend_from_slice(&record.report.report_hash);
    Ok(out)
}

/// Decode an accepted-report record. A record whose epoch is not a settlement
/// epoch, whose sequence is zero, or whose trailing bytes are inconsistent is
/// rejected; settlement must then fail closed rather than trust the record.
pub(crate) fn decode_accepted_report_record(
    bytes: &[u8],
) -> Result<AcceptedReportRecord, RuntimeError> {
    let (version, payload) = storage_payload(bytes, StorageKind::AcceptedReport)?;
    if version != STORAGE_VERSION_CURRENT {
        return Err(RuntimeError::storage_error(
            "unsupported accepted report storage version",
        ));
    }
    let mut cursor = 0usize;
    let settlement_epoch = EpochId::new(read_u64(payload, &mut cursor)?);
    if !settlement_epoch.is_settlement_epoch() {
        return Err(RuntimeError::storage_error(
            "accepted report epoch invalid",
        ));
    }
    let report_seq = read_u64(payload, &mut cursor)?;
    if report_seq == 0 {
        return Err(RuntimeError::storage_error(
            "accepted report sequence invalid",
        ));
    }
    let as_of_ns = TimestampNs::from_nanos(read_u64(payload, &mut cursor)?);
    let report_hash = read_exact(payload, &mut cursor, 32)?;
    let mut report_hash_bytes = [0u8; 32];
    report_hash_bytes.copy_from_slice(report_hash);
    finish_decode(payload, cursor)?;
    Ok(AcceptedReportRecord {
        settlement_epoch,
        report: ValuationReportRef {
            report_seq,
            as_of_ns,
            report_hash: report_hash_bytes,
        },
    })
}

/// Encode a positive-u64 configuration record. Zero cannot be written, so a
/// zero-valued freshness bound can never silently disable staleness law.
pub(crate) fn encode_u64_record(kind: StorageKind, value: u64) -> Result<Vec<u8>, RuntimeError> {
    if value == 0 {
        return Err(RuntimeError::storage_error("record value must be positive"));
    }
    let mut out = Vec::new();
    push_storage_header_version(&mut out, kind.tag(), STORAGE_VERSION_CURRENT);
    push_u64(&mut out, value);
    Ok(out)
}

/// Decode a positive-u64 configuration record.
pub(crate) fn decode_u64_record(bytes: &[u8], kind: StorageKind) -> Result<u64, RuntimeError> {
    let (version, payload) = storage_payload(bytes, kind)?;
    if version != STORAGE_VERSION_CURRENT {
        return Err(RuntimeError::storage_error("unsupported record storage version"));
    }
    let mut cursor = 0usize;
    let value = read_u64(payload, &mut cursor)?;
    if value == 0 {
        return Err(RuntimeError::storage_error("record value must be positive"));
    }
    if cursor != payload.len() {
        return Err(RuntimeError::storage_error("record trailing bytes invalid"));
    }
    Ok(value)
}

