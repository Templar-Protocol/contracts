pub mod median;
pub mod priority;

use crate::*;

pub trait Aggregate<S> {
    fn aggregate<I>(&self, prices: I) -> Result<Price, Error>
    where
        I: IntoIterator<Item = Option<Price>>,
        I::IntoIter: ExactSizeIterator<Item = Option<Price>>;
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("length mismatch: expected {expected}, actual {actual}")]
    LengthMismatch { expected: usize, actual: usize },
    #[error("too few valid sources: expected {expected}, actual {actual}")]
    TooFewValidSources { expected: usize, actual: usize },
}
