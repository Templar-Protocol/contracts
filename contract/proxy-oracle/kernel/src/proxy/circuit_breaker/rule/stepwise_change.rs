#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::Decimal;

use crate::proxy::circuit_breaker::{
    math::relative_abs_change_exceeds, CircuitBreakerRule, Observation, RingBuffer,
};

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    /// Trips when the relative absolute change between adjacent observations
    /// exceeds the configured maximum.
    pub struct StepwiseChange {
        /// Maximum allowed relative absolute change between adjacent prices.
        pub max_relative_change: Decimal,
    }
}

impl CircuitBreakerRule for StepwiseChange {
    fn should_trip(&self, history: &RingBuffer<Observation>) -> bool {
        if history.len() < 2 {
            return false;
        }
        let Some(previous) = history.get(history.len() - 2) else {
            return false;
        };
        let Some(current) = history.get(history.len() - 1) else {
            return false;
        };

        relative_abs_change_exceeds(&previous.price, &current.price, self.max_relative_change)
    }
}
