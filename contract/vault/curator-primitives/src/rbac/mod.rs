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

use alloc::{collections::BTreeMap, vec::Vec};
use templar_vault_kernel::Address;

use crate::auth::{
    allowed_while_paused, canonical_policy_class, ActionKind, AuthAdapter, AuthError,
    AuthPolicyClass, AuthResult,
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct RoleSet(u8);

impl RoleSet {
    const NONE: Self = Self(0);
    const CURATOR: Self = Self(1 << 0);
    const SENTINEL: Self = Self(1 << 1);
    const ALLOCATOR: Self = Self(1 << 2);

    #[inline]
    const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[inline]
    const fn contains(self, role: Role) -> bool {
        let mask = match role {
            Role::Curator => Self::CURATOR.0,
            Role::Sentinel => Self::SENTINEL.0,
            Role::Allocator => Self::ALLOCATOR.0,
        };
        self.0 & mask != 0
    }
}

const fn allowed_roles_for(action: ActionKind) -> RoleSet {
    match canonical_policy_class(action) {
        AuthPolicyClass::Public => RoleSet::NONE,
        AuthPolicyClass::Sentinel => RoleSet::SENTINEL,
        AuthPolicyClass::Allocator => RoleSet::ALLOCATOR.union(RoleSet::CURATOR),
        AuthPolicyClass::AllocatorEmergency => RoleSet::ALLOCATOR
            .union(RoleSet::SENTINEL)
            .union(RoleSet::CURATOR),
        AuthPolicyClass::Curator => RoleSet::CURATOR,
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
    assignments_by_address: BTreeMap<Address, Vec<Role>>,
    /// Whether the vault is paused.
    paused: bool,
}

impl RbacConfig {
    /// Create an RBAC configuration with a single curator.
    #[inline]
    #[must_use]
    pub fn new(curator: Address) -> Self {
        let mut assignments_by_address = BTreeMap::new();
        assignments_by_address.insert(curator, alloc::vec![Role::Curator]);

        Self {
            assignments_by_address,
            paused: false,
        }
    }

    /// Create an RBAC configuration with a single curator.
    #[inline]
    #[must_use]
    pub fn with_curator(curator: Address) -> Self {
        Self::new(curator)
    }

    /// Add a role assignment.
    #[inline]
    pub fn add_role(&mut self, address: Address, role: Role) -> bool {
        let roles = self.assignments_by_address.entry(address).or_default();
        if roles.contains(&role) {
            return false;
        }

        roles.push(role);
        true
    }

    /// Remove a role from an address.
    #[inline]
    pub fn remove_role(&mut self, address: &Address, role: Role) -> bool {
        if role == Role::Curator && self.curator_count() == 1 && self.has_role(address, role) {
            return false;
        }

        let Some(roles) = self.assignments_by_address.get_mut(address) else {
            return false;
        };

        let original_len = roles.len();
        roles.retain(|existing_role| *existing_role != role);
        let changed = roles.len() != original_len;

        if roles.is_empty() {
            self.assignments_by_address.remove(address);
        }

        changed
    }

    /// Check if an address has a specific role.
    #[inline]
    #[must_use]
    pub fn has_role(&self, address: &Address, role: Role) -> bool {
        self.assignments_by_address
            .get(address)
            .is_some_and(|roles| roles.contains(&role))
    }

    #[inline]
    #[must_use]
    fn is_curator(&self, address: &Address) -> bool {
        self.has_role(address, Role::Curator)
    }

    #[inline]
    #[must_use]
    fn curator_count(&self) -> usize {
        self.assignments_by_address
            .values()
            .filter(|roles| roles.contains(&Role::Curator))
            .count()
    }

    /// Get all roles for an address.
    #[must_use]
    pub fn get_roles(&self, address: &Address) -> Vec<Role> {
        self.assignments_by_address
            .get(address)
            .cloned()
            .unwrap_or_default()
    }

    #[must_use]
    pub fn role_assignments(&self) -> Vec<RoleAssignment> {
        self.assignments_by_address
            .iter()
            .flat_map(|(address, roles)| {
                roles.iter().copied().map(|role| RoleAssignment {
                    address: *address,
                    role,
                })
            })
            .collect()
    }

    /// Set the paused state.
    #[inline]
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    #[inline]
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }
}

#[inline]
#[must_use]
pub fn allowed_roles_for_action(action: ActionKind) -> Vec<Role> {
    [Role::Curator, Role::Sentinel, Role::Allocator]
        .into_iter()
        .filter(|role| allowed_roles_for(action).contains(*role))
        .collect()
}

/// RBAC auth adapter implementation.
///
/// This adapter enforces role-based access control for curator vault actions.
/// It checks that the caller has the required role for each action type.
#[templar_vault_macros::vault_derive]
#[derive(Clone)]
pub struct RbacAuth {
    /// RBAC configuration.
    config: RbacConfig,
}

impl RbacAuth {
    #[inline]
    #[must_use]
    pub fn new(config: RbacConfig) -> Self {
        Self { config }
    }

    #[inline]
    #[must_use]
    pub fn config(&self) -> &RbacConfig {
        &self.config
    }

    #[inline]
    pub fn set_paused(&mut self, paused: bool) {
        self.config.set_paused(paused);
    }

    #[inline]
    fn is_allowed(&self, caller: &Address, allowed_roles: RoleSet) -> bool {
        allowed_roles == RoleSet::NONE
            || self
                .config
                .get_roles(caller)
                .into_iter()
                .any(|role| allowed_roles.contains(role))
    }
}

impl AuthAdapter for RbacAuth {
    fn authorize(
        &self,
        action: ActionKind,
        caller: Address,
        _proof: Option<&[u8]>,
    ) -> AuthResult<()> {
        if self.config.is_paused() && !allowed_while_paused(action) {
            return Err(AuthError::VaultPaused);
        }

        if !self.is_allowed(&caller, allowed_roles_for(action)) {
            return Err(AuthError::MissingRole {
                action,
                policy_class: canonical_policy_class(action),
            });
        }

        Ok(())
    }

    fn is_paused(&self) -> bool {
        self.config.is_paused()
    }
}
