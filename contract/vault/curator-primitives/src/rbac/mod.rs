//! RBAC (Role-Based Access Control) auth adapter for curator vaults.
//!
//! This module provides an RBAC implementation of the [`AuthAdapter`] trait
//! for curator vaults. It enforces role-based access control where different
//! roles have permission to perform different actions.
//!
//! # Roles
//!
//! - **Curator**: Curator-scoped actions, plus allocator-class operations
//! - **Sentinel**: Emergency backstop (used for pause and restriction updates)
//! - **Allocator**: Can manage allocations and refreshes
//! - **User**: Can deposit, withdraw, execute withdrawals

use alloc::vec::Vec;
use templar_vault_kernel::Address;

use crate::auth::{
    canonical_policy_class, ActionKind, AuthAdapter, AuthError, AuthPolicyClass, AuthResult,
};

/// Role types for RBAC.
#[templar_vault_macros::vault_derive(borsh, schemars, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "boundary", derive(near_sdk::BorshStorageKey))]
pub enum Role {
    /// Curator-scoped privileged actions (and allocator-class operations).
    Curator,
    /// Emergency backstop (used for pause and restriction updates).
    Sentinel,
    /// Can manage allocations and market operations.
    Allocator,
}

impl Role {
    #[cfg(not(target_arch = "wasm32"))]
    #[inline]
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Curator => "curator",
            Role::Sentinel => "sentinel",
            Role::Allocator => "allocator",
        }
    }
}

/// Role assignment for an address.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct RoleAssignment {
    /// The address with this role.
    pub address: Address,
    /// The assigned role.
    pub role: Role,
}

/// RBAC configuration for the vault.
#[templar_vault_macros::vault_derive]
#[derive(Clone, Default)]
pub struct RbacConfig {
    /// List of role assignments.
    pub assignments: Vec<RoleAssignment>,
    /// Whether the vault is paused.
    pub paused: bool,
}

impl RbacConfig {
    /// Create an RBAC configuration with a single curator.
    #[inline]
    #[must_use]
    pub fn with_curator(curator: Address) -> Self {
        Self {
            assignments: alloc::vec![RoleAssignment {
                address: curator,
                role: Role::Curator,
            }],
            paused: false,
        }
    }

    /// Add a role assignment.
    #[inline]
    pub fn add_role(&mut self, address: Address, role: Role) {
        // Remove any existing assignment for this address with the same role
        self.assignments
            .retain(|a| !(a.address == address && a.role == role));
        self.assignments.push(RoleAssignment { address, role });
    }

    /// Remove a role from an address.
    #[inline]
    pub fn remove_role(&mut self, address: &Address, role: Role) {
        self.assignments
            .retain(|assignment| !(assignment.address == *address && assignment.role == role));
    }

    /// Check if an address has a specific role.
    #[inline]
    #[must_use]
    pub fn has_role(&self, address: &Address, role: Role) -> bool {
        self.assignments
            .iter()
            .any(|assignment| assignment.address == *address && assignment.role == role)
    }

    #[inline]
    #[must_use]
    fn is_curator(&self, address: &Address) -> bool {
        self.has_role(address, Role::Curator)
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
    match canonical_policy_class(action) {
        AuthPolicyClass::Public => None,
        AuthPolicyClass::Sentinel => Some(Role::Sentinel),
        AuthPolicyClass::Allocator | AuthPolicyClass::AllocatorEmergency => Some(Role::Allocator),
        AuthPolicyClass::Curator => Some(Role::Curator),
    }
}

/// RBAC auth adapter implementation.
///
/// This adapter enforces role-based access control for curator vault actions.
/// It checks that the caller has the required role for each action type.
#[templar_vault_macros::vault_derive]
#[derive(Clone, Default)]
pub struct RbacAuth {
    /// RBAC configuration.
    pub config: RbacConfig,
}

impl RbacAuth {
    #[inline]
    fn is_allowed(&self, action: ActionKind, caller: &Address) -> bool {
        match canonical_policy_class(action) {
            AuthPolicyClass::Public => true,
            AuthPolicyClass::Sentinel => self.config.has_role(caller, Role::Sentinel),
            AuthPolicyClass::Allocator => {
                self.config.has_role(caller, Role::Allocator) || self.config.is_curator(caller)
            }
            AuthPolicyClass::AllocatorEmergency => {
                self.config.has_role(caller, Role::Allocator)
                    || self.config.has_role(caller, Role::Sentinel)
                    || self.config.is_curator(caller)
            }
            AuthPolicyClass::Curator => self.config.is_curator(caller),
        }
    }
}

impl AuthAdapter for RbacAuth {
    fn authorize(
        &self,
        action: ActionKind,
        caller: Address,
        _proof: Option<&[u8]>,
    ) -> AuthResult<()> {
        // Check if paused (allow pause action even when paused).
        // Public/user actions are blocked while paused.
        if self.config.paused && action != ActionKind::Pause && !action.is_privileged() {
            return Err(AuthError::VaultPaused);
        }

        if !self.is_allowed(action, &caller) {
            return Err(AuthError::MissingRole);
        }

        Ok(())
    }

    fn is_paused(&self) -> bool {
        self.config.paused
    }
}
