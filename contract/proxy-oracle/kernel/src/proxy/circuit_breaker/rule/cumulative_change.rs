#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::Decimal;

use crate::{
    proxy::circuit_breaker::{
        math::relative_abs_change_exceeds, CircuitBreakerRule, Observation, RingBuffer,
    },
    Price,
};

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CumulativeChange {
        pub baseline: Price,
        pub max_relative_change: Decimal,
    }
}

impl CircuitBreakerRule for CumulativeChange {
    fn should_trip(&self, history: &RingBuffer<Observation>) -> bool {
        history.last().is_some_and(|observation| {
            relative_abs_change_exceeds(
                &self.baseline,
                &observation.price,
                self.max_relative_change,
            )
        })
    }
}
