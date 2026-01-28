//! Chain-agnostic types for the vault kernel.
//!
//! These types are designed to be portable across NEAR and Soroban.

use alloc::string::String;

#[cfg(feature = "near")]
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "near")]
use serde::{Deserialize, Serialize};

/// Timestamp in nanoseconds (u64).
pub type TimestampNs = u64;

/// Expected index in a queue or plan.
pub type ExpectedIdx = u32;

/// Actual index reached during processing.
pub type ActualIdx = u32;

/// Generic actor identifier (account/address).
/// On NEAR: maps to AccountId
/// On Soroban: maps to Address
pub type ActorId = String;

/// Raw address as bytes (32 bytes).
pub type Address = [u8; 32];

/// Asset identifier - 32-byte hash or canonical identifier.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub [u8; 32]);

impl AssetId {
    /// Create an AssetId from raw bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

/// Settlement result for escrowed shares.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
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
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelVersion(pub u32);
