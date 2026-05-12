use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    TooManyBreakers = 1,
    BreakerNotFound = 2,
    ManuallyTripped = 3,
    BreakerTripped = 4,
    UnexpectedBreakerId = 5,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitBreakerError {
    TooManyBreakers,
    BreakerNotFound { breaker_id: u32 },
    UnexpectedBreakerId { expected: u32, actual: u32 },
    ManuallyTripped,
    Tripped { breaker_ids: Vec<u32> },
}

impl CircuitBreakerError {
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::TooManyBreakers => ErrorCode::TooManyBreakers,
            Self::BreakerNotFound { .. } => ErrorCode::BreakerNotFound,
            Self::UnexpectedBreakerId { .. } => ErrorCode::UnexpectedBreakerId,
            Self::ManuallyTripped => ErrorCode::ManuallyTripped,
            Self::Tripped { .. } => ErrorCode::BreakerTripped,
        }
    }
}

#[cfg(feature = "std")]
impl core::fmt::Display for CircuitBreakerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooManyBreakers => write!(f, "too many circuit breakers"),
            Self::BreakerNotFound { breaker_id } => {
                write!(f, "circuit breaker not found: {breaker_id}")
            }
            Self::UnexpectedBreakerId { expected, actual } => {
                write!(
                    f,
                    "unexpected circuit breaker ID: expected {expected}, got {actual}"
                )
            }
            Self::ManuallyTripped => write!(f, "circuit breaker manually tripped"),
            Self::Tripped { breaker_ids } => write!(f, "circuit breaker tripped: {breaker_ids:?}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CircuitBreakerError {}
