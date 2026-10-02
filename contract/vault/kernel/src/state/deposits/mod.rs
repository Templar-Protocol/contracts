//! Chain-agnostic pending-deposit primitives.
//!
//! Pending deposits are liabilities recorded outside vault pricing. They are
//! excluded from `VaultState` totals (`total_assets`, `total_shares`,
//! `idle_assets`, `external_assets`) until immutable settlement admission:
//! admission is performed only by epoch settlement from an accepted
//! [`crate::state::settlement::EpochSnapshot`], never by request-time pricing.
//!
//! A `PendingDeposit` carries no NAV, price, or share field, so a fixed
//! pre-settlement claim is unrepresentable by construction. Settlement math
//! must recompute everything from the bound snapshot at execution time.

use crate::state::settlement::{EpochId, RequestError};
use crate::types::{Address, TimestampNs};

#[cfg(feature = "borsh-schema")]
use alloc::string::ToString;

/// Maximum pending deposit queue length (kernel-wide upper bound).
pub const MAX_PENDING_DEPOSITS: usize = 1024;

/// A pending, unadmitted deposit liability.
///
/// Holds only identity and amount data. No asset-claim or pricing field
/// exists; deposits are admitted exclusively through epoch settlement.
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct PendingDeposit {
    pub owner: Address,
    pub assets: u128,
    pub requested_at_ns: TimestampNs,
    pub epoch_id: EpochId,
}

impl PendingDeposit {
    /// Construct a pending deposit bound to a settlement-eligible epoch.
    ///
    /// Rejects zero amounts and non-settlement epochs so unbounded or
    /// epochless liabilities cannot enter the ledger.
    #[must_use = "return value binds or validates settlement law; ignoring it can admit an unpriced claim"]
    pub fn new(
        owner: Address,
        assets: u128,
        requested_at_ns: TimestampNs,
        epoch_id: EpochId,
    ) -> Result<Self, RequestError> {
        if assets == 0 {
            return Err(RequestError::ZeroDepositAssets);
        }
        if !epoch_id.is_settlement_epoch() {
            return Err(RequestError::InvalidEpoch);
        }
        Ok(Self {
            owner,
            assets,
            requested_at_ns,
            epoch_id,
        })
    }
}

#[cfg(all(test, feature = "action-epoch-settlement"))]
mod tests;
