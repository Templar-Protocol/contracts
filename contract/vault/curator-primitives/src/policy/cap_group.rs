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
//! let group = CapGroup::new(1000, 500_000_000_000_000_000_000_000); // 50% in WAD
//!
//! // Check if we can allocate 200 more (current principal is 300)
//! let can_alloc = can_allocate_to_group(&group, 300, 200, 2000);
//! assert!(can_alloc);
//! ```

use alloc::string::String;
use templar_vault_kernel::Wad;

/// Identifier for a cap group.
#[cfg_attr(
    feature = "near",
    derive(
        near_sdk::borsh::BorshSerialize,
        near_sdk::borsh::BorshDeserialize,
        serde::Serialize,
        serde::Deserialize
    )
)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapGroupId(pub String);

impl CapGroupId {
    /// Create a new cap group ID from a string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl From<String> for CapGroupId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for CapGroupId {
    fn from(value: &str) -> Self {
        Self(String::from(value))
    }
}

impl core::fmt::Display for CapGroupId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

/// A cap group defines maximum allocation limits for a set of markets.
#[cfg_attr(
    feature = "near",
    derive(
        near_sdk::borsh::BorshSerialize,
        near_sdk::borsh::BorshDeserialize,
        serde::Serialize,
        serde::Deserialize
    )
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapGroup {
    /// Absolute cap in underlying asset units.
    /// Zero means no absolute cap.
    pub absolute_cap: u128,
    /// Relative cap as a WAD fraction of total vault assets (1e24 = 100%).
    /// Zero means no relative cap.
    pub relative_cap: Wad,
}

impl CapGroup {
    /// Create a new cap group with the given absolute and relative caps.
    pub fn new(absolute_cap: u128, relative_cap_raw: u128) -> Self {
        Self {
            absolute_cap,
            relative_cap: Wad::from(relative_cap_raw),
        }
    }

    /// Create a cap group with only an absolute cap.
    pub fn absolute_only(cap: u128) -> Self {
        Self {
            absolute_cap: cap,
            relative_cap: Wad::zero(),
        }
    }

    /// Create a cap group with only a relative cap (WAD value).
    pub fn relative_only(relative_cap_raw: u128) -> Self {
        Self {
            absolute_cap: 0,
            relative_cap: Wad::from(relative_cap_raw),
        }
    }

    /// Create an unlimited cap group (no restrictions).
    pub fn unlimited() -> Self {
        Self {
            absolute_cap: 0,
            relative_cap: Wad::zero(),
        }
    }

    /// Returns true if this cap group has no restrictions.
    pub fn is_unlimited(&self) -> bool {
        self.absolute_cap == 0 && self.relative_cap.is_zero()
    }
}

impl Default for CapGroup {
    fn default() -> Self {
        Self::unlimited()
    }
}

/// Record tracking the state of a cap group.
#[cfg_attr(
    feature = "near",
    derive(
        near_sdk::borsh::BorshSerialize,
        near_sdk::borsh::BorshDeserialize,
        serde::Serialize,
        serde::Deserialize
    )
)]
#[derive(Clone, Debug, Default)]
pub struct CapGroupRecord {
    /// The cap group configuration.
    pub cap: CapGroup,
    /// Current total principal allocated to markets in this group.
    pub principal: u128,
}

impl CapGroupRecord {
    /// Create a new cap group record.
    pub fn new(cap: CapGroup) -> Self {
        Self { cap, principal: 0 }
    }

