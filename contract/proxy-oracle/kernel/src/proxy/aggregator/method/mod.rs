pub mod median;
pub mod priority;

use crate::*;

pub trait Aggregate<S> {
    fn sources(&self) -> Vec<&S>;
    fn aggregate(&self, prices: Vec<Option<Price>>) -> Result<Price, Error>;
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("length mismatch: expected {expected}, actual {actual}")]
    LengthMismatch { expected: usize, actual: usize },
    #[error("too few valid sources: expected {expected}, actual {actual}")]
    TooFewValidSources { expected: usize, actual: usize },
}
