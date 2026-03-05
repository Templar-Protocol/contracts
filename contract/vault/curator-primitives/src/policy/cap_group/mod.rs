//! Cap group enforcement for market allocation limits.

use alloc::string::String;
use core::num::NonZeroU128;
use derive_more::{Display, From, Into};
use templar_vault_kernel::Wad;
use typed_builder::TypedBuilder;

#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "borsh-schema", derive(borsh::BorshSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, From, Into, Display)]
#[display("{_0}")]
pub struct CapGroupId(pub String);

impl CapGroupId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl From<&str> for CapGroupId {
    fn from(value: &str) -> Self {
        Self(String::from(value))
    }
}

/// A cap group defines maximum allocation limits for a set of markets.
///
/// Caps are optional - `None` means no limit for that cap type.
/// When both caps are set, the effective cap is the minimum of the two.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "borsh-schema", derive(borsh::BorshSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq, Default, TypedBuilder)]
pub struct CapGroup {
    /// Absolute cap in underlying asset units.
    /// `None` means no absolute cap.
    #[builder(default, setter(transform = |cap: u128| NonZeroU128::new(cap)))]
    pub absolute_cap: Option<NonZeroU128>,
    /// Relative cap as a WAD fraction of total vault assets (1e24 = 100%).
    /// `None` means no relative cap.
    #[builder(default, setter(transform = |cap: Wad| if cap.is_zero() { None } else { Some(cap) }))]
    pub relative_cap: Option<Wad>,
}

impl CapGroup {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn absolute_only(cap: u128) -> Self {
        Self {
            absolute_cap: NonZeroU128::new(cap),
            relative_cap: None,
        }
    }

    #[must_use]
    pub fn relative_only(relative_cap: Wad) -> Self {
        let relative = if relative_cap.is_zero() {
            None
        } else {
            Some(relative_cap)
        };
        Self {
            absolute_cap: None,
            relative_cap: relative,
        }
    }

    #[must_use]
    pub fn with_absolute(mut self, cap: u128) -> Self {
        self.absolute_cap = NonZeroU128::new(cap);
        self
    }

    #[must_use]
    pub fn with_relative(mut self, cap: Wad) -> Self {
        self.relative_cap = if cap.is_zero() { None } else { Some(cap) };
        self
    }

    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.absolute_cap.is_none() && self.relative_cap.is_none()
    }

    /// Compute the effective cap for a cap group given total vault assets.
    ///
    /// Returns `u128::MAX` if the cap group is unlimited.
    #[must_use]
    pub fn effective_cap(&self, total_assets: u128) -> u128 {
        if self.is_unlimited() {
            return u128::MAX;
        }

        let absolute = self.absolute_cap.map(|c| c.get()).unwrap_or(u128::MAX);

        let relative = self
            .relative_cap
            .as_ref()
            .map(|cap| {
                cap.apply_floored(templar_vault_kernel::Number::from(total_assets))
                    .as_u128_saturating()
            })
            .unwrap_or(u128::MAX);

        absolute.min(relative)
    }

    /// Check if an allocation is allowed under cap group constraints.
    #[must_use]
    pub fn can_allocate(&self, current_principal: u128, amount: u128, total_assets: u128) -> bool {
        if self.is_unlimited() {
            return true;
        }

        let effective_cap = self.effective_cap(total_assets);
        // Use checked_add to detect overflow - overflow means exceeds any cap
        let new_principal = match current_principal.checked_add(amount) {
            Some(p) => p,
            None => return false, // Overflow always exceeds cap
        };

        new_principal <= effective_cap
    }

    /// Enforce cap group constraints on an allocation.
    pub fn enforce(
        &self,
        current_principal: u128,
        amount: u128,
        total_assets: u128,
    ) -> Result<(), CapGroupError> {
        if self.is_unlimited() {
            return Ok(());
        }

        // Use checked_add to detect overflow
        let new_principal = match current_principal.checked_add(amount) {
            Some(new_principal) => new_principal,
            None => {
                return Err(CapGroupError::Overflow {
                    current_principal,
                    requested: amount,
                })
            }
        };

        if let Some(abs_cap) = self.absolute_cap {
            if new_principal > abs_cap.get() {
                return Err(CapGroupError::ExceedsAbsoluteCap {
                    requested: amount,
                    current_principal,
                    absolute_cap: abs_cap.get(),
                });
            }
        }

        if let Some(ref rel_cap) = self.relative_cap {
            let effective_cap = rel_cap
                .apply_floored(templar_vault_kernel::Number::from(total_assets))
                .as_u128_saturating();

            if new_principal > effective_cap {
                return Err(CapGroupError::ExceedsRelativeCap {
                    requested: amount,
                    current_principal,
                    effective_cap,
                    total_assets,
                });
            }
        }

        Ok(())
    }

    /// Compute the maximum additional amount that can be allocated to a cap group.
    #[must_use]
    pub fn available_capacity(&self, current_principal: u128, total_assets: u128) -> u128 {
        if self.is_unlimited() {
            return u128::MAX;
        }

        let effective_cap = self.effective_cap(total_assets);
        effective_cap.saturating_sub(current_principal)
    }
}

