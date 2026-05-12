mod monotonic_run;
mod stepwise_change;
mod windowed_change_delta;

#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
#[cfg(feature = "schemars")]
use alloc::{boxed::Box, vec};

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
    }
}

impl CircuitBreaker {
    fn rule(&self) -> &dyn CircuitBreakerRule {
        match self {
            Self::StepwiseChange(inner) => inner,
            Self::MonotonicRun(inner) => inner,
            Self::WindowedChangeDelta(inner) => inner,
        }
    }
}

impl CircuitBreakerRule for CircuitBreaker {
    fn should_trip(&self, history: &RingBuffer<Observation>) -> bool {
        self.rule().should_trip(history)
    }
}

/// Runtime rule interface used by [`CircuitBreakerSet`](super::CircuitBreakerSet).
///
/// The kernel set is generic over this trait for off-chain/library consumers. The NEAR contract
/// intentionally stores and governs only the closed [`CircuitBreaker`] enum so on-chain rule
/// schemas remain explicit and auditable.
pub trait CircuitBreakerRule {
    fn should_trip(&self, history: &RingBuffer<Observation>) -> bool;
}
