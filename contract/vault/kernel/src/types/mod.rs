//! Chain-agnostic types for the vault kernel.
//!
//! These types are designed to be portable across NEAR and Soroban.

#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(feature = "borsh-schema")]
use alloc::string::ToString;

use derive_more::{Display, From, Into};

/// Timestamp in nanoseconds.
#[repr(transparent)]
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde, postcard, schemars)]
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, From, Into, Display)]
#[display("{_0}")]
pub struct TimestampNs(pub u64);

impl TimestampNs {
    pub const ZERO: Self = Self(0);

    /// Create a timestamp from raw nanoseconds.
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Return the raw nanosecond value.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Saturating addition with another timestamp-like nanosecond delta.
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Saturating addition with a raw nanosecond delta.
    pub const fn saturating_add_u64(self, rhs: u64) -> Self {
        Self(self.0.saturating_add(rhs))
    }

    /// Saturating subtraction with another timestamp-like nanosecond value.
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

/// Expected index in a queue or plan.
#[repr(transparent)]
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde, postcard, schemars)]
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, From, Into, Display)]
#[display("{_0}")]
pub struct ExpectedIdx(pub u32);

/// Actual index reached during processing.
#[repr(transparent)]
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde, postcard, schemars)]
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, From, Into, Display)]
#[display("{_0}")]
pub struct ActualIdx(pub u32);

/// Canonical address bytes.
/// Executors map chain-native account identifiers to this form (sha256 hash).
#[repr(transparent)]
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde, postcard, schemars)]
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, From, Into)]
pub struct Address(pub [u8; 32]);

impl Address {
    /// Create an address from raw bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return the raw bytes for this address.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AsRef<[u8; 32]> for Address {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AsRef<[u8]> for Address {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Asset identifier as a fixed 32-byte hash.
/// Executors map chain-native asset identifiers (e.g., NEAR account id)
/// to this form (sha256 hash) and maintain the mapping.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, From, Into)]
pub struct AssetId(pub [u8; 32]);

impl AssetId {
    /// Create an AssetId from raw bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return the raw bytes for this AssetId.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
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
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
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
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, Copy, PartialEq, Eq, From, Into)]
pub struct KernelVersion(pub u32);

#[cfg(test)]
mod tests;
