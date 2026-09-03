use std::collections::BTreeSet;

use crate::error::{Error, Result};

/// Uniquely identifies an appended packet record:
/// `(source_eid, sender, nonce, guid)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct PacketKey {
    pub source_eid: u32,
    pub sender: [u8; 32],
    pub nonce: u64,
    pub guid: [u8; 32],
}

/// Mutually exclusive custody stages a packet can occupy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketStage {
    /// Locked on Stellar, not yet minted on EVM.
    ForwardLocked,
    /// Burned on EVM, not yet unlocked on Stellar.
    ReverseAfterBurn,
    /// Fully settled on both chains; contributes no outstanding obligation.
    Delivered,
}

impl PacketStage {
    /// Classifies a message-ledger stage kind. Unknown kinds are custody
    /// failures so unknown records can never contribute silently.
    pub fn classify(kind: &str) -> Result<Self> {
        match kind {
            "forward_locked" => Ok(Self::ForwardLocked),
            "reverse_after_burn" => Ok(Self::ReverseAfterBurn),
            "delivered" => Ok(Self::Delivered),
            other => Err(Error::Custody(format!(
                "unknown packet stage kind {other:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketRecord {
    pub key: PacketKey,
    pub stage: PacketStage,
    pub amount_raw: u128,
}

/// Outstanding forward and reverse obligations in raw common units.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StageDeltas {
    pub pending_forward_raw: u128,
    pub pending_reverse_raw: u128,
}

/// Observed on-chain custody snapshot in raw common units. External fees are
/// paid in the native gas asset outside the lockbox, so they are reported,
/// never folded into the conservation equation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObservedCustody {
    pub observed_lockbox_raw: u128,
    pub normalized_evm_supply_raw: u128,
    pub lockbox_retained_fee_or_dust_raw: u128,
    pub external_fee_reported_raw: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconcileReport {
    pub expected_lockbox_raw: u128,
    pub observed_lockbox_raw: u128,
    pub deficit_raw: u128,
    pub surplus_raw: u128,
    pub external_fee_reported_raw: u128,
}

/// Aggregates stage deltas from packet records. Each unique packet key
/// contributes exactly one mutually exclusive delta; duplicate keys are
/// conflicts and unknown stages are custody failures, so neither ever
/// contributes silently.
pub fn aggregate(records: &[PacketRecord]) -> Result<StageDeltas> {
    let mut seen = BTreeSet::new();
    let mut deltas = StageDeltas::default();
    for record in records {
        if !seen.insert(record.key) {
            return Err(Error::Conflict(format!(
                "duplicate packet record for guid {}",
                hex::encode(record.key.guid)
            )));
        }
        let target = match record.stage {
            PacketStage::ForwardLocked => &mut deltas.pending_forward_raw,
            PacketStage::ReverseAfterBurn => &mut deltas.pending_reverse_raw,
            PacketStage::Delivered => continue,
        };
        *target = (*target)
            .checked_add(record.amount_raw)
            .ok_or_else(|| Error::Custody("pending obligation overflow".into()))?;
    }
    Ok(deltas)
}

/// Computes the custody conservation report where the expected lockbox equals
/// the sum of normalized EVM supply, pending reverse after burn, lockbox
/// retained fee/dust, and pending forward locked-not-minted; deficit is
/// `max(0, expected - observed)` and surplus is `max(0, observed - expected)`.
pub fn reconcile(observed: &ObservedCustody, deltas: &StageDeltas) -> Result<ReconcileReport> {
    let expected_lockbox_raw = observed
        .normalized_evm_supply_raw
        .checked_add(deltas.pending_reverse_raw)
        .and_then(|partial| partial.checked_add(deltas.pending_forward_raw))
        .and_then(|partial| partial.checked_add(observed.lockbox_retained_fee_or_dust_raw))
        .ok_or_else(|| Error::Custody("expected lockbox computation overflows".into()))?;
    let deficit_raw = expected_lockbox_raw.saturating_sub(observed.observed_lockbox_raw);
    let surplus_raw = observed
        .observed_lockbox_raw
        .saturating_sub(expected_lockbox_raw);
    Ok(ReconcileReport {
        expected_lockbox_raw,
        observed_lockbox_raw: observed.observed_lockbox_raw,
        deficit_raw,
        surplus_raw,
        external_fee_reported_raw: observed.external_fee_reported_raw,
    })
}

/// `reconcile` command: verifies log integrity then reports custody from
/// state-bound observations. Fails closed when opening custody is absent.
pub fn run_command(
    state: &std::path::Path,
    fail_on_deficit: bool,
) -> Result<crate::output::CommandData> {
    let store = crate::state::RouteStore::open(state)?;
    let route = store.load_state()?;
    store.verify_log::<crate::state::OperationEventV1>(&route.operations_log, "operations")?;
    let Some(opening) = route.opening_custody else {
        return Err(Error::Custody(
            "opening custody is not finalized; reconciliation requires adopted baseline".into(),
        ));
    };
    let observed = ObservedCustody {
        observed_lockbox_raw: opening.lockbox_raw,
        normalized_evm_supply_raw: opening.evm_supply_raw,
        lockbox_retained_fee_or_dust_raw: 0,
        external_fee_reported_raw: 0,
    };
    let report = reconcile(&observed, &StageDeltas::default())?;
    if fail_on_deficit && report.deficit_raw > 0 {
        return Err(Error::Custody(format!(
            "custody deficit {} raw; failing per --fail-on-deficit",
            report.deficit_raw
        )));
    }
    Ok(crate::output::CommandData {
        result: serde_json::json!({
            "expected_lockbox_raw": report.expected_lockbox_raw.to_string(),
            "observed_lockbox_raw": report.observed_lockbox_raw.to_string(),
            "deficit_raw": report.deficit_raw.to_string(),
            "surplus_raw": report.surplus_raw.to_string()
        }),
        artifact: None,
    })
}

/// `health` command: stable custody/config health over verified state.
pub fn health_command(state: &std::path::Path) -> Result<crate::output::CommandData> {
    let store = crate::state::RouteStore::open(state)?;
    let route = store.load_state()?;
    store.verify_log::<crate::state::OperationEventV1>(&route.operations_log, "operations")?;
    Ok(crate::output::CommandData {
        result: serde_json::json!({
            "route_id": route.route_id,
            "opening_custody_finalized": route.opening_custody.is_some(),
            "log_chain_verified": true,
            "whole_directory_rollback_residual_risk": true
        }),
        artifact: None,
    })
}