    /// Create a record with initial principal.
    pub fn with_principal(cap: CapGroup, principal: u128) -> Self {
        Self { cap, principal }
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

/// Compute the effective cap for a cap group given total vault assets.
///
/// The effective cap is the minimum of:
/// - The absolute cap (if set, i.e., non-zero)
/// - The relative cap applied to total assets (if set)
///
/// Returns `u128::MAX` if the cap group is unlimited.
pub fn compute_effective_cap(cap: &CapGroup, total_assets: u128) -> u128 {
    if cap.is_unlimited() {
        return u128::MAX;
    }

    let absolute = if cap.absolute_cap > 0 {
        cap.absolute_cap
    } else {
        u128::MAX
    };

    let relative = if !cap.relative_cap.is_zero() {
        cap.relative_cap
            .apply_floored(templar_vault_kernel::Number::from(total_assets))
            .as_u128_saturating()
    } else {
        u128::MAX
    };

    absolute.min(relative)
}

/// Check if an allocation is allowed under cap group constraints.
///
/// # Arguments
/// * `cap` - The cap group configuration
/// * `current_principal` - Current total allocated to the group
/// * `amount` - Amount to allocate
/// * `total_assets` - Total vault assets for relative cap calculation
///
/// # Returns
/// `true` if the allocation is allowed, `false` otherwise.
pub fn can_allocate_to_group(
    cap: &CapGroup,
    current_principal: u128,
    amount: u128,
    total_assets: u128,
) -> bool {
    if cap.is_unlimited() {
        return true;
    }

    let effective_cap = compute_effective_cap(cap, total_assets);
    let new_principal = current_principal.saturating_add(amount);

    new_principal <= effective_cap
}

/// Enforce cap group constraints on an allocation.
///
/// # Arguments
/// * `cap` - The cap group configuration
/// * `current_principal` - Current total allocated to the group
/// * `amount` - Amount to allocate
/// * `total_assets` - Total vault assets for relative cap calculation
///
/// # Returns
/// `Ok(())` if the allocation is allowed, `Err` with details otherwise.
pub fn enforce_cap_group(
    cap: &CapGroup,
    current_principal: u128,
    amount: u128,
    total_assets: u128,
) -> Result<(), CapGroupError> {
    if cap.is_unlimited() {
        return Ok(());
    }

    let new_principal = current_principal.saturating_add(amount);

    // Check absolute cap
    if cap.absolute_cap > 0 && new_principal > cap.absolute_cap {
        return Err(CapGroupError::ExceedsAbsoluteCap {
            requested: amount,
            current_principal,
            absolute_cap: cap.absolute_cap,
        });
    }

    // Check relative cap
    if !cap.relative_cap.is_zero() {
        let effective_cap = cap
            .relative_cap
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
///
/// # Arguments
/// * `cap` - The cap group configuration
/// * `current_principal` - Current total allocated to the group
/// * `total_assets` - Total vault assets for relative cap calculation
///
/// # Returns
/// The maximum amount that can still be allocated.
pub fn compute_available_capacity(
    cap: &CapGroup,
    current_principal: u128,
    total_assets: u128,
) -> u128 {
    if cap.is_unlimited() {
        return u128::MAX;
    }

    let effective_cap = compute_effective_cap(cap, total_assets);
    effective_cap.saturating_sub(current_principal)
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
        enforce_cap_group(&record.cap, record.principal, *amount, total_assets)?;
    }
    Ok(())
}

/// Apply an allocation to a cap group record.
///
/// This is a pure function that returns a new record with updated principal.
pub fn apply_allocation(record: &CapGroupRecord, amount: u128) -> CapGroupRecord {
    CapGroupRecord {
        cap: record.cap.clone(),
        principal: record.principal.saturating_add(amount),
    }
}

/// Remove allocation from a cap group record.
///
/// This is a pure function that returns a new record with reduced principal.
pub fn remove_allocation(record: &CapGroupRecord, amount: u128) -> CapGroupRecord {
    CapGroupRecord {
        cap: record.cap.clone(),
        principal: record.principal.saturating_sub(amount),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const WAD: u128 = 1_000_000_000_000_000_000_000_000;

    #[test]
    fn test_cap_group_unlimited() {
        let cap = CapGroup::unlimited();
        assert!(cap.is_unlimited());
        assert!(can_allocate_to_group(&cap, 0, u128::MAX, 1000));
    }

    #[test]
    fn test_cap_group_absolute_only() {
        let cap = CapGroup::absolute_only(1000);
        assert!(!cap.is_unlimited());

        // Can allocate up to cap
        assert!(can_allocate_to_group(&cap, 0, 1000, 10000));
        assert!(can_allocate_to_group(&cap, 500, 500, 10000));

        // Cannot exceed cap
        assert!(!can_allocate_to_group(&cap, 500, 501, 10000));
        assert!(!can_allocate_to_group(&cap, 1000, 1, 10000));
    }

    #[test]
    fn test_cap_group_relative_only() {
        // 50% relative cap
        let cap = CapGroup::relative_only(WAD / 2);
        assert!(!cap.is_unlimited());

        // Total assets = 1000, effective cap = 500
        assert!(can_allocate_to_group(&cap, 0, 500, 1000));
        assert!(can_allocate_to_group(&cap, 200, 300, 1000));
        assert!(!can_allocate_to_group(&cap, 200, 301, 1000));
    }

    #[test]
    fn test_cap_group_both_caps() {
        // 1000 absolute, 50% relative
        let cap = CapGroup::new(1000, WAD / 2);

        // With 3000 total assets, relative cap = 1500, but absolute = 1000
        assert!(can_allocate_to_group(&cap, 0, 1000, 3000));
        assert!(!can_allocate_to_group(&cap, 0, 1001, 3000));

        // With 1000 total assets, relative cap = 500, which is stricter
        assert!(can_allocate_to_group(&cap, 0, 500, 1000));
        assert!(!can_allocate_to_group(&cap, 0, 501, 1000));
    }

    #[test]
    fn test_compute_effective_cap() {
        let cap = CapGroup::new(1000, WAD / 2);

        // When relative cap is stricter
        assert_eq!(compute_effective_cap(&cap, 1000), 500);

        // When absolute cap is stricter
        assert_eq!(compute_effective_cap(&cap, 3000), 1000);

        // Unlimited
        let unlimited = CapGroup::unlimited();
        assert_eq!(compute_effective_cap(&unlimited, 1000), u128::MAX);
    }

    #[test]
    fn test_enforce_cap_group_errors() {
        let cap = CapGroup::new(1000, WAD / 2);

        // Exceeds absolute cap
        let result = enforce_cap_group(&cap, 0, 1001, 3000);
        assert!(matches!(
            result,
            Err(CapGroupError::ExceedsAbsoluteCap { .. })
        ));

        // Exceeds relative cap (500 effective cap when total = 1000)
        let result = enforce_cap_group(&cap, 0, 501, 1000);
        assert!(matches!(
            result,
            Err(CapGroupError::ExceedsRelativeCap { .. })
        ));
    }

    #[test]
    fn test_compute_available_capacity() {
        let cap = CapGroup::absolute_only(1000);

        assert_eq!(compute_available_capacity(&cap, 0, 2000), 1000);
        assert_eq!(compute_available_capacity(&cap, 300, 2000), 700);
        assert_eq!(compute_available_capacity(&cap, 1000, 2000), 0);
        assert_eq!(compute_available_capacity(&cap, 1500, 2000), 0); // Already over, saturates to 0
    }

    #[test]
    fn test_apply_and_remove_allocation() {
        let cap = CapGroup::absolute_only(1000);
        let record = CapGroupRecord::new(cap);

        let updated = apply_allocation(&record, 300);
        assert_eq!(updated.principal, 300);

        let reduced = remove_allocation(&updated, 100);
        assert_eq!(reduced.principal, 200);

        // Saturating subtraction
        let zero = remove_allocation(&reduced, 500);
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
}
