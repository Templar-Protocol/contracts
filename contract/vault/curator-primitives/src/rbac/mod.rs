//! RBAC (Role-Based Access Control) auth adapter for curator vaults.
//!
//! This module provides an RBAC implementation of the [`AuthAdapter`] trait
//! for curator vaults. It enforces role-based access control where different
//! roles have permission to perform different actions.
//!
//! # Roles
//!
//! - **Curator**: Full control over the vault, including role management
//! - **Guardian**: Can pause/unpause the vault
//! - **Sentinel**: Emergency backstop, distinct from guardian (used by NEAR)
//! - **Allocator**: Can manage allocations and refreshes
//! - **User**: Can deposit, withdraw, execute withdrawals

use alloc::vec::Vec;
use templar_vault_kernel::Address;

use crate::auth::{
    action_policy_class, ActionKind, AuthAdapter, AuthError, AuthPolicyClass, AuthPolicyProfile,
    AuthResult,
};

/// Role types for RBAC.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "boundary", derive(near_sdk::BorshStorageKey))]
pub enum Role {
    /// Full curator control.
    Curator,
    /// Can pause/unpause and perform emergency actions.
    Guardian,
    /// Emergency backstop, distinct from guardian.
    Sentinel,
    /// Can manage allocations and market operations.
    Allocator,
}

impl Role {
    #[cfg(not(target_arch = "wasm32"))]
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Curator => "curator",
            Role::Guardian => "guardian",
            Role::Sentinel => "sentinel",
            Role::Allocator => "allocator",
        }
    }
}

/// Role assignment for an address.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct RoleAssignment {
    /// The address with this role.
    pub address: Address,
    /// The assigned role.
    pub role: Role,
}

impl RoleAssignment {
    /// Create a new role assignment.
    #[inline]
    #[must_use]
    pub const fn new(address: Address, role: Role) -> Self {
        Self { address, role }
    }
}

/// RBAC configuration for the vault.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Default)]
pub struct RbacConfig {
    /// List of role assignments.
    pub assignments: Vec<RoleAssignment>,
    /// Whether the vault is paused.
    pub paused: bool,
}

impl RbacConfig {
    /// Create a new empty RBAC configuration.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an RBAC configuration with a single curator.
    #[inline]
    #[must_use]
    pub fn with_curator(curator: Address) -> Self {
        Self {
            assignments: alloc::vec![RoleAssignment::new(curator, Role::Curator)],
            paused: false,
        }
    }

    /// Add a role assignment.
    #[inline]
    pub fn add_role(&mut self, address: Address, role: Role) {
        // Remove any existing assignment for this address with the same role
        self.assignments
            .retain(|a| !(a.address == address && a.role == role));
        self.assignments.push(RoleAssignment::new(address, role));
    }

    /// Remove a role from an address.
    #[inline]
    pub fn remove_role(&mut self, address: &Address, role: Role) {
        self.assignments
            .retain(|a| !(&a.address == address && a.role == role));
    }

    /// Check if an address has a specific role.
    #[inline]
    #[must_use]
    pub fn has_role(&self, address: &Address, role: Role) -> bool {
        self.assignments
            .iter()
            .any(|a| &a.address == address && a.role == role)
    }

    /// Get all roles for an address.
    #[must_use]
    pub fn get_roles(&self, address: &Address) -> Vec<Role> {
        self.assignments
            .iter()
            .filter(|a| &a.address == address)
            .map(|a| a.role)
            .collect()
    }

    /// Set the paused state.
    #[inline]
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }
}

/// Get the required role for an action.
///
/// This is the canonical action-to-role mapping shared across all executors.
/// Returns `None` for user-facing actions that don't require a special role.
#[inline]
#[must_use]
pub fn required_role(action: ActionKind) -> Option<Role> {
    match action_policy_class(action, AuthPolicyProfile::Canonical) {
        AuthPolicyClass::Public => None,
        AuthPolicyClass::Guardian => Some(Role::Guardian),
        AuthPolicyClass::Allocator | AuthPolicyClass::AllocatorEmergency => Some(Role::Allocator),
        AuthPolicyClass::Curator => Some(Role::Curator),
    }
}

/// RBAC auth adapter implementation.
///
/// This adapter enforces role-based access control for curator vault actions.
/// It checks that the caller has the required role for each action type.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone)]
pub struct RbacAuth {
    /// RBAC configuration.
    pub config: RbacConfig,
}

impl RbacAuth {
    /// Create a new RBAC auth adapter.
    #[inline]
    #[must_use]
    pub fn new(config: RbacConfig) -> Self {
        Self { config }
    }

    /// Create an RBAC auth adapter with a single curator.
    #[inline]
    #[must_use]
    pub fn with_curator(curator: Address) -> Self {
        Self::new(RbacConfig::with_curator(curator))
    }

    /// Check if the caller has the required role or is a curator.
    fn has_required_role(&self, caller: &Address, required: Role) -> bool {
        // Curator can do anything
        if self.config.has_role(caller, Role::Curator) {
            return true;
        }

        // Check for the specific role
        self.config.has_role(caller, required)
    }
}

impl Default for RbacAuth {
    fn default() -> Self {
        Self::new(RbacConfig::new())
    }
}

impl AuthAdapter for RbacAuth {
    fn authorize(
        &self,
        action: ActionKind,
        caller: Address,
        _proof: Option<&[u8]>,
    ) -> AuthResult<()> {
        // Check if paused (allow pause action even when paused)
        if self.config.paused && action != ActionKind::Pause {
            // Only allow user to read/check state when paused, but not deposit/withdraw
            if !action.is_privileged(AuthPolicyProfile::Canonical) {
                return Err(AuthError::VaultPaused);
            }
            // Allow curator to unpause and perform privileged recovery actions
            if !self.config.has_role(&caller, Role::Curator) {
                return Err(AuthError::VaultPaused);
            }
        }

        // Check role requirements
        if let Some(required_role) = required_role(action) {
            if !self.has_required_role(&caller, required_role) {
                return Err(AuthError::MissingRole);
            }
        }

        Ok(())
    }

    fn is_paused(&self) -> bool {
        self.config.paused
    }
}

#[cfg(test)]
mod tests;
