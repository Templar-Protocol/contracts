mod error;
mod math;
mod observation;
mod ring_buffer;
mod rule;
mod set;
mod status;

pub use error::{CircuitBreakerError, ErrorCode};
pub use observation::Observation;
pub use ring_buffer::{RingBuffer, RingBufferParseError, UncheckedRingBuffer};
pub use rule::{
    CircuitBreaker, CircuitBreakerRule, MonotonicRun, StepwiseChange, WindowedChangeDelta,
};
pub use set::{
    AcceptedHistorySource, CircuitBreakerEvent, CircuitBreakerOutcome, CircuitBreakerSet,
    CircuitBreakerSetConfig, CircuitBreakerSetParseError, CircuitBreakerState, PriceAcceptance,
    PriceBlockedReason, UncheckedCircuitBreakerSet,
};
pub use status::CircuitBreakerStatus;

#[cfg(test)]
mod tests;
