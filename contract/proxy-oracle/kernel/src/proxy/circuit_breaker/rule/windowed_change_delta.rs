#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::Decimal;

use crate::proxy::circuit_breaker::{
    math::relative_signed_change, CircuitBreakerRule, Observation, RingBuffer,
};

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    /// Trips when the current window's signed relative change differs too much
    /// from a historical window at a fixed lookback offset.
    pub struct WindowedChangeDelta {
        /// Number of observations in each compared window.
        pub window_len: u32,
        /// Number of full windows to look back from the current window.
        ///
        /// This is an offset, not a scan count. A value of `1` compares the current
        /// window to the immediately preceding window; a value of `2` skips one
        /// full window and compares against the window before that.
        pub lookback_windows: u32,
        /// Maximum allowed absolute difference between the two windows' signed
        /// relative cumulative changes.
        pub max_relative_change_delta: Decimal,
    }
}

impl WindowedChangeDelta {
    fn change_delta(&self, history: &RingBuffer<Observation>) -> Option<Decimal> {
        let window_len = self.window_len as usize;
        let lookback_windows = self.lookback_windows as usize;
        if window_len < 2 || lookback_windows == 0 {
            return None;
        }

        let current_start = history.len().checked_sub(window_len)?;
        let lookback_offset = lookback_windows.checked_mul(window_len)?;
        let previous_start = current_start.checked_sub(lookback_offset)?;
        let previous_last = previous_start.checked_add(window_len - 1)?;

        let current_first = history.get(current_start)?;
        let current_last = history.last()?;
        let previous_first = history.get(previous_start)?;
        let previous_last = history.get(previous_last)?;

        let current = relative_signed_change(&current_first.price, &current_last.price)?;
        let previous = relative_signed_change(&previous_first.price, &previous_last.price)?;

        Some(current.abs_diff(previous))
    }
}

impl CircuitBreakerRule for WindowedChangeDelta {
    fn should_trip(&self, history: &RingBuffer<Observation>) -> bool {
        self.change_delta(history)
            .is_some_and(|delta| delta > self.max_relative_change_delta)
    }
}
