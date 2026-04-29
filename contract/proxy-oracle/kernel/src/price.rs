use templar_primitives::time::Nanoseconds;

serialize! {
    #[derive(Clone, Debug)]
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
