pub mod median_low;
pub mod priority;

use crate::{oracle::pyth, time::Nanoseconds};

use super::source::Source;

pub trait AggregationMethod {
    fn sources(&self) -> Vec<&Source>;
    fn aggregate(
        &self,
        prices: &[Option<pyth::Price>],
        now: Nanoseconds,
    ) -> Result<pyth::Price, Error>;
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("too few valid sources: expected {expected}, actual {actual}")]
    TooFewValidSources { expected: usize, actual: usize },
}
