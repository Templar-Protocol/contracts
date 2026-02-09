//! RBAC (Role-Based Access Control) auth adapter for curator vaults.
//!
//! This module provides an RBAC implementation of the [`AuthAdapter`] trait
//! for curator vaults. It enforces role-based access control where different
//! roles have permission to perform different actions.
//!
//! # Roles
//!
//! - **Admin**: Full control over the vault, including role management
//! - **Guardian**: Can pause/unpause the vault
//! - **Sentinel**: Emergency backstop, distinct from guardian (used by NEAR)
//! - **Allocator**: Can manage allocations and refreshes
//! - **User**: Can deposit, withdraw, execute withdrawals

use alloc::string::String;
use alloc::vec::Vec;
use templar_vault_kernel::Address;

use crate::auth::{ActionKind, AuthAdapter, AuthError, AuthResult};

/// Role types for RBAC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Full administrative control.
    Admin,
    /// Can pause/unpause and perform emergency actions.
    Guardian,
    /// Emergency backstop, distinct from guardian.
    Sentinel,
    /// Can manage allocations and market operations.
    Allocator,
}

impl Role {
    /// Get the role name as a string.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Guardian => "guardian",
            Role::Sentinel => "sentinel",
            Role::Allocator => "allocator",
        }
    }
}

