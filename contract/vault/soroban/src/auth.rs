//! Authentication and authorization adapters.
//!
//! This module re-exports chain-agnostic auth types from `templar-curator-primitives`
//! and provides the Soroban-specific [`SorobanAuth`] adapter that integrates with
//! Soroban's native `require_auth()` authentication.

use alloc::string::String;
use soroban_sdk::{Address as SdkAddress, Env};

// Re-export chain-agnostic types from curator-primitives
pub use templar_curator_primitives::auth::{
    ActionKind, AuthAdapter, AuthError, AuthResult, PermissiveAuth, StrictAuth,
};
pub use templar_curator_primitives::rbac::{required_role, Role};

// ---------------------------------------------------------------------------
// Soroban Native Auth Adapter
// ---------------------------------------------------------------------------

/// Soroban native authentication adapter.
///
/// This adapter integrates with Soroban's native authentication using
/// `require_auth()`. It verifies that callers have signed the transaction
/// and optionally delegates to RBAC for role-based permission checks.
///
/// # Usage
///
/// ```ignore
/// use soroban_sdk::Env;
/// use templar_soroban_runtime::auth::{SorobanAuth, ActionKind};
///
/// let env = Env::default();
/// let auth = SorobanAuth::new(&env, admin_addr);
///
/// // This will call require_auth() on the caller
/// auth.verify_and_authorize(ActionKind::Deposit, &caller)?;
/// ```
pub struct SorobanAuth<'a> {
    /// The Soroban environment.
    env: &'a Env,
    /// The vault admin address (for privilege checks).
    admin: SdkAddress,
    /// Whether the vault is paused.
    paused: bool,
    /// Optional guardian address.
    guardian: Option<SdkAddress>,
    /// Optional allocator address.
    allocator: Option<SdkAddress>,
}

impl<'a> SorobanAuth<'a> {
    #[inline]
    #[must_use]
    fn is_admin_or(addr: &SdkAddress, delegated: &Option<SdkAddress>, admin: &SdkAddress) -> bool {
        addr == admin
            || delegated
                .as_ref()
                .is_some_and(|candidate| candidate == addr)
    }

    #[inline]
    #[must_use]
    fn has_role(&self, role: Role, caller: &SdkAddress) -> bool {
        match role {
            Role::Admin => self.is_admin(caller),
            Role::Guardian => self.is_guardian(caller),
            Role::Sentinel => self.is_admin(caller),
            Role::Allocator => self.is_allocator(caller),
        }
    }

    /// Create a new Soroban auth adapter.
    #[inline]
    #[must_use]
    pub fn new(env: &'a Env, admin: SdkAddress) -> Self {
        Self {
            env,
            admin,
            paused: false,
            guardian: None,
            allocator: None,
        }
    }

    /// Create a new Soroban auth adapter with RBAC roles.
    #[inline]
    #[must_use]
    pub fn with_roles(
        env: &'a Env,
        admin: SdkAddress,
        guardian: Option<SdkAddress>,
        allocator: Option<SdkAddress>,
    ) -> Self {
        Self {
            env,
            admin,
            paused: false,
            guardian,
            allocator,
        }
    }

    /// Set the paused state.
    #[inline]
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Check if an address is the admin.
    #[inline]
    #[must_use]
    pub fn is_admin(&self, addr: &SdkAddress) -> bool {
        addr == &self.admin
    }

    /// Check if an address is a guardian.
    #[inline]
    #[must_use]
    pub fn is_guardian(&self, addr: &SdkAddress) -> bool {
        Self::is_admin_or(addr, &self.guardian, &self.admin)
    }

    /// Check if an address is an allocator.
    #[inline]
    #[must_use]
    pub fn is_allocator(&self, addr: &SdkAddress) -> bool {
        Self::is_admin_or(addr, &self.allocator, &self.admin)
    }

    /// Verify caller signature and authorize an action.
    ///
    /// This is the primary entry point for Soroban contracts. It:
    /// 1. Calls `require_auth()` on the caller to verify their signature
    /// 2. Checks role-based permissions
    /// 3. Checks if the vault is paused
    ///
    /// # Errors
    ///
    /// Returns `AuthError::NotAuthorized` if the caller lacks the required role.
    /// Returns `AuthError::VaultPaused` if the vault is paused and action is not Pause.
    pub fn verify_and_authorize(&self, action: ActionKind, caller: &SdkAddress) -> AuthResult<()> {
        // Verify the caller has signed the transaction
        caller.require_auth();

        // Check if paused (allow pause action even when paused)
        if self.paused && action != ActionKind::Pause {
            // Only allow admin to perform actions when paused
            if !self.is_admin(caller) {
                return Err(AuthError::VaultPaused);
            }
        }

        // Check role-based permissions
        self.check_role(action, caller)
    }

