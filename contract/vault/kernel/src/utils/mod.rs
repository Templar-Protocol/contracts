//! Shared non-domain helpers for kernel-adjacent crates.

use crate::types::{DurationNs, TimestampNs};

/// Generic readiness gate represented by an optional ready-at timestamp.
///
/// - `None` means ready immediately.
/// - `Some(ts)` means ready when `now >= ts`.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct TimeGate {
    ready_at_ns: Option<TimestampNs>,
}

impl TimeGate {
    #[must_use]
    pub const fn ready_now() -> Self {
        Self { ready_at_ns: None }
    }

    #[must_use]
    pub const fn from_ready_at(ready_at_ns: TimestampNs) -> Self {
        Self {
            ready_at_ns: Some(ready_at_ns),
        }
    }

    #[must_use]
    pub const fn schedule_from(now_ns: TimestampNs, delay_ns: DurationNs) -> Self {
        Self::from_ready_at(now_ns.saturating_add_duration(delay_ns))
    }

    #[must_use]
    pub const fn ready_at_ns(self) -> Option<TimestampNs> {
        self.ready_at_ns
    }

    #[must_use]
    pub fn is_ready(self, now_ns: TimestampNs) -> bool {
        self.ready_at_ns.is_none_or(|ready_at| now_ns >= ready_at)
    }

    #[must_use]
    pub fn remaining(self, now_ns: TimestampNs) -> TimestampNs {
        self.ready_at_ns.map_or(TimestampNs::ZERO, |ready_at| {
            ready_at.saturating_sub(now_ns)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::TimeGate;
    use crate::types::{DurationNs, TimestampNs};

    #[test]
    fn time_gate_ready_now_is_always_ready() {
        let gate = TimeGate::ready_now();
        assert!(gate.is_ready(TimestampNs(0)));
        assert!(gate.is_ready(TimestampNs(u64::MAX)));
        assert_eq!(gate.remaining(TimestampNs(123)), TimestampNs(0));
        assert_eq!(gate.ready_at_ns(), None);
    }

    #[test]
    fn time_gate_scheduled_reports_remaining_and_readiness() {
        let gate = TimeGate::schedule_from(TimestampNs(100), DurationNs(50));
        assert_eq!(gate.ready_at_ns(), Some(TimestampNs(150)));
        assert!(!gate.is_ready(TimestampNs(149)));
        assert!(gate.is_ready(TimestampNs(150)));
        assert_eq!(gate.remaining(TimestampNs(120)), TimestampNs(30));
        assert_eq!(gate.remaining(TimestampNs(160)), TimestampNs(0));
    }
}
