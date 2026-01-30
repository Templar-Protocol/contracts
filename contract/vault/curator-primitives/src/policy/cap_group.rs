//! Cap group enforcement for market allocation limits.
//!
//! Cap groups allow curators to define maximum allocation caps for groups of markets,
//! preventing over-concentration in correlated assets or strategies.
//!
//! # Example
//!
//! ```ignore
//! use templar_curator_primitives::policy::cap_group::*;
//!
//! // Create a cap group with 1000 absolute cap and 50% relative cap
//! let group = CapGroup::new()
//!     .with_absolute(1000)
//!     .with_relative(Wad::from_percent(50));
//!
//! // Check if we can allocate 200 more (current principal is 300)
//! let can_alloc = group.can_allocate(300, 200, 2000);
//! assert!(can_alloc);
//! ```

use alloc::string::String;
use core::num::NonZeroU128;
use derive_more::{Display, From, Into};
use templar_vault_kernel::Wad;
use typed_builder::TypedBuilder;

/// Identifier for a cap group.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, From, Into, Display)]
#[display("{_0}")]
pub struct CapGroupId(pub String);

impl CapGroupId {
    /// Create a new cap group ID from a string.
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq, Default, TypedBuilder)]
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
    /// Create a new unlimited cap group (no restrictions).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cap group with only an absolute cap.
    #[must_use]
    pub fn absolute_only(cap: u128) -> Self {
        Self {
            absolute_cap: NonZeroU128::new(cap),
            relative_cap: None,
        }
    }

    /// Create a cap group with only a relative cap (WAD value).
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

    /// Builder method: set absolute cap.
    #[must_use]
    pub fn with_absolute(mut self, cap: u128) -> Self {
        self.absolute_cap = NonZeroU128::new(cap);
        self
    }

    /// Builder method: set relative cap.
    #[must_use]
    pub fn with_relative(mut self, cap: Wad) -> Self {
        self.relative_cap = if cap.is_zero() { None } else { Some(cap) };
        self
    }

    /// Returns true if this cap group has no restrictions.
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

        let absolute = self
            .absolute_cap
            .map(|c| c.get())
            .unwrap_or(u128::MAX);

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
    pub fn can_allocate(
        &self,
        current_principal: u128,
        amount: u128,
        total_assets: u128,
    ) -> bool {
        if self.is_unlimited() {
            return true;
        }

        let effective_cap = self.effective_cap(total_assets);
        let new_principal = current_principal.saturating_add(amount);

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

        let new_principal = current_principal.saturating_add(amount);

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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
mod tests {
    use super::*;
    use alloc::vec;

    const WAD: u128 = 1_000_000_000_000_000_000_000_000;

    #[test]
    fn test_cap_group_unlimited() {
        let cap = CapGroup::new();
        assert!(cap.is_unlimited());
        assert!(cap.can_allocate(0, u128::MAX, 1000));
    }

    #[test]
    fn test_cap_group_absolute_only() {
        let cap = CapGroup::absolute_only(1000);
        assert!(!cap.is_unlimited());
        assert!(cap.absolute_cap.is_some());
        assert!(cap.relative_cap.is_none());

        // Can allocate up to cap
        assert!(cap.can_allocate(0, 1000, 10000));
        assert!(cap.can_allocate(500, 500, 10000));

        // Cannot exceed cap
        assert!(!cap.can_allocate(500, 501, 10000));
        assert!(!cap.can_allocate(1000, 1, 10000));
    }

    #[test]
    fn test_cap_group_relative_only() {
        // 50% relative cap
        let cap = CapGroup::relative_only(Wad::from(WAD / 2));
        assert!(!cap.is_unlimited());
        assert!(cap.absolute_cap.is_none());
        assert!(cap.relative_cap.is_some());

        // Total assets = 1000, effective cap = 500
        assert!(cap.can_allocate(0, 500, 1000));
        assert!(cap.can_allocate(200, 300, 1000));
        assert!(!cap.can_allocate(200, 301, 1000));
    }

    #[test]
    fn test_cap_group_both_caps() {
        // 1000 absolute, 50% relative
        let cap = CapGroup::new()
            .with_absolute(1000)
            .with_relative(Wad::from(WAD / 2));

        // With 3000 total assets, relative cap = 1500, but absolute = 1000
        assert!(cap.can_allocate(0, 1000, 3000));
        assert!(!cap.can_allocate(0, 1001, 3000));

        // With 1000 total assets, relative cap = 500, which is stricter
        assert!(cap.can_allocate(0, 500, 1000));
        assert!(!cap.can_allocate(0, 501, 1000));
    }

    #[test]
    fn test_compute_effective_cap() {
        let cap = CapGroup::new()
            .with_absolute(1000)
            .with_relative(Wad::from(WAD / 2));

        // When relative cap is stricter
        assert_eq!(cap.effective_cap(1000), 500);

        // When absolute cap is stricter
        assert_eq!(cap.effective_cap(3000), 1000);

        // Unlimited
        let unlimited = CapGroup::new();
        assert_eq!(unlimited.effective_cap(1000), u128::MAX);
    }

    #[test]
    fn test_enforce_cap_group_errors() {
        let cap = CapGroup::new()
            .with_absolute(1000)
            .with_relative(Wad::from(WAD / 2));

        // Exceeds absolute cap
        let result = cap.enforce(0, 1001, 3000);
        assert!(matches!(
            result,
            Err(CapGroupError::ExceedsAbsoluteCap { .. })
        ));

        // Exceeds relative cap (500 effective cap when total = 1000)
        let result = cap.enforce(0, 501, 1000);
        assert!(matches!(
            result,
            Err(CapGroupError::ExceedsRelativeCap { .. })
        ));
    }

    #[test]
    fn test_compute_available_capacity() {
        let cap = CapGroup::absolute_only(1000);

        assert_eq!(cap.available_capacity(0, 2000), 1000);
        assert_eq!(cap.available_capacity(300, 2000), 700);
        assert_eq!(cap.available_capacity(1000, 2000), 0);
        assert_eq!(cap.available_capacity(1500, 2000), 0); // Already over, saturates to 0
    }

    #[test]
    fn test_apply_and_remove_allocation() {
        let cap = CapGroup::absolute_only(1000);
        let record = CapGroupRecord::new(cap);

        let updated = record.apply_allocation(300);
        assert_eq!(updated.principal, 300);

        let reduced = updated.remove_allocation(100);
        assert_eq!(reduced.principal, 200);

        // Saturating subtraction
        let zero = reduced.remove_allocation(500);
        assert_eq!(zero.principal, 0);
    }

    #[test]
    fn test_validate_allocations() {
        let cap1 = CapGroupRecord::new(CapGroup::absolute_only(1000));
        let cap2 = CapGroupRecord::new(CapGroup::absolute_only(500));

        // Valid allocations
        let allocations = vec![(cap1.clone(), 500), (cap2.clone(), 300)];
        assert!(validate_allocations(&allocations, 2000).is_ok());

        // Invalid - second exceeds cap
        let invalid = vec![(cap1, 500), (cap2, 600)];
        assert!(validate_allocations(&invalid, 2000).is_err());
    }

    #[test]
    fn test_cap_group_record_methods() {
        let record = CapGroupRecord::new(CapGroup::absolute_only(1000));

        assert!(record.can_allocate(500, 2000));
        assert!(!record.can_allocate(1001, 2000));
        assert_eq!(record.available_capacity(2000), 1000);

        assert!(record.enforce(500, 2000).is_ok());
        assert!(record.enforce(1001, 2000).is_err());
    }

    #[test]
    fn test_zero_absolute_cap_is_unlimited() {
        let cap = CapGroup::absolute_only(0);
        // NonZeroU128::new(0) returns None, so this should be unlimited
        assert!(cap.absolute_cap.is_none());
    }

    proptest::proptest! {
        #[test]
        fn prop_available_capacity_matches_effective_cap(
            absolute in 0u128..=1_000_000_000_000u128,
            relative in 0u128..=WAD,
            current in 0u128..=1_000_000_000_000u128,
            total in 0u128..=1_000_000_000_000u128,
        ) {
            let cap = CapGroup::new()
                .with_absolute(absolute)
                .with_relative(Wad::from(relative));
            let effective = cap.effective_cap(total);
            let available = cap.available_capacity(current, total);

            if cap.is_unlimited() {
                proptest::prop_assert_eq!(available, u128::MAX);
            } else {
                proptest::prop_assert_eq!(available, effective.saturating_sub(current));
            }
        }
    }
}
