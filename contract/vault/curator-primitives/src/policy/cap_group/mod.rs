//! Cap group enforcement for market allocation limits.

use alloc::string::String;
#[cfg(feature = "borsh-schema")]
use alloc::string::ToString;
use alloc::vec::Vec;
use core::str::FromStr;
#[cfg(not(target_arch = "wasm32"))]
use derive_more::Display;
use templar_vault_kernel::Wad;
use typed_builder::TypedBuilder;

#[templar_vault_macros::vault_derive(borsh, borsh_schema, schemars, serde)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Display))]
#[cfg_attr(not(target_arch = "wasm32"), display("{_0}"))]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapGroupId(String);

impl CapGroupId {
    const POLICY_STATE_SENTINEL: &'static str = "policy-state";

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub(crate) fn policy_state_sentinel() -> Self {
        Self(String::from(Self::POLICY_STATE_SENTINEL))
    }

    fn validate(value: &str) -> Result<(), CapGroupIdError> {
        const MAX_LEN: usize = 64;

        if value.is_empty() {
            return Err(CapGroupIdError::Empty);
        }

        if value.len() > MAX_LEN {
            return Err(CapGroupIdError::TooLong { max_len: MAX_LEN });
        }

        if !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        }) {
            return Err(CapGroupIdError::InvalidCharacter);
        }

        Ok(())
    }
}

impl TryFrom<String> for CapGroupId {
    type Error = CapGroupIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::validate(&value)?;
        Ok(Self(value))
    }
}

impl TryFrom<&str> for CapGroupId {
    type Error = CapGroupIdError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::validate(value)?;
        Ok(Self(String::from(value)))
    }
}