/// Role assignment for an address.
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, Default)]
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

    /// Create an RBAC configuration with a single admin.
    #[inline]
    #[must_use]
    pub fn with_admin(admin: Address) -> Self {
        Self {
            assignments: alloc::vec![RoleAssignment::new(admin, Role::Admin)],
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

    /// Check if an address is an admin.
    #[inline]
    #[must_use]
    pub fn is_admin(&self, address: &Address) -> bool {
        self.has_role(address, Role::Admin)
    }

    /// Check if an address is a guardian.
    #[inline]
    #[must_use]
    pub fn is_guardian(&self, address: &Address) -> bool {
        self.has_role(address, Role::Guardian)
    }

    /// Check if an address is a sentinel.
    #[inline]
    #[must_use]
    pub fn is_sentinel(&self, address: &Address) -> bool {
        self.has_role(address, Role::Sentinel)
    }

    /// Check if an address is an allocator.
    #[inline]
    #[must_use]
    pub fn is_allocator(&self, address: &Address) -> bool {
        self.has_role(address, Role::Allocator)
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
    match action {
        // User-facing actions don't require special roles
        ActionKind::Deposit | ActionKind::RequestWithdraw | ActionKind::ExecuteWithdraw => None,

        // Guardian actions
        ActionKind::Pause => Some(Role::Guardian),

        // Allocator actions
        ActionKind::BeginAllocating
        | ActionKind::FinishAllocating
        | ActionKind::SyncExternalAssets
        | ActionKind::BeginRefreshing
        | ActionKind::FinishRefreshing
        | ActionKind::AbortAllocating
        | ActionKind::AbortWithdrawing
        | ActionKind::AbortRefreshing
        | ActionKind::SettlePayout
        | ActionKind::RefreshFees => Some(Role::Allocator),

        // Admin-only actions
        ActionKind::ManualReconcile | ActionKind::SetRestrictions => Some(Role::Admin),
    }
}

/// RBAC auth adapter implementation.
///
/// This adapter enforces role-based access control for curator vault actions.
/// It checks that the caller has the required role for each action type.
#[derive(Clone, Debug)]
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

    /// Create an RBAC auth adapter with a single admin.
    #[inline]
    #[must_use]
    pub fn with_admin(admin: Address) -> Self {
        Self::new(RbacConfig::with_admin(admin))
    }

    /// Check if the caller has the required role or is an admin.
    fn has_required_role(&self, caller: &Address, required: Role) -> bool {
        // Admin can do anything
        if self.config.is_admin(caller) {
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
            if action.is_user_facing() {
                return Err(AuthError::VaultPaused);
            }
            // Allow admin to unpause and perform privileged recovery actions
            if !self.config.is_admin(&caller) {
                return Err(AuthError::VaultPaused);
            }
        }

        // Check role requirements
        if let Some(required_role) = required_role(action) {
            if !self.has_required_role(&caller, required_role) {
                return Err(AuthError::MissingRole(String::from(required_role.as_str())));
            }
        }

        Ok(())
    }

    fn is_paused(&self) -> bool {
        self.config.paused
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admin_addr() -> Address {
        [1u8; 32]
    }

    fn guardian_addr() -> Address {
        [2u8; 32]
    }

    fn allocator_addr() -> Address {
        [3u8; 32]
    }

    fn user_addr() -> Address {
        [4u8; 32]
    }

    fn sentinel_addr() -> Address {
        [5u8; 32]
    }

    fn test_rbac() -> RbacAuth {
        let mut config = RbacConfig::with_admin(admin_addr());
        config.add_role(guardian_addr(), Role::Guardian);
        config.add_role(allocator_addr(), Role::Allocator);
        RbacAuth::new(config)
    }

    #[test]
    fn test_role_assignment() {
        let config = RbacConfig::with_admin(admin_addr());

        assert!(config.is_admin(&admin_addr()));
        assert!(!config.is_admin(&user_addr()));
    }

    #[test]
    fn test_add_remove_role() {
        let mut config = RbacConfig::new();

        config.add_role(guardian_addr(), Role::Guardian);
        assert!(config.is_guardian(&guardian_addr()));

        config.remove_role(&guardian_addr(), Role::Guardian);
        assert!(!config.is_guardian(&guardian_addr()));
    }

    #[test]
    fn test_get_roles() {
        let mut config = RbacConfig::with_admin(admin_addr());
        config.add_role(admin_addr(), Role::Guardian); // Admin also guardian

        let roles = config.get_roles(&admin_addr());
        assert_eq!(roles.len(), 2);
        assert!(roles.contains(&Role::Admin));
        assert!(roles.contains(&Role::Guardian));
    }

    #[test]
    fn test_sentinel_role() {
        let mut config = RbacConfig::with_admin(admin_addr());
        config.add_role(sentinel_addr(), Role::Sentinel);

        assert!(config.is_sentinel(&sentinel_addr()));
        assert!(!config.is_sentinel(&user_addr()));
        assert!(!config.is_sentinel(&guardian_addr()));

        assert_eq!(Role::Sentinel.as_str(), "sentinel");

        let roles = config.get_roles(&sentinel_addr());
        assert_eq!(roles.len(), 1);
        assert!(roles.contains(&Role::Sentinel));
    }

    #[test]
    fn test_sentinel_add_remove() {
        let mut config = RbacConfig::new();

        config.add_role(sentinel_addr(), Role::Sentinel);
        assert!(config.is_sentinel(&sentinel_addr()));

        config.remove_role(&sentinel_addr(), Role::Sentinel);
        assert!(!config.is_sentinel(&sentinel_addr()));
    }

    #[test]
    fn test_user_actions_allowed() {
        let auth = test_rbac();

        // Any user can deposit
        assert!(auth
            .authorize(ActionKind::Deposit, user_addr(), None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::RequestWithdraw, user_addr(), None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::ExecuteWithdraw, user_addr(), None)
            .is_ok());
    }

    #[test]
    fn test_guardian_can_pause() {
        let auth = test_rbac();

        // Guardian can pause
        assert!(auth
            .authorize(ActionKind::Pause, guardian_addr(), None)
            .is_ok());

        // User cannot pause
        let result = auth.authorize(ActionKind::Pause, user_addr(), None);
        assert!(matches!(result, Err(AuthError::MissingRole(_))));
    }

    #[test]
    fn test_allocator_actions() {
        let auth = test_rbac();

        // Allocator can do allocation operations
        assert!(auth
            .authorize(ActionKind::BeginAllocating, allocator_addr(), None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::FinishAllocating, allocator_addr(), None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::SyncExternalAssets, allocator_addr(), None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::BeginRefreshing, allocator_addr(), None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::FinishRefreshing, allocator_addr(), None)
            .is_ok());

        // User cannot do allocation operations
        let result = auth.authorize(ActionKind::BeginAllocating, user_addr(), None);
        assert!(matches!(result, Err(AuthError::MissingRole(_))));
    }

    #[test]
    fn test_admin_can_do_everything() {
        let auth = test_rbac();

        // Admin can do all privileged actions
        assert!(auth
            .authorize(ActionKind::Pause, admin_addr(), None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::BeginAllocating, admin_addr(), None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::ManualReconcile, admin_addr(), None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::Deposit, admin_addr(), None)
            .is_ok());
    }

    #[test]
    fn test_manual_reconcile_admin_only() {
        let auth = test_rbac();

        // Only admin can do manual reconcile
        assert!(auth
            .authorize(ActionKind::ManualReconcile, admin_addr(), None)
            .is_ok());

        // Allocator cannot
        let result = auth.authorize(ActionKind::ManualReconcile, allocator_addr(), None);
        assert!(matches!(result, Err(AuthError::MissingRole(_))));

        // Guardian cannot
        let result = auth.authorize(ActionKind::ManualReconcile, guardian_addr(), None);
        assert!(matches!(result, Err(AuthError::MissingRole(_))));
    }

    #[test]
    fn test_paused_blocks_user_actions() {
        let mut auth = test_rbac();
        auth.config.set_paused(true);

        // User actions blocked
        let result = auth.authorize(ActionKind::Deposit, user_addr(), None);
        assert!(matches!(result, Err(AuthError::VaultPaused)));

        // Admin can still act when paused
        assert!(auth
            .authorize(ActionKind::BeginAllocating, admin_addr(), None)
            .is_ok());
    }

    #[test]
    fn test_paused_allows_pause_action() {
        let mut auth = test_rbac();
        auth.config.set_paused(true);

        // Guardian can still trigger pause action (to unpause)
        assert!(auth
            .authorize(ActionKind::Pause, guardian_addr(), None)
            .is_ok());
    }

    #[test]
    fn test_is_paused() {
        let mut auth = test_rbac();

        assert!(!auth.is_paused());

        auth.config.set_paused(true);
        assert!(auth.is_paused());
    }

    #[test]
    fn test_role_as_str() {
        assert_eq!(Role::Admin.as_str(), "admin");
        assert_eq!(Role::Guardian.as_str(), "guardian");
        assert_eq!(Role::Sentinel.as_str(), "sentinel");
        assert_eq!(Role::Allocator.as_str(), "allocator");
    }
}
