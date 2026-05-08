#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    OrderOccupied = 1,
    BreakerNotFound = 2,
    ManuallyTripped = 3,
    BreakerTripped = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    OrderOccupied { order: u32 },
    BreakerNotFound { breaker_id: u32 },
    ManuallyTripped,
    Tripped { breaker_id: u32 },
}

impl Error {
    #[must_use]
    pub const fn code(self) -> ErrorCode {
        match self {
            Self::OrderOccupied { .. } => ErrorCode::OrderOccupied,
            Self::BreakerNotFound { .. } => ErrorCode::BreakerNotFound,
            Self::ManuallyTripped => ErrorCode::ManuallyTripped,
            Self::Tripped { .. } => ErrorCode::BreakerTripped,
        }
    }
}

#[cfg(feature = "std")]
impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OrderOccupied { order } => {
                write!(f, "circuit breaker order already occupied: {order}")
            }
            Self::BreakerNotFound { breaker_id } => {
                write!(f, "circuit breaker not found: {breaker_id}")
            }
            Self::ManuallyTripped => write!(f, "circuit breaker manually tripped"),
            Self::Tripped { breaker_id } => write!(f, "circuit breaker tripped: {breaker_id}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
