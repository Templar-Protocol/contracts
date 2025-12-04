//! Chain-specific implementations for treasury management
//!
//! This module contains the NEAR handler implementation.
//! Cross-chain operations (deposits/withdrawals) are handled via NEAR Intents Bridge API.

pub mod near;

pub use near::NearHandler;
