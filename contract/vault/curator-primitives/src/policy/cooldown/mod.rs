//! Cooldown tracking for rate-limiting operations.
//!
//! This module provides a reusable [`Cooldown`] type for tracking time-based
//! rate limits. It's used by both [`RefreshPlan`](super::refresh_plan::RefreshPlan)
//! and [`MarketLock`](super::market_lock::MarketLock) for expiry semantics.

use core::num::NonZeroU64;

use templar_vault_kernel::{DurationNs, TimeGate, TimestampNs};

/// Tracks cooldown state for rate-limited operations.
///
/// A cooldown enforces a minimum interval between operations. It tracks
/// when the last operation occurred and the required interval before
/// the next operation is allowed.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cooldown {
    /// Timestamp of the last operation (nanoseconds), if any.
    last_event_ns: Option<u64>,
    /// Required interval between operations (nanoseconds), if finite.
    interval_ns: Option<NonZeroU64>,
}

impl Cooldown {
    #[must_use]
    fn normalized(last_event_ns: Option<u64>, interval_ns: Option<NonZeroU64>) -> Self {
        Self {
            last_event_ns,
            interval_ns,
        }
    }

    fn gate(&self) -> TimeGate {
        if self.is_unlimited() {
            return TimeGate::ready_now();
        }

        match self.last_event_ns {
            Some(last) => TimeGate::schedule_from(
                TimestampNs(last),
                DurationNs(self.interval_ns.unwrap().get()),
            ),
            None => TimeGate::ready_now(),
        }
    }

    #[must_use]
    pub fn new(interval_ns: NonZeroU64) -> Self {
        Self::normalized(None, Some(interval_ns))
    }

    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            last_event_ns: None,
            interval_ns: None,
        }
    }

    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.interval_ns.is_none()
    }

    #[must_use]
    pub fn last_event_ns(&self) -> Option<u64> {
        self.last_event_ns
    }

    #[must_use]
    pub fn interval_ns(&self) -> Option<NonZeroU64> {
        self.interval_ns
    }

    /// Check if an operation is allowed at the given timestamp.
    ///
    /// Returns `true` if:
    /// - No cooldown is configured, or
    /// - No previous operation has occurred, or
    /// - Sufficient time has elapsed since the last operation
    ///
    /// Readiness is inclusive at the exact boundary: `current_ns == ready_at()` is ready.
    /// Callers are expected to pass a non-decreasing clock source; this type does not
    /// correct or reject backward-moving timestamps.
    #[must_use]
    pub fn is_ready(&self, current_ns: u64) -> bool {
        self.gate().is_ready(TimestampNs(current_ns))
    }

    pub fn try_acquire(self, current_ns: u64) -> Result<Self, CooldownError> {
        match self.ready_at() {
            Some(ready_at_ns) if current_ns < ready_at_ns => Err(CooldownError::OnCooldown {
                ready_at_ns,
                remaining_ns: ready_at_ns - current_ns,
            }),
            _ => Ok(self.recorded_at(current_ns)),
        }
    }

    /// Check cooldown and return an error if not ready.
    pub fn check(&self, current_ns: u64) -> Result<(), CooldownError> {
        self.try_acquire(current_ns).map(|_| ())
    }

    #[must_use]
    pub fn recorded_at(self, timestamp_ns: u64) -> Self {
        Self::normalized(Some(timestamp_ns), self.interval_ns)
    }

    #[must_use]
    pub fn with_last_event_ns(self, last_event_ns: Option<u64>) -> Self {
        Self::normalized(last_event_ns, self.interval_ns)
    }

    #[must_use]
    pub fn ready_at(&self) -> Option<u64> {
        self.gate().ready_at_ns().map(Into::into)
    }

    #[must_use]
    pub fn remaining(&self, current_ns: u64) -> u64 {
        self.gate().remaining(TimestampNs(current_ns)).into()
    }
}

/// Errors that can occur during cooldown checks.
#[templar_vault_macros::vault_derive]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CooldownError {
    /// Operation is still on cooldown.
    OnCooldown { ready_at_ns: u64, remaining_ns: u64 },
}
