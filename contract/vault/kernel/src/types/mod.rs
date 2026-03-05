//! Chain-agnostic types for the vault kernel.
//!
//! These types are designed to be portable across NEAR and Soroban.

use derive_more::{From, Into};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Timestamp in nanoseconds (u64).
pub type TimestampNs = u64;

/// Expected index in a queue or plan.
pub type ExpectedIdx = u32;

/// Actual index reached during processing.
pub type ActualIdx = u32;

/// Canonical address bytes (32 bytes).
/// Executors map chain-native account identifiers to this form (sha256 hash).
pub type Address = [u8; 32];

/// Asset identifier as a fixed 32-byte hash.
/// Executors map chain-native asset identifiers (e.g., NEAR account id)
/// to this form (sha256 hash) and maintain the mapping.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, From, Into)]
pub struct AssetId(pub [u8; 32]);

impl AssetId {
    /// Create an AssetId from raw bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return the raw bytes for this AssetId.
    pub const fn as_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl AsRef<[u8; 32]> for AssetId {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AsRef<[u8]> for AssetId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Settlement result for escrowed shares.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EscrowSettlement {
    /// Shares to burn (successfully redeemed).
    pub to_burn: u128,
    /// Shares to refund (excess or on failure).
    pub refund: u128,
}

impl EscrowSettlement {
    /// Create a settlement from escrowed shares and intended burned shares.
    ///
    /// Burned shares are clamped to `escrow_shares`, and the remainder is refunded.
    pub fn from_escrow_and_burn(escrow_shares: u128, burn_shares: u128) -> Self {
        let to_burn = burn_shares.min(escrow_shares);
        let refund = escrow_shares.saturating_sub(to_burn);
        Self { to_burn, refund }
    }

    /// Create a settlement that burns all shares.
    pub fn burn_all(shares: u128) -> Self {
        Self {
            to_burn: shares,
            refund: 0,
        }
    }

    /// Create a settlement that refunds all shares.
    pub fn refund_all(shares: u128) -> Self {
        Self {
            to_burn: 0,
            refund: shares,
        }
    }

    /// Create a settlement with partial burn.
    pub fn partial(to_burn: u128, refund: u128) -> Self {
        Self { to_burn, refund }
    }
}

/// Kernel version identifier.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, From, Into)]
pub struct KernelVersion(pub u32);

#[cfg(test)]
mod tests;
