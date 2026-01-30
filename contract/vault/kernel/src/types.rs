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
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, From, Into)]
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
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EscrowSettlement {
    /// Shares to burn (successfully redeemed).
    pub to_burn: u128,
    /// Shares to refund (excess or on failure).
    pub refund: u128,
}

impl EscrowSettlement {
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
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, From, Into)]
pub struct KernelVersion(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_id_from_bytes() {
        let bytes = [42u8; 32];
        let id = AssetId::from_bytes(bytes);
        assert_eq!(id.0, bytes);
    }

    #[test]
    fn asset_id_as_bytes() {
        let bytes = [99u8; 32];
        let id = AssetId(bytes);
        assert_eq!(id.as_bytes(), bytes);
    }

    #[test]
    fn asset_id_roundtrip() {
        let bytes = [123u8; 32];
        let id = AssetId::from_bytes(bytes);
        assert_eq!(id.as_bytes(), bytes);
    }

    #[test]
    fn escrow_settlement_burn_all() {
        let s = EscrowSettlement::burn_all(100);
        assert_eq!(s.to_burn, 100);
        assert_eq!(s.refund, 0);
    }

    #[test]
    fn escrow_settlement_refund_all() {
        let s = EscrowSettlement::refund_all(100);
        assert_eq!(s.to_burn, 0);
        assert_eq!(s.refund, 100);
    }

    #[test]
    fn escrow_settlement_partial() {
        let s = EscrowSettlement::partial(60, 40);
        assert_eq!(s.to_burn, 60);
        assert_eq!(s.refund, 40);
    }

    #[test]
    fn kernel_version_from_into() {
        let v: KernelVersion = 42u32.into();
        assert_eq!(v.0, 42);
        let n: u32 = v.into();
        assert_eq!(n, 42);
    }

    #[test]
    fn asset_id_from_into() {
        let bytes = [1u8; 32];
        let id: AssetId = bytes.into();
        assert_eq!(id.0, bytes);
        let out: [u8; 32] = id.into();
        assert_eq!(out, bytes);
    }
}
