pub static PREFIX: &str = "ed25519:";

mod public_key;
pub use public_key::*;
mod signature;
pub use signature::*;
