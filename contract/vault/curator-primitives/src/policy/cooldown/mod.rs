//! Cooldown tracking for rate-limiting operations.
//!
//! This module provides a reusable [`Cooldown`] type for tracking time-based
//! rate limits. It's used by both [`RefreshPlan`](super::refresh_plan::RefreshPlan)
//! and [`MarketLock`](super::market_lock::MarketLock) for expiry semantics.

use templar_vault_kernel::TimeGate;

/// Tracks cooldown state for rate-limited operations.
///
/// A cooldown enforces a minimum interval between operations. It tracks
/// when the last operation occurred and the required interval before
/// the next operation is allowed.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Cooldown {
    /// Timestamp of the last operation (nanoseconds), if any.
    pub last_event_ns: Option<u64>,
    /// Required interval between operations (nanoseconds).
    /// Zero means no cooldown (always ready).
    pub interval_ns: u64,
}

impl Cooldown {
    fn gate(&self) -> TimeGate {
        if self.is_unlimited() {
            return TimeGate::ready_now();
        }

        match self.last_event_ns {
            Some(last) => TimeGate::schedule_from(last, self.interval_ns),
            None => TimeGate::ready_now(),
        }
    }

    #[must_use]
    pub fn new(interval_ns: u64) -> Self {
        Self {
            last_event_ns: None,
            interval_ns,
        }
    }

    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            last_event_ns: None,
            interval_ns: 0,
        }
    }

    #[must_use]
    pub fn with_last_event(interval_ns: u64, last_event_ns: u64) -> Self {
        Self {
            last_event_ns: Some(last_event_ns),
            interval_ns,
        }
    }

    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.interval_ns == 0
    }

    /// Check if an operation is allowed at the given timestamp.
    ///
    /// Returns `true` if:
    /// - No cooldown is configured (interval_ns == 0), or
    /// - No previous operation has occurred, or
    /// - Sufficient time has elapsed since the last operation
    #[must_use]
    pub fn is_ready(&self, current_ns: u64) -> bool {
        self.gate().is_ready(current_ns)
    }

    /// Check cooldown and return an error if not ready.
    pub fn check(&self, current_ns: u64) -> Result<(), CooldownError> {
        if self.is_ready(current_ns) {
            Ok(())
        } else {
            Err(CooldownError::OnCooldown {
                last_event_ns: self.last_event_ns.unwrap_or(0),
                interval_ns: self.interval_ns,
                current_ns,
            })
        }
    }

    #[must_use]
    pub fn record(&self, timestamp_ns: u64) -> Self {
        Self {
            last_event_ns: Some(timestamp_ns),
            interval_ns: self.interval_ns,
        }
    }

    #[must_use]
    pub fn ready_at(&self) -> Option<u64> {
        self.gate().ready_at_ns()
    }

    #[must_use]
    pub fn remaining(&self, current_ns: u64) -> u64 {
        self.gate().remaining(current_ns)
    }
}

/// Errors that can occur during cooldown checks.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum CooldownError {
    /// Operation is still on cooldown.
    OnCooldown {
        last_event_ns: u64,
        interval_ns: u64,
        current_ns: u64,
    },
}
