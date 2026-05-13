pub mod median;
pub mod priority;

use crate::Price;

pub trait Aggregate<S> {
    fn aggregate<I>(&self, prices: I) -> Result<Price, Error>
    where
        I: IntoIterator<Item = Option<Price>>,
        I::IntoIter: ExactSizeIterator<Item = Option<Price>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    LengthMismatch = 1,
    TooFewValidSources = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    LengthMismatch { expected: usize, actual: usize },
    TooFewValidSources { expected: usize, actual: usize },
}

impl Error {
    #[must_use]
    pub const fn code(self) -> ErrorCode {
        match self {
            Self::LengthMismatch { .. } => ErrorCode::LengthMismatch,
            Self::TooFewValidSources { .. } => ErrorCode::TooFewValidSources,
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LengthMismatch { expected, actual } => {
                write!(f, "length mismatch: expected {expected}, actual {actual}")
            }
            Self::TooFewValidSources { expected, actual } => {
                write!(
                    f,
                    "too few valid sources: expected {expected}, actual {actual}"
                )
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
