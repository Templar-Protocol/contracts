//! Fee structures for vault operations.
//!
//! Portable across NEAR and Soroban.
//!
//! This module provides two fee representation approaches:
//! - `Fee<T>` / `Fees<T>`: Generic types with string recipients for chain flexibility
//! - `FeeSlot` / `FeesSpec`: Spec-compliant types with fixed-size `Address` recipients
//!
//! Use `Fee<Wad>` for NEAR where `AccountId` is naturally a string.
//! Use `FeeSlot` when strict spec conformance with 32-byte addresses is required.

use alloc::string::String;

#[cfg(feature = "near")]
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "near")]
use serde::{Deserialize, Serialize};

use crate::math::wad::Wad;
use crate::types::Address;

// ============================================================================
// Generic Fee Types (String recipient - flexible)
// ============================================================================

/// A fee configuration with a rate and recipient.
///
/// This generic type uses a string recipient for maximum chain flexibility.
/// For spec-compliant 32-byte address recipients, see `FeeSlot`.
#[cfg_attr(
    feature = "near",
    derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize)
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fee<T> {
    /// The fee rate (interpretation depends on T).
    pub fee: T,
    /// The recipient identifier (account/address as string).
    pub recipient: String,
}

/// Collection of fees for a vault.
///
/// This generic type uses `Fee<T>` with string recipients.
/// For spec-compliant types, see `FeesSpec`.
#[cfg_attr(
    feature = "near",
    derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize)
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fees<T> {
    /// Performance fee (charged on profits).
    pub performance: Fee<T>,
    /// Management fee (charged over time).
    pub management: Fee<T>,
    /// Optional cap on how fast `total_assets` is allowed to grow for fee accrual.
    ///
    /// Interpreted as an annualized WAD rate (1e24 = 100% per year). When set,
    /// fee accrual uses `min(cur_total_assets, last_total_assets * (1 + max_rate * dt / YEAR))`
    /// as the effective `cur_total_assets`.
    pub max_total_assets_growth_rate: Option<T>,
}

// ============================================================================
// Spec-Compliant Fee Types (Address recipient - fixed size)
// ============================================================================

/// A fee slot with a WAD-scaled rate and 32-byte address recipient.
///
/// This type matches the kernel spec exactly:
/// - `fee_wad`: WAD-scaled fee rate (1e24 = 100%)
/// - `recipient`: 32-byte canonical address
///
/// The executor is responsible for mapping chain-native addresses to/from
/// this canonical 32-byte format.
#[cfg_attr(
    feature = "near",
    derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeSlot {
    /// The fee rate as a WAD value (1e24 = 100%).
    pub fee_wad: Wad,
    /// The recipient as a 32-byte canonical address.
    pub recipient: Address,
}

impl FeeSlot {
    /// Create a new fee slot.
    #[inline]
    #[must_use]
    pub fn new(fee_wad: Wad, recipient: Address) -> Self {
        Self { fee_wad, recipient }
    }

    /// Zero constant.
    pub const ZERO: Self = Self {
        fee_wad: Wad::ZERO,
        recipient: [0u8; 32],
    };

    /// Create a zero fee slot (no fee, zero address).
    #[inline]
    #[must_use]
    pub const fn zero() -> Self {
        Self::ZERO
    }

    /// Check if this fee slot has a zero rate.
    #[inline]
    #[must_use]
    pub fn is_zero_rate(&self) -> bool {
        self.fee_wad.is_zero()
    }
}

impl Default for FeeSlot {
    fn default() -> Self {
        Self::zero()
    }
}

/// Spec-compliant fee collection using `FeeSlot` with 32-byte addresses.
///
/// This type matches the kernel spec exactly and uses fixed-size addresses
/// for performance and predictable serialization.
#[cfg_attr(
    feature = "near",
    derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeesSpec {
    /// Performance fee (charged on profits).
    pub performance: FeeSlot,
    /// Management fee (charged over time).
    pub management: FeeSlot,
    /// Optional cap on total assets growth rate for fee accrual.
    ///
    /// Interpreted as an annualized WAD rate (1e24 = 100% per year).
    pub max_total_assets_growth_rate: Option<Wad>,
}

impl FeesSpec {
    /// Create a new fees configuration.
    #[inline]
    #[must_use]
    pub fn new(
        performance: FeeSlot,
        management: FeeSlot,
        max_total_assets_growth_rate: Option<Wad>,
    ) -> Self {
        Self {
            performance,
            management,
            max_total_assets_growth_rate,
        }
    }

    /// Zero constant.
    pub const ZERO: Self = Self {
        performance: FeeSlot::ZERO,
        management: FeeSlot::ZERO,
        max_total_assets_growth_rate: None,
    };

    /// Create a fees configuration with no fees.
    #[inline]
    #[must_use]
    pub const fn zero() -> Self {
        Self::ZERO
    }
}

impl Default for FeesSpec {
    fn default() -> Self {
        Self::zero()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fee_slot_zero() {
        let slot = FeeSlot::zero();
        assert!(slot.is_zero_rate());
        assert_eq!(slot.recipient, [0u8; 32]);
    }

    #[test]
    fn test_fee_slot_new() {
        let recipient = [1u8; 32];
        let slot = FeeSlot::new(Wad::one(), recipient);
        assert!(!slot.is_zero_rate());
        assert_eq!(slot.recipient, recipient);
    }

    #[test]
    fn test_fee_slot_default() {
        let slot = FeeSlot::default();
        assert!(slot.is_zero_rate());
        assert_eq!(slot.recipient, [0u8; 32]);
    }

    #[test]
    fn test_fees_spec_zero() {
        let fees = FeesSpec::zero();
        assert!(fees.performance.is_zero_rate());
        assert!(fees.management.is_zero_rate());
        assert!(fees.max_total_assets_growth_rate.is_none());
    }

    #[test]
    fn test_fees_spec_new() {
        let perf = FeeSlot::new(Wad::from(100_000_000_000_000_000_000_000u128), [1u8; 32]); // 10%
        let mgmt = FeeSlot::new(Wad::from(50_000_000_000_000_000_000_000u128), [2u8; 32]); // 5%
        let fees = FeesSpec::new(perf, mgmt, Some(Wad::one()));
        assert!(!fees.performance.is_zero_rate());
        assert!(!fees.management.is_zero_rate());
        assert!(fees.max_total_assets_growth_rate.is_some());
    }

    #[test]
    fn test_fees_spec_default() {
        let fees = FeesSpec::default();
        assert!(fees.performance.is_zero_rate());
        assert!(fees.management.is_zero_rate());
        assert!(fees.max_total_assets_growth_rate.is_none());
    }

    #[test]
    fn test_generic_fee_with_wad() {
        let fee: Fee<Wad> = Fee {
            fee: Wad::one(),
            recipient: alloc::string::String::from("test.near"),
        };
        assert_eq!(fee.fee, Wad::one());
        assert_eq!(fee.recipient, "test.near");
    }

    #[test]
    fn test_generic_fees_with_wad() {
        let fees: Fees<Wad> = Fees {
            performance: Fee {
                fee: Wad::from(100_000_000_000_000_000_000_000u128),
                recipient: alloc::string::String::from("perf.near"),
            },
            management: Fee {
                fee: Wad::from(50_000_000_000_000_000_000_000u128),
                recipient: alloc::string::String::from("mgmt.near"),
            },
            max_total_assets_growth_rate: None,
        };
        assert!(!fees.performance.fee.is_zero());
        assert!(!fees.management.fee.is_zero());
    }
}