    /// Check role-based permissions without calling require_auth.
    ///
    /// Delegates to the canonical `required_role()` mapping from
    /// curator-primitives, then checks the Soroban-specific role holders.
    pub fn check_role(&self, action: ActionKind, caller: &SdkAddress) -> AuthResult<()> {
        let role = match required_role(action) {
            None => return Ok(()),
            Some(r) => r,
        };

        let has_role = self.has_role(role, caller);

        if has_role {
            Ok(())
        } else {
            Err(AuthError::MissingRole(String::from(role.as_str())))
        }
    }

    /// Get the admin address.
    #[inline]
    #[must_use]
    pub fn admin(&self) -> &SdkAddress {
        &self.admin
    }

    /// Check if the vault is paused.
    #[inline]
    #[must_use]
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Get the environment reference.
    #[inline]
    #[must_use]
    pub fn env(&self) -> &Env {
        self.env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_soroban_auth_new() {
        let env = Env::default();
        let admin = SdkAddress::generate(&env);

        let auth = SorobanAuth::new(&env, admin.clone());

        assert_eq!(auth.admin(), &admin);
        assert!(!auth.paused());
    }

    #[test]
    fn test_soroban_auth_is_admin() {
        let env = Env::default();
        let admin = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::new(&env, admin.clone());

        assert!(auth.is_admin(&admin));
        assert!(!auth.is_admin(&user));
    }

    #[test]
    fn test_soroban_auth_is_guardian() {
        let env = Env::default();
        let admin = SdkAddress::generate(&env);
        let guardian = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, admin.clone(), Some(guardian.clone()), None);

        // Admin is always a guardian
        assert!(auth.is_guardian(&admin));
        // Designated guardian
        assert!(auth.is_guardian(&guardian));
        // Regular user is not
        assert!(!auth.is_guardian(&user));
    }

    #[test]
    fn test_soroban_auth_is_allocator() {
        let env = Env::default();
        let admin = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, admin.clone(), None, Some(allocator.clone()));

        // Admin is always an allocator
        assert!(auth.is_allocator(&admin));
        // Designated allocator
        assert!(auth.is_allocator(&allocator));
        // Regular user is not
        assert!(!auth.is_allocator(&user));
    }

    #[test]
    fn test_soroban_auth_check_role_user_actions() {
        let env = Env::default();
        let admin = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::new(&env, admin);

        // User actions allowed for anyone
        assert!(auth.check_role(ActionKind::Deposit, &user).is_ok());
        assert!(auth.check_role(ActionKind::RequestWithdraw, &user).is_ok());
        assert!(auth.check_role(ActionKind::ExecuteWithdraw, &user).is_ok());
    }

    #[test]
    fn test_soroban_auth_check_role_guardian_actions() {
        let env = Env::default();
        let admin = SdkAddress::generate(&env);
        let guardian = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, admin.clone(), Some(guardian.clone()), None);

        // Guardian can pause
        assert!(auth.check_role(ActionKind::Pause, &guardian).is_ok());
        // Admin can pause (admin is always guardian)
        assert!(auth.check_role(ActionKind::Pause, &admin).is_ok());
        // User cannot pause
        let result = auth.check_role(ActionKind::Pause, &user);
        assert!(matches!(result, Err(AuthError::MissingRole(_))));
    }

    #[test]
    fn test_soroban_auth_check_role_allocator_actions() {
        let env = Env::default();
        let admin = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, admin.clone(), None, Some(allocator.clone()));

        // Allocator can do allocation operations
        assert!(auth
            .check_role(ActionKind::BeginAllocating, &allocator)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::FinishAllocating, &allocator)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::BeginRefreshing, &allocator)
            .is_ok());
        assert!(auth
            .check_role(ActionKind::SyncExternalAssets, &allocator)
            .is_ok());

        // Admin can too
        assert!(auth.check_role(ActionKind::BeginAllocating, &admin).is_ok());

        // User cannot
        let result = auth.check_role(ActionKind::BeginAllocating, &user);
        assert!(matches!(result, Err(AuthError::MissingRole(_))));
    }

    #[test]
    fn test_soroban_auth_check_role_admin_only() {
        let env = Env::default();
        let admin = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, admin.clone(), None, Some(allocator.clone()));

        // Only admin can do manual reconcile
        assert!(auth.check_role(ActionKind::ManualReconcile, &admin).is_ok());

        // Allocator cannot
        let result = auth.check_role(ActionKind::ManualReconcile, &allocator);
        assert!(matches!(result, Err(AuthError::MissingRole(_))));
    }

    #[test]
    fn test_soroban_auth_set_paused() {
        let env = Env::default();
        let admin = SdkAddress::generate(&env);

        let mut auth = SorobanAuth::new(&env, admin);

        assert!(!auth.paused());
        auth.set_paused(true);
        assert!(auth.paused());
        auth.set_paused(false);
        assert!(!auth.paused());
    }
}
