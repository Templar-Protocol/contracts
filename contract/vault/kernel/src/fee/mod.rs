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

use crate::math::wad::Wad;
use crate::types::Address;

// Generic Fee Types (String recipient - flexible)

/// A fee configuration with a rate and recipient.
///
/// This generic type uses a string recipient for maximum chain flexibility.
/// For spec-compliant 32-byte address recipients, see `FeeSlot`.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq)]
pub struct Fee<T> {
    /// The fee rate (interpretation depends on T).
    pub fee: T,
    /// The recipient identifier (account/address as string).
    pub recipient: String,
}

impl<T> Fee<T> {
    /// Create a new generic fee entry.
    #[inline]
    #[must_use]
    pub fn new(fee: T, recipient: impl Into<String>) -> Self {
        Self {
            fee,
            recipient: recipient.into(),
        }
    }
}

/// Collection of fees for a vault.
///
/// This generic type uses `Fee<T>` with string recipients.
/// For spec-compliant types, see `FeesSpec`.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq)]
pub struct Fees<T> {
    /// Performance fee (charged on profits).
    pub performance: Fee<T>,
    /// Management fee (charged over time).
    pub management: Fee<T>,
    /// Optional cap on how fast `total_assets` is allowed to grow for fee accrual.
    ///
    /// Interpreted as an annualized WAD rate (1e18 = 100% per year). When set,
    /// fee accrual uses `min(cur_total_assets, last_total_assets * (1 + max_rate * dt / YEAR))`
    /// as the effective `cur_total_assets`.
    pub max_total_assets_growth_rate: Option<T>,
}

impl<T> Fees<T> {
    /// Create a new generic fees configuration.
    #[inline]
    #[must_use]
    pub const fn new(
        performance: Fee<T>,
        management: Fee<T>,
        max_total_assets_growth_rate: Option<T>,
    ) -> Self {
        Self {
            performance,
            management,
            max_total_assets_growth_rate,
        }
    }
}

// Spec-Compliant Fee Types (Address recipient - fixed size)

/// A fee slot with a WAD-scaled rate and 32-byte address recipient.
///
/// This type matches the kernel spec exactly:
/// - `fee_wad`: WAD-scaled fee rate (1e18 = 100%)
/// - `recipient`: 32-byte canonical address
///
/// The executor is responsible for mapping chain-native addresses to/from
/// this canonical 32-byte format.
#[templar_vault_macros::vault_derive(borsh)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FeeSlot {
    /// The fee rate as a WAD value (1e18 = 100%).
    pub fee_wad: Wad,
    /// The recipient as a 32-byte canonical address.
    pub recipient: Address,
}

impl FeeSlot {
    /// Create a new fee slot.
    #[inline]
    #[must_use]
    pub const fn new(fee_wad: Wad, recipient: Address) -> Self {
        Self { fee_wad, recipient }
    }

    pub const ZERO: Self = Self {
        fee_wad: Wad::ZERO,
        recipient: Address([0u8; 32]),
    };

    /// Create a zero fee slot (no fee, zero address).
    #[inline]
    #[must_use]
    pub const fn zero() -> Self {
        Self::ZERO
    }

    #[inline]
    #[must_use]
    pub fn has_rate(&self) -> bool {
        !self.fee_wad.is_zero()
    }

    /// Check if this fee slot has a zero rate.
    #[inline]
    #[must_use]
    pub fn is_zero_rate(&self) -> bool {
        !self.has_rate()
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
#[templar_vault_macros::vault_derive(borsh)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FeesSpec {
    /// Performance fee (charged on profits).
    pub performance: FeeSlot,
    /// Management fee (charged over time).
    pub management: FeeSlot,
    /// Optional cap on total assets growth rate for fee accrual.
    ///
    /// Interpreted as an annualized WAD rate (1e18 = 100% per year).
    pub max_total_assets_growth_rate: Option<Wad>,
}

impl FeesSpec {
    /// Create a new fees configuration.
    #[inline]
    #[must_use]
    pub const fn new(
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

    #[inline]
    #[must_use]
    pub fn has_active_slot_fees(&self) -> bool {
        self.performance.has_rate() || self.management.has_rate()
    }

    #[inline]
    #[must_use]
    pub fn has_growth_cap(&self) -> bool {
        self.max_total_assets_growth_rate.is_some()
    }

    /// Returns true when all fee fields are unset/zeroed.
    #[inline]
    #[must_use]
    pub fn is_zero(&self) -> bool {
        !self.has_active_slot_fees() && !self.has_growth_cap()
    }

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

#[cfg(all(feature = "serde", not(feature = "postcard")))]
mod serde_impl {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct FeeSlotSerde {
        fee_wad: Wad,
        recipient: Address,
    }

    impl Serialize for FeeSlot {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            FeeSlotSerde {
                fee_wad: self.fee_wad,
                recipient: self.recipient,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for FeeSlot {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let value = FeeSlotSerde::deserialize(deserializer)?;
            Ok(Self::new(value.fee_wad, value.recipient))
        }
    }

    #[derive(Serialize, Deserialize)]
    struct FeesSpecSerde {
        performance: FeeSlot,
        management: FeeSlot,
        max_total_assets_growth_rate: Option<Wad>,
    }

    impl Serialize for FeesSpec {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            FeesSpecSerde {
                performance: self.performance,
                management: self.management,
                max_total_assets_growth_rate: self.max_total_assets_growth_rate,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for FeesSpec {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let value = FeesSpecSerde::deserialize(deserializer)?;
            Ok(Self::new(
                value.performance,
                value.management,
                value.max_total_assets_growth_rate,
            ))
        }
    }
}

#[cfg(feature = "postcard")]
mod postcard_serde_impl {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    impl Serialize for FeeSlot {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            (&self.fee_wad, &self.recipient).serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for FeeSlot {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            <(Wad, Address)>::deserialize(deserializer)
                .map(|(fee_wad, recipient)| Self { fee_wad, recipient })
        }
    }

    impl Serialize for FeesSpec {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            (
                &self.performance,
                &self.management,
                &self.max_total_assets_growth_rate,
            )
                .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for FeesSpec {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            <(FeeSlot, FeeSlot, Option<Wad>)>::deserialize(deserializer).map(
                |(performance, management, max_total_assets_growth_rate)| Self {
                    performance,
                    management,
                    max_total_assets_growth_rate,
                },
            )
        }
    }
}

// Tests

#[cfg(test)]
mod tests;
