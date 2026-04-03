pub mod median;
pub mod priority;

use crate::oracle::pyth;

use super::source::Source;

pub trait Aggregate {
    fn sources(&self) -> Vec<&Source>;
    fn aggregate(&self, prices: Vec<Option<pyth::Price>>) -> Result<pyth::Price, Error>;
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("too few valid sources: expected {expected}, actual {actual}")]
    TooFewValidSources { expected: usize, actual: usize },
}
