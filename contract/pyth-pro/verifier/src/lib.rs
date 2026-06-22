//! Chain-agnostic verification and parsing for Pyth Pro (formerly Pyth Lazer) price updates.
//!
//! This crate wraps the upstream [`pyth_lazer_protocol`] wire-format parser (built with its
//! minimal, `default-features = false` feature set) and adds the trust checks an on-chain
//! adapter needs: an ed25519 signature check against a trusted, non-expired signer set, channel
//! filtering, and a timestamp freshness window. It targets Pyth Pro's **solana** delivery format
//! (ed25519 — NEAR's native scheme). It deliberately contains no chain-specific types — the host
//! (e.g. a NEAR contract) supplies a [`Crypto`] implementation and converts the neutral
//! [`VerifiedUpdate`] into its own storage/ABI types.

mod crypto;
mod error;
mod verify;

pub use crypto::Crypto;
pub use error::VerifyError;
pub use verify::{verify_solana_update, ParsedFeed, TrustedSigner, VerifiedUpdate, VerifyParams};
