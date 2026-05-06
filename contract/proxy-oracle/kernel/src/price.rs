#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::time::Nanoseconds;

serialize! {
    #[derive(Clone, Copy, Debug)]
    pub struct Price {
        pub price: i64,
        /// Confidence interval around the price
        pub conf: u64,
        /// The exponent
        pub expo: i32,
        /// Unix timestamp of when this price was computed
        pub publish_time_ns: Nanoseconds,
    }
}