/// Record tracking the state of a cap group.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "borsh-schema", derive(borsh::BorshSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Default)]
pub struct CapGroupRecord {
    /// The cap group configuration.
    pub cap: CapGroup,
    /// Current total principal allocated to markets in this group.
    pub principal: u128,
}

impl CapGroupRecord {
    /// Create a new cap group record.
    #[must_use]
    pub fn new(cap: CapGroup) -> Self {
        Self { cap, principal: 0 }
    }

    /// Create a record with initial principal.
    #[must_use]
    pub fn with_principal(cap: CapGroup, principal: u128) -> Self {
        Self { cap, principal }
    }

    /// Apply an allocation to a cap group record.
    #[must_use]
    pub fn apply_allocation(&self, amount: u128) -> Self {
        Self {
            cap: self.cap.clone(),
            principal: self.principal.saturating_add(amount),
        }
    }

    /// Remove allocation from a cap group record.
    #[must_use]
    pub fn remove_allocation(&self, amount: u128) -> Self {
        Self {
            cap: self.cap.clone(),
            principal: self.principal.saturating_sub(amount),
        }
    }

    /// Check if an allocation is allowed.
    #[must_use]
    pub fn can_allocate(&self, amount: u128, total_assets: u128) -> bool {
        self.cap.can_allocate(self.principal, amount, total_assets)
    }

    /// Enforce cap constraints.
    pub fn enforce(&self, amount: u128, total_assets: u128) -> Result<(), CapGroupError> {
        self.cap.enforce(self.principal, amount, total_assets)
    }

    /// Get available capacity.
    #[must_use]
    pub fn available_capacity(&self, total_assets: u128) -> u128 {
        self.cap.available_capacity(self.principal, total_assets)
    }
}

impl From<CapGroup> for CapGroupRecord {
    fn from(cap: CapGroup) -> Self {
        Self::new(cap)
    }
}

/// Errors that can occur during cap group operations.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum CapGroupError {
    /// Allocation would exceed the absolute cap.
    ExceedsAbsoluteCap {
        requested: u128,
        current_principal: u128,
        absolute_cap: u128,
    },
    /// Allocation would exceed the relative cap.
    ExceedsRelativeCap {
        requested: u128,
        current_principal: u128,
        effective_cap: u128,
        total_assets: u128,
    },
    /// Cap group not found.
    NotFound { id: CapGroupId },
    /// Arithmetic overflow when computing new principal.
    Overflow {
        current_principal: u128,
        requested: u128,
    },
}

/// A cap-group governance update (shared across chains).
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[cfg_attr(feature = "borsh-schema", derive(borsh::BorshSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[derive(Clone, PartialEq, Eq)]
pub enum CapGroupUpdate {
    SetCap {
        cap_group_id: CapGroupId,
        new_cap: u128,
    },
    SetRelativeCap {
        cap_group_id: CapGroupId,
        new_relative_cap_wad: u128,
    },
    SetMembership {
        market_id: templar_vault_kernel::TargetId,
        cap_group_id: Option<CapGroupId>,
    },
}

/// Validate a list of allocations against their cap groups.
///
/// # Arguments
/// * `allocations` - List of (cap_group_record, allocation_amount) pairs
/// * `total_assets` - Total vault assets for relative cap calculation
///
/// # Returns
/// `Ok(())` if all allocations are valid, or the first error encountered.
pub fn validate_allocations(
    allocations: &[(CapGroupRecord, u128)],
    total_assets: u128,
) -> Result<(), CapGroupError> {
    for (record, amount) in allocations {
        record.enforce(*amount, total_assets)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
