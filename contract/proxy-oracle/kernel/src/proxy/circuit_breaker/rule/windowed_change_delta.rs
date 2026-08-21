#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::Decimal;

use crate::proxy::circuit_breaker::{
    math::relative_sum_change_exceeds, CircuitBreakerRule, Observation, RingBuffer,
};

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    /// Trips when equal-sized current and historical windows have different means.
    pub struct WindowedChangeDelta {
        /// Number of observations in each compared window.
        pub window_len: u32,
        /// Number of full windows to look back from the current window.
        ///
        /// This is an offset, not a scan count. A value of `1` compares the current
        /// window to the immediately preceding window; a value of `2` skips one
        /// full window and compares against the window before that.
        pub lookback_windows: u32,
        /// Maximum allowed relative difference between the two windows' means.
        pub max_relative_mean_change: Decimal,
    }
}

impl CircuitBreakerRule for WindowedChangeDelta {
    fn should_trip(&self, history: &RingBuffer<Observation>) -> bool {
        let window_len = self.window_len as usize;
        let lookback_windows = self.lookback_windows as usize;
        if window_len < 2 || lookback_windows == 0 {
            return false;
        }

        let Some(current_start) = history.len().checked_sub(window_len) else {
            return false;
        };
        let Some(lookback_offset) = lookback_windows.checked_mul(window_len) else {
            return false;
        };
        let Some(previous_start) = current_start.checked_sub(lookback_offset) else {
            return false;
        };
        let previous = history
            .iter()
            .skip(previous_start)
            .take(window_len)
            .map(|observation| &observation.price);
        let current = history
            .iter()
            .skip(current_start)
            .take(window_len)
            .map(|observation| &observation.price);

        relative_sum_change_exceeds(previous, current, self.max_relative_mean_change)
    }
}
