use super::Observation;
#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
#[cfg(feature = "schemars")]
use alloc::{boxed::Box, vec};
use templar_primitives::Nanoseconds;

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CircuitBreakerStatus {
        Armed,
        Muted { until_ns: Nanoseconds },
        Tripped { tripped_at_ns: Nanoseconds, price_update: Observation },
    }
}
