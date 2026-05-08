#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::Decimal;

use crate::proxy::circuit_breaker::{
    math::{classify_step_change, StepChange},
    CircuitBreakerRule, Observation, RingBuffer,
};

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    /// Trips when recent price changes form a long enough same-direction run.
    pub struct MonotonicRun {
        /// Number of consecutive significant same-direction steps required to trip.
        pub max_streak: u32,
        /// Minimum relative absolute change for a step to count toward the streak.
        pub min_relative_step_change: Decimal,
    }
}

impl CircuitBreakerRule for MonotonicRun {
    fn should_trip(&self, history: &RingBuffer<Observation>) -> bool {
        if self.max_streak == 0 || history.len() < 2 {
            return false;
        }

        let mut direction = None;
        let mut streak = 0_u32;

        for pair in history.as_slice().windows(2).rev() {
            let step_change = classify_step_change(
                &pair[0].price,
                &pair[1].price,
                self.min_relative_step_change,
            );

            if step_change == StepChange::Minor {
                break;
            }

            if let Some(direction) = direction {
                if direction != step_change {
                    break;
                }
            } else {
                direction = Some(step_change);
            }

            streak = streak.saturating_add(1);
            if streak >= self.max_streak {
                return true;
            }
        }

        false
    }
}
