//! Cooldown tracking for rate-limiting operations.
//!
//! This module provides a reusable [`Cooldown`] type for tracking time-based
//! rate limits. It's used by both [`RefreshPlan`](super::refresh_plan::RefreshPlan)
//! and [`MarketLock`](super::market_lock::MarketLock) for expiry semantics.

/// Tracks cooldown state for rate-limited operations.
///
/// A cooldown enforces a minimum interval between operations. It tracks
/// when the last operation occurred and the required interval before
/// the next operation is allowed.
///
/// # Example
///
/// ```ignore
/// use templar_curator_primitives::policy::cooldown::Cooldown;
///
/// let cooldown = Cooldown::new(1000); // 1000ns interval
/// assert!(cooldown.is_ready(500)); // First operation always ready
///
/// let cooldown = cooldown.record(500);
/// assert!(!cooldown.is_ready(1000)); // Only 500ns elapsed
/// assert!(cooldown.is_ready(1500)); // 1000ns elapsed, ready
/// ```
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cooldown {
    /// Timestamp of the last operation (nanoseconds), if any.
    pub last_event_ns: Option<u64>,
    /// Required interval between operations (nanoseconds).
    /// Zero means no cooldown (always ready).
    pub interval_ns: u64,
}

impl Cooldown {
    /// Create a new cooldown with the given interval.
    ///
    /// An interval of 0 means no cooldown is enforced.
    #[must_use]
    pub fn new(interval_ns: u64) -> Self {
        Self {
            last_event_ns: None,
            interval_ns,
        }
    }

    /// Create a cooldown with no rate limit.
    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            last_event_ns: None,
            interval_ns: 0,
        }
    }

    /// Create a cooldown with a known last event time.
    #[must_use]
    pub fn with_last_event(interval_ns: u64, last_event_ns: u64) -> Self {
        Self {
            last_event_ns: Some(last_event_ns),
            interval_ns,
        }
    }

    /// Returns true if there is no cooldown interval configured.
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
        if self.is_unlimited() {
            return true;
        }

        match self.last_event_ns {
            None => true,
            Some(last) => {
                let elapsed = current_ns.saturating_sub(last);
                elapsed >= self.interval_ns
            }
        }
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

    /// Record an operation at the given timestamp.
    ///
    /// Returns a new cooldown with the updated last event time.
    #[must_use]
    pub fn record(&self, timestamp_ns: u64) -> Self {
        Self {
            last_event_ns: Some(timestamp_ns),
            interval_ns: self.interval_ns,
        }
    }

    /// Compute when the cooldown will be ready.
    ///
    /// Returns `None` if already ready or no cooldown is configured.
    /// Returns `Some(timestamp)` indicating when the cooldown expires.
    #[must_use]
    pub fn ready_at(&self) -> Option<u64> {
        if self.is_unlimited() {
            return None;
        }

        self.last_event_ns
            .map(|last| last.saturating_add(self.interval_ns))
    }

    /// Compute remaining time until ready.
    ///
    /// Returns 0 if already ready.
    #[must_use]
    pub fn remaining(&self, current_ns: u64) -> u64 {
        if self.is_ready(current_ns) {
            return 0;
        }

        match self.ready_at() {
            Some(ready) => ready.saturating_sub(current_ns),
            None => 0,
        }
    }
}

/// Errors that can occur during cooldown checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CooldownError {
    /// Operation is still on cooldown.
    OnCooldown {
        last_event_ns: u64,
        interval_ns: u64,
        current_ns: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unlimited_cooldown() {
        let cooldown = Cooldown::unlimited();
        assert!(cooldown.is_unlimited());
        assert!(cooldown.is_ready(0));
        assert!(cooldown.is_ready(u64::MAX));
    }

    #[test]
    fn test_first_operation_always_ready() {
        let cooldown = Cooldown::new(1000);
        assert!(cooldown.is_ready(0));
        assert!(cooldown.is_ready(500));
    }

    #[test]
    fn test_cooldown_enforced() {
        let cooldown = Cooldown::new(1000);
        let cooldown = cooldown.record(100);

        // Not ready yet
        assert!(!cooldown.is_ready(100));
        assert!(!cooldown.is_ready(500));
        assert!(!cooldown.is_ready(1099));

        // Ready at exactly interval
        assert!(cooldown.is_ready(1100));
        assert!(cooldown.is_ready(2000));
    }

    #[test]
    fn test_check_returns_error() {
        let cooldown = Cooldown::with_last_event(1000, 100);

        let result = cooldown.check(500);
        assert!(matches!(result, Err(CooldownError::OnCooldown { .. })));

        let result = cooldown.check(1100);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ready_at() {
        let cooldown = Cooldown::new(1000);
        assert_eq!(cooldown.ready_at(), None); // No last event

        let cooldown = cooldown.record(100);
        assert_eq!(cooldown.ready_at(), Some(1100));

        let unlimited = Cooldown::unlimited();
        assert_eq!(unlimited.ready_at(), None);
    }

    #[test]
    fn test_remaining() {
        let cooldown = Cooldown::with_last_event(1000, 100);

        assert_eq!(cooldown.remaining(100), 1000);
        assert_eq!(cooldown.remaining(500), 600);
        assert_eq!(cooldown.remaining(1100), 0);
        assert_eq!(cooldown.remaining(2000), 0);
    }

    #[test]
    fn test_record_updates_last_event() {
        let cooldown = Cooldown::new(1000);
        assert_eq!(cooldown.last_event_ns, None);

        let cooldown = cooldown.record(500);
        assert_eq!(cooldown.last_event_ns, Some(500));

        let cooldown = cooldown.record(1500);
        assert_eq!(cooldown.last_event_ns, Some(1500));
    }
}
