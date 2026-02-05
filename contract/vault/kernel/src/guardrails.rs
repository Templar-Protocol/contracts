use crate::math::wad::Wad;

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Optional pause triggers that can be enforced by the runtime.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PauseTriggers {
    /// Pause if total assets drop by more than this WAD fraction.
    pub max_loss_wad: Option<Wad>,
    /// Pause if reported external assets drift beyond this WAD fraction.
    pub max_external_drift_wad: Option<Wad>,
}

#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guardrails {
    /// Maximum number of pending withdrawals allowed in the queue.
    pub max_pending_withdrawals: u32,
    /// Minimum deposit amount in base asset units.
    pub min_deposit: Option<u128>,
    /// Maximum size of a single deposit in base asset units.
    pub max_single_deposit: Option<u128>,
    /// Maximum size of a single withdrawal in base asset units.
    pub max_single_withdrawal: Option<u128>,
    /// Maximum aggregate withdrawal amount allowed over a 24h window.
    pub daily_withdrawal_limit: Option<u128>,
    /// Maximum tolerated slippage expressed as a WAD fraction.
    pub slippage_tolerance_wad: Option<Wad>,
    /// Optional pause triggers enforced by the runtime.
    pub pause_triggers: PauseTriggers,
}

impl Guardrails {
    /// Create guardrails with only the required queue bound.
    #[inline]
    #[must_use]
    pub const fn new(max_pending_withdrawals: u32) -> Self {
        Self {
            max_pending_withdrawals,
            min_deposit: None,
            max_single_deposit: None,
            max_single_withdrawal: None,
            daily_withdrawal_limit: None,
            slippage_tolerance_wad: None,
            pause_triggers: PauseTriggers {
                max_loss_wad: None,
                max_external_drift_wad: None,
            },
        }
    }
}