impl FromStr for CapGroupId {
    type Err = CapGroupIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl From<CapGroupId> for String {
    fn from(value: CapGroupId) -> Self {
        value.0
    }
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum CapGroupIdError {
    Empty,
    TooLong { max_len: usize },
    InvalidCharacter,
}

/// A cap group defines maximum allocation limits for a set of markets.
///
/// Caps are optional - `None` means no limit for that cap type.
/// When both caps are set, the effective cap is the minimum of the two.
#[templar_vault_macros::vault_derive(borsh, borsh_schema, schemars, serde)]
#[derive(Clone, PartialEq, Eq, Default, TypedBuilder)]
pub struct CapGroup {
    /// Absolute cap in underlying asset units.
    /// `None` means no absolute cap.
    #[builder(default, setter(transform = |cap: u128| Some(cap)))]
    absolute_cap: Option<u128>,
    /// Relative cap as a WAD fraction of total vault assets (1e18 = 100%).
    /// `None` means no relative cap.
    #[builder(default, setter(transform = |cap: Wad| Some(cap)))]
    relative_cap: Option<Wad>,
}

impl CapGroup {
    #[must_use]
    pub fn absolute_cap(&self) -> Option<u128> {
        self.absolute_cap
    }

    #[must_use]
    pub fn relative_cap(&self) -> Option<Wad> {
        self.relative_cap
    }

    pub fn set_absolute_cap(&mut self, absolute_cap: Option<u128>) {
        self.absolute_cap = absolute_cap;
    }

    pub fn set_relative_cap(&mut self, relative_cap: Option<Wad>) {
        self.relative_cap = relative_cap;
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

        let absolute = self.absolute_cap.unwrap_or(u128::MAX);

        let relative = self
            .relative_cap
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
        let Some(new_principal) = current_principal.checked_add(amount) else {
            return false;
        };
        new_principal <= self.effective_cap(total_assets)
    }

    /// Enforce cap group constraints on an allocation.
    pub fn enforce(
        &self,
        current_principal: u128,
        amount: u128,
        total_assets: u128,
    ) -> Result<(), CapGroupError> {
        let Some(new_principal) = current_principal.checked_add(amount) else {
            return Err(CapGroupError::Overflow {
                current_principal,
                requested: amount,
            });
        };

        if let Some(abs_cap) = self.absolute_cap {
            if new_principal > abs_cap {
                return Err(CapGroupError::ExceedsAbsoluteCap {
                    cap_group_id: None,
                    requested: amount,
                    current_principal,
                    absolute_cap: abs_cap,
                });
            }
        }

        if let Some(ref rel_cap) = self.relative_cap {
            let effective_cap = rel_cap
                .apply_floored(templar_vault_kernel::Number::from(total_assets))
                .as_u128_saturating();

            if new_principal > effective_cap {
                return Err(CapGroupError::ExceedsRelativeCap {
                    cap_group_id: None,
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
        self.effective_cap(total_assets)
            .saturating_sub(current_principal)
    }
}

/// Record tracking the state of a cap group.
#[templar_vault_macros::vault_derive(borsh, borsh_schema, schemars, serde)]
#[derive(Clone, Default)]
pub struct CapGroupRecord {
    /// The cap group configuration.
    pub cap: CapGroup,
    /// Current total principal allocated to markets in this group.
    pub principal: u128,
}

impl CapGroupRecord {
    /// Apply an allocation to a cap group record.
    pub fn apply_allocation(&self, amount: u128) -> Result<Self, CapGroupError> {
        let principal = self
            .principal
            .checked_add(amount)
            .ok_or(CapGroupError::Overflow {
                current_principal: self.principal,
                requested: amount,
            })?;

        Ok(Self {
            cap: self.cap.clone(),
            principal,
        })
    }

    /// Remove allocation from a cap group record.
    pub fn remove_allocation(&self, amount: u128) -> Result<Self, CapGroupError> {
        let principal = self
            .principal
            .checked_sub(amount)
            .ok_or(CapGroupError::Underflow {
                current_principal: self.principal,
                requested: amount,
            })?;

        Ok(Self {
            cap: self.cap.clone(),
            principal,
        })
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
        Self { cap, principal: 0 }
    }
}

/// Errors that can occur during cap group operations.
#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum CapGroupError {
    /// Allocation would exceed the absolute cap.
    ExceedsAbsoluteCap {
        cap_group_id: Option<CapGroupId>,
        requested: u128,
        current_principal: u128,
        absolute_cap: u128,
    },
    /// Allocation would exceed the relative cap.
    ExceedsRelativeCap {
        cap_group_id: Option<CapGroupId>,
        requested: u128,
        current_principal: u128,
        effective_cap: u128,
        total_assets: u128,
    },
    /// Cap group not found.
    NotFound {
        id: CapGroupId,
    },
    /// Arithmetic overflow when computing new principal.
    Overflow {
        current_principal: u128,
        requested: u128,
    },
    Underflow {
        current_principal: u128,
        requested: u128,
    },
    InconsistentRecord {
        id: CapGroupId,
    },
}

/// A cap-group governance update (shared across chains).
#[templar_vault_macros::vault_derive(borsh, borsh_schema, postcard, schemars, serde)]
#[derive(Clone, PartialEq, Eq)]
pub enum CapGroupUpdate {
    SetCap {
        cap_group_id: CapGroupId,
        new_cap: Option<u128>,
    },
    SetRelativeCap {
        cap_group_id: CapGroupId,
        new_relative_cap: Option<Wad>,
    },
    SetMembership {
        market_id: templar_vault_kernel::TargetId,
        cap_group_id: Option<CapGroupId>,
    },
}

/// Identifies a cap-group governance update for accept/revoke operations.
#[templar_vault_macros::vault_derive(borsh, borsh_schema, postcard, schemars, serde)]
#[derive(Clone, PartialEq, Eq)]
pub enum CapGroupUpdateKey {
    SetCap {
        cap_group_id: CapGroupId,
    },
    SetRelativeCap {
        cap_group_id: CapGroupId,
    },
    SetMembership {
        market_id: templar_vault_kernel::TargetId,
    },
}

impl CapGroupUpdate {
    /// Build the canonical accept/revoke key for this update.
    #[must_use]
    pub fn key(&self) -> CapGroupUpdateKey {
        self.into()
    }
}

impl From<&CapGroupUpdate> for CapGroupUpdateKey {
    fn from(value: &CapGroupUpdate) -> Self {
        match value {
            CapGroupUpdate::SetCap { cap_group_id, .. } => Self::SetCap {
                cap_group_id: cap_group_id.clone(),
            },
            CapGroupUpdate::SetRelativeCap { cap_group_id, .. } => Self::SetRelativeCap {
                cap_group_id: cap_group_id.clone(),
            },
            CapGroupUpdate::SetMembership { market_id, .. } => Self::SetMembership {
                market_id: *market_id,
            },
        }
    }
}

/// Validate a list of allocations against their cap groups.
///
/// # Arguments
/// * `allocations` - List of (cap_group_id, cap_group_record, allocation_amount) tuples
/// * `total_assets` - Total vault assets for relative cap calculation
///
/// # Returns
/// `Ok(())` if all allocations are valid, or the first error encountered.
///
/// Note: This function tracks cumulative allocations per cap group to detect
/// cases where multiple allocations to the same group would exceed the cap,
/// even if each individual allocation is valid against the original principal.
pub fn validate_allocations(
    allocations: &[(&CapGroupId, &CapGroupRecord, u128)],
    total_assets: u128,
) -> Result<(), CapGroupError> {
    let mut cumulative: Vec<(&CapGroupId, CapGroupRecord, u128)> = Vec::new();

    for (group_id, record, amount) in allocations {
        let existing = cumulative
            .iter_mut()
            .find(|(existing_group_id, _, _)| *existing_group_id == *group_id);

        let (_, canonical_record, prior_cumulative) = match existing {
            Some(existing) => existing,
            None => {
                cumulative.push((group_id, (*record).clone(), 0));
                cumulative.last_mut().unwrap()
            }
        };

        if canonical_record.principal != record.principal || canonical_record.cap != record.cap {
            return Err(CapGroupError::InconsistentRecord {
                id: (*group_id).clone(),
            });
        }

        let effective_principal = canonical_record
            .principal
            .checked_add(*prior_cumulative)
            .ok_or(CapGroupError::Overflow {
                current_principal: canonical_record.principal,
                requested: *prior_cumulative,
            })?;

        canonical_record
            .cap
            .enforce(effective_principal, *amount, total_assets)
            .map_err(|error| match error {
                CapGroupError::ExceedsAbsoluteCap {
                    requested,
                    current_principal,
                    absolute_cap,
                    ..
                } => CapGroupError::ExceedsAbsoluteCap {
                    cap_group_id: Some((*group_id).clone()),
                    requested,
                    current_principal,
                    absolute_cap,
                },
                CapGroupError::ExceedsRelativeCap {
                    requested,
                    current_principal,
                    effective_cap,
                    total_assets,
                    ..
                } => CapGroupError::ExceedsRelativeCap {
                    cap_group_id: Some((*group_id).clone()),
                    requested,
                    current_principal,
                    effective_cap,
                    total_assets,
                },
                other => other,
            })?;

        *prior_cumulative =
            prior_cumulative
                .checked_add(*amount)
                .ok_or(CapGroupError::Overflow {
                    current_principal: *prior_cumulative,
                    requested: *amount,
                })?;
    }
    Ok(())
}
