#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use core::cmp::Ordering;

use templar_primitives::time::Nanoseconds;

serialize! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

// Compare signed decimal mantissas exactly without normalizing through a fixed-width scaled
// integer. Once sign is handled, compare by decimal magnitude (`digits + exponent`) and only
// rescale by the difference in mantissa digit counts, which is bounded for `i64` values.
pub(crate) fn compare_scaled(
    left_value: i64,
    left_exponent: i32,
    right_value: i64,
    right_exponent: i32,
) -> Ordering {
    match (left_value.cmp(&0), right_value.cmp(&0)) {
        (Ordering::Equal, right_sign) => return right_sign.reverse(),
        (left_sign, Ordering::Equal) => return left_sign,
        (left_sign, right_sign) if left_sign != right_sign => return left_sign,
        _ => {}
    }

    let negative = left_value.is_negative();
    let left_abs = u128::from(left_value.unsigned_abs());
    let right_abs = u128::from(right_value.unsigned_abs());
    let left_log10 = left_abs.ilog10();
    let right_log10 = right_abs.ilog10();

    let left_scale = i64::from(left_exponent) + i64::from(left_log10);
    let right_scale = i64::from(right_exponent) + i64::from(right_log10);
    let magnitude_order = left_scale.cmp(&right_scale).then_with(|| {
        let max_digits = left_log10.max(right_log10);
        let left_scaled = left_abs * 10u128.pow(max_digits - left_log10);
        let right_scaled = right_abs * 10u128.pow(max_digits - right_log10);
        left_scaled.cmp(&right_scaled)
    });

    if negative {
        magnitude_order.reverse()
    } else {
        magnitude_order
    }
}
