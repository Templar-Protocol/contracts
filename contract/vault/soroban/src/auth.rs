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
/// let auth = SorobanAuth::new(&env, curator_addr);
///
/// // This will call require_auth() on the caller
/// auth.verify_and_authorize(ActionKind::Deposit, &caller)?;
/// ```
pub struct SorobanAuth<'a> {
    /// The Soroban environment.
    env: &'a Env,
    /// The vault curator address (for privilege checks).
    curator: SdkAddress,
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
    fn is_curator_or(
        addr: &SdkAddress,
        delegated: &Option<SdkAddress>,
        curator: &SdkAddress,
    ) -> bool {
        addr == curator
            || delegated
                .as_ref()
                .is_some_and(|candidate| candidate == addr)
    }

    #[inline]
    #[must_use]
    fn has_role(&self, role: Role, caller: &SdkAddress) -> bool {
        match role {
            Role::Curator | Role::Sentinel => caller == &self.curator,
            Role::Guardian => Self::is_curator_or(caller, &self.guardian, &self.curator),
            Role::Allocator => Self::is_curator_or(caller, &self.allocator, &self.curator),
        }
    }

    /// Create a new Soroban auth adapter.
    #[inline]
    #[must_use]
    pub fn new(env: &'a Env, curator: SdkAddress) -> Self {
        Self {
            env,
            curator,
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
        curator: SdkAddress,
        guardian: Option<SdkAddress>,
        allocator: Option<SdkAddress>,
    ) -> Self {
        Self {
            env,
            curator,
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
            // Only allow curator to perform actions when paused
            if !self.has_role(Role::Curator, caller) {
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

    /// Get the curator address.
    #[inline]
    #[must_use]
    pub fn curator(&self) -> &SdkAddress {
        &self.curator
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
        let curator = SdkAddress::generate(&env);

        let auth = SorobanAuth::new(&env, curator.clone());

        assert_eq!(auth.curator(), &curator);
        assert!(!auth.paused());
    }

    #[test]
    fn test_soroban_auth_curator_role() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::new(&env, curator.clone());

        assert!(auth.has_role(Role::Curator, &curator));
        assert!(!auth.has_role(Role::Curator, &user));
    }

    #[test]
    fn test_soroban_auth_guardian_role() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let guardian = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, curator.clone(), Some(guardian.clone()), None);

        // Curator is always a guardian
        assert!(auth.has_role(Role::Guardian, &curator));
        // Designated guardian
        assert!(auth.has_role(Role::Guardian, &guardian));
        // Regular user is not
        assert!(!auth.has_role(Role::Guardian, &user));
    }

    #[test]
    fn test_soroban_auth_allocator_role() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));

        // Curator is always an allocator
        assert!(auth.has_role(Role::Allocator, &curator));
        // Designated allocator
        assert!(auth.has_role(Role::Allocator, &allocator));
        // Regular user is not
        assert!(!auth.has_role(Role::Allocator, &user));
    }

    #[test]
    fn test_soroban_auth_check_role_user_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::new(&env, curator);

        // User actions allowed for anyone
        assert!(auth.check_role(ActionKind::Deposit, &user).is_ok());
        assert!(auth.check_role(ActionKind::RequestWithdraw, &user).is_ok());
        assert!(auth.check_role(ActionKind::ExecuteWithdraw, &user).is_ok());
    }

    #[test]
    fn test_soroban_auth_check_role_guardian_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let guardian = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, curator.clone(), Some(guardian.clone()), None);

        // Guardian can pause
        assert!(auth.check_role(ActionKind::Pause, &guardian).is_ok());
        // Curator can pause (curator is always guardian)
        assert!(auth.check_role(ActionKind::Pause, &curator).is_ok());
        // User cannot pause
        let result = auth.check_role(ActionKind::Pause, &user);
        assert!(matches!(result, Err(AuthError::MissingRole(_))));
    }

    #[test]
    fn test_soroban_auth_check_role_allocator_actions() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);
        let user = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));

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

        // Curator can too
        assert!(auth
            .check_role(ActionKind::BeginAllocating, &curator)
            .is_ok());

        // User cannot
        let result = auth.check_role(ActionKind::BeginAllocating, &user);
        assert!(matches!(result, Err(AuthError::MissingRole(_))));
    }

    #[test]
    fn test_soroban_auth_check_role_curator_only() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);
        let allocator = SdkAddress::generate(&env);

        let auth = SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));

        // Only curator can do manual reconcile
        assert!(auth
            .check_role(ActionKind::ManualReconcile, &curator)
            .is_ok());

        // Allocator cannot
        let result = auth.check_role(ActionKind::ManualReconcile, &allocator);
        assert!(matches!(result, Err(AuthError::MissingRole(_))));
    }

    #[test]
    fn test_soroban_auth_set_paused() {
        let env = Env::default();
        let curator = SdkAddress::generate(&env);

        let mut auth = SorobanAuth::new(&env, curator);

        assert!(!auth.paused());
        auth.set_paused(true);
        assert!(auth.paused());
        auth.set_paused(false);
        assert!(!auth.paused());
    }
}
