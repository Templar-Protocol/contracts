use std::error::Error;

pub mod ed25519;
pub mod ethereum;
pub mod p256;

#[derive(thiserror::Error, Debug)]
pub enum ParseError {
    #[error("Missing \"{0}\" prefix")]
    MissingPrefix(&'static str),
    #[error("Invalid encoding: {0}")]
    InvalidEncoding(#[from] Box<dyn Error>),
    #[error("Invalid: expected {expected}, got {actual}")]
    InvalidLength { expected: usize, actual: usize },
}
