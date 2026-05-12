mod error;
mod math;
mod observation;
mod ring_buffer;
mod rule;
mod set;
mod status;

pub use error::{Error, ErrorCode};
pub use observation::Observation;
pub use ring_buffer::RingBuffer;
pub use rule::{
    CircuitBreaker, CircuitBreakerRule, MonotonicRun, StepwiseChange, WindowedChangeDelta,
};
pub use set::{CircuitBreakerSet, CircuitBreakerSetConfig, CircuitBreakerState};
pub use status::CircuitBreakerStatus;

#[cfg(test)]
mod tests;
