mod cumulative_change;
mod monotonic_run;
mod stepwise_change;
mod windowed_change_delta;

#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
#[cfg(feature = "schemars")]
use alloc::{boxed::Box, vec};

pub use cumulative_change::CumulativeChange;
pub use monotonic_run::MonotonicRun;
pub use stepwise_change::StepwiseChange;
pub use windowed_change_delta::WindowedChangeDelta;

use super::{Observation, RingBuffer};

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CircuitBreaker {
        StepwiseChange(StepwiseChange),
        MonotonicRun(MonotonicRun),
        WindowedChangeDelta(WindowedChangeDelta),
        CumulativeChange(CumulativeChange),
    }
}

impl CircuitBreaker {
    fn rule(&self) -> &dyn CircuitBreakerRule {
        match self {
            Self::StepwiseChange(inner) => inner,
            Self::MonotonicRun(inner) => inner,
            Self::WindowedChangeDelta(inner) => inner,
            Self::CumulativeChange(inner) => inner,
        }
    }
}

impl CircuitBreakerRule for CircuitBreaker {
    fn should_trip(&self, history: &RingBuffer<Observation>) -> bool {
        self.rule().should_trip(history)
    }

    fn is_valid_for(
        &self,
        sample_interval_ns: templar_primitives::Nanoseconds,
        history_len: u32,
    ) -> bool {
        let valid_threshold = |threshold: templar_primitives::Decimal| {
            threshold != templar_primitives::Decimal::ZERO
                && threshold <= templar_primitives::Decimal::ONE
        };
        match self {
            Self::StepwiseChange(rule) => {
                history_len >= 2 && valid_threshold(rule.max_relative_change)
            }
            Self::MonotonicRun(rule) => {
                rule.max_streak > 0
                    && rule.max_streak < history_len
                    && sample_interval_ns == templar_primitives::Nanoseconds::zero()
                    && valid_threshold(rule.min_relative_step_change)
            }
            Self::WindowedChangeDelta(rule) => {
                rule.window_len >= 2
                    && rule.lookback_windows > 0
                    && rule
                        .lookback_windows
                        .checked_add(1)
                        .and_then(|windows| rule.window_len.checked_mul(windows))
                        .is_some_and(|required| required <= history_len)
                    && valid_threshold(rule.max_relative_mean_change)
            }
            Self::CumulativeChange(rule) => {
                history_len >= 1
                    && rule.baseline.has_strictly_positive_confidence_interval()
                    && valid_threshold(rule.max_relative_change)
            }
        }
    }
}

/// Runtime rule interface used by [`CircuitBreakerSet`](super::CircuitBreakerSet).
///
/// The kernel set is generic over this trait for off-chain/library consumers. The NEAR contract
/// intentionally stores and governs only the closed [`CircuitBreaker`] enum so on-chain rule
/// schemas remain explicit and auditable.
pub trait CircuitBreakerRule {
    fn should_trip(&self, history: &RingBuffer<Observation>) -> bool;

    fn is_valid_for(
        &self,
        _sample_interval_ns: templar_primitives::Nanoseconds,
        _history_len: u32,
    ) -> bool {
        true
    }
}
