#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::Nanoseconds;

use crate::Price;

serialize! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Observation {
        pub price: Price,
        pub observed_at_ns: Nanoseconds,
    }
}
