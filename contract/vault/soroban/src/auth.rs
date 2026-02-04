//! Authentication and authorization adapters.
//!
//! This module provides a pluggable auth surface so curator and strategy vaults
//! can share the runtime while using different authorization mechanisms.
//!
//! # Soroban Native Auth
//!
//! The [`SorobanAuth`] adapter integrates with Soroban's native authentication
//! using `require_auth()`. It verifies that callers have signed the transaction
//! and optionally delegates to RBAC for role-based permission checks.

use alloc::string::String;
use soroban_sdk::{Address as SdkAddress, Env};
use templar_vault_kernel::{Address, KernelAction};

/// Kinds of actions that require authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    /// User deposit action.
    Deposit,
    /// User withdraw request.
    RequestWithdraw,
    /// Execute pending withdrawal.
    ExecuteWithdraw,
    /// Pause/unpause the vault.
    Pause,
    /// Set kernel restrictions (pause/allowlist/denylist).
    SetRestrictions,
    /// Begin allocation operation.
    BeginAllocating,
    /// Finish allocation operation.
    FinishAllocating,
    /// Sync external assets.
    SyncExternalAssets,
    /// Begin refresh operation.
    BeginRefreshing,
    /// Finish refresh operation.
    FinishRefreshing,
    /// Abort allocation.
    AbortAllocating,
    /// Abort withdrawal.
    AbortWithdrawing,
    /// Abort refresh.
    AbortRefreshing,
    /// Settle payout.
    SettlePayout,
    /// Refresh fees.
    RefreshFees,
    /// Privileged manual reconciliation of external assets.
    ManualReconcile,
}

impl ActionKind {
    /// Returns true if this action is user-facing (can be called by any user).
    #[inline]
    #[must_use]
    pub const fn is_user_facing(&self) -> bool {
        matches!(
            self,
            ActionKind::Deposit | ActionKind::RequestWithdraw | ActionKind::ExecuteWithdraw
        )
    }

    /// Returns true if this action requires privileged access.
    #[inline]
    #[must_use]
    pub const fn is_privileged(&self) -> bool {
        !self.is_user_facing()
    }
}

impl From<&KernelAction> for ActionKind {
    fn from(action: &KernelAction) -> Self {
        match action {
            KernelAction::BeginAllocating { .. } => ActionKind::BeginAllocating,
            KernelAction::Deposit { .. } => ActionKind::Deposit,
            KernelAction::RequestWithdraw { .. } => ActionKind::RequestWithdraw,
            KernelAction::ExecuteWithdraw { .. } => ActionKind::ExecuteWithdraw,
            KernelAction::BeginRefreshing { .. } => ActionKind::BeginRefreshing,
            KernelAction::FinishAllocating { .. } => ActionKind::FinishAllocating,
            KernelAction::SyncExternalAssets { .. } => ActionKind::SyncExternalAssets,
            KernelAction::FinishRefreshing { .. } => ActionKind::FinishRefreshing,
            KernelAction::AbortRefreshing { .. } => ActionKind::AbortRefreshing,
            KernelAction::SettlePayout { .. } => ActionKind::SettlePayout,
            KernelAction::AbortAllocating { .. } => ActionKind::AbortAllocating,
            KernelAction::AbortWithdrawing { .. } => ActionKind::AbortWithdrawing,
            KernelAction::RefreshFees { .. } => ActionKind::RefreshFees,
            KernelAction::Pause { .. } => ActionKind::Pause,
        }
    }
}

impl From<KernelAction> for ActionKind {
    fn from(action: KernelAction) -> Self {
        ActionKind::from(&action)
    }
}

/// Authorization error details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// Caller is not authorized for this action.
    NotAuthorized { caller: Address, action: ActionKind },
    /// Invalid proof provided.
    InvalidProof,
    /// Missing required role.
    MissingRole(String),
    /// Vault is paused.
    VaultPaused,
}

/// Result type for auth operations.
pub type AuthResult<T> = Result<T, AuthError>;

/// Pluggable authorization adapter interface.
///
/// Curator vaults use RBAC checks while strategy vaults use Merkle proof
/// verification against a globally updatable root.
///
pub trait AuthAdapter {
    /// Authorize an action for a caller.
    ///
    fn authorize(
        &self,
        action: ActionKind,
        caller: Address,
        proof: Option<&[u8]>,
    ) -> AuthResult<()>;

    /// Check if the vault is currently paused.
    fn is_paused(&self) -> bool;
}

/// A permissive auth adapter that allows all actions (for testing).
#[derive(Clone, Copy, Debug, Default)]
pub struct PermissiveAuth;

impl AuthAdapter for PermissiveAuth {
    fn authorize(
        &self,
        _action: ActionKind,
        _caller: Address,
        _proof: Option<&[u8]>,
    ) -> AuthResult<()> {
        Ok(())
    }

    fn is_paused(&self) -> bool {
        false
    }
}

/// A strict auth adapter that denies all privileged actions (for testing).
#[derive(Clone, Copy, Debug, Default)]
pub struct StrictAuth {
    paused: bool,
}

impl StrictAuth {
    /// Create a new strict auth adapter.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self { paused: false }
    }

    /// Create a paused strict auth adapter.
    #[inline]
    #[must_use]
    pub const fn paused() -> Self {
        Self { paused: true }
    }
}

impl AuthAdapter for StrictAuth {
    fn authorize(
        &self,
        action: ActionKind,
        caller: Address,
        _proof: Option<&[u8]>,
    ) -> AuthResult<()> {
        if self.paused && action != ActionKind::Pause {
            return Err(AuthError::VaultPaused);
        }

        if action.is_privileged() {
            return Err(AuthError::NotAuthorized { caller, action });
        }

        Ok(())
    }

    fn is_paused(&self) -> bool {
        self.paused
    }
}

// ---------------------------------------------------------------------------
// Soroban Native Auth Adapter
// ---------------------------------------------------------------------------

/// Soroban native authentication adapter.
///
/// This adapter integrates with Soroban's native authentication using
/// `require_auth()`. It verifies that callers have signed the transaction
/// and optionally delegates to an inner RBAC adapter for role-based checks.
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
        self.is_admin(addr) || self.guardian.as_ref().is_some_and(|g| g == addr)
    }

    /// Check if an address is an allocator.
    #[inline]
    #[must_use]
    pub fn is_allocator(&self, addr: &SdkAddress) -> bool {
        self.is_admin(addr) || self.allocator.as_ref().is_some_and(|a| a == addr)
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
    pub fn verify_and_authorize(
        &self,
        action: ActionKind,
        caller: &SdkAddress,
    ) -> AuthResult<()> {
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
    /// Use this when auth has already been verified elsewhere.
    pub fn check_role(&self, action: ActionKind, caller: &SdkAddress) -> AuthResult<()> {
        match action {
            // User-facing actions don't require special roles
            ActionKind::Deposit | ActionKind::RequestWithdraw | ActionKind::ExecuteWithdraw => {
                Ok(())
            }

            // Guardian actions
            ActionKind::Pause => {
                if self.is_guardian(caller) {
                    Ok(())
                } else {
                    Err(AuthError::MissingRole(String::from("guardian")))
                }
            }

            // Admin-only actions
            ActionKind::SetRestrictions => {
                if self.is_admin(caller) {
                    Ok(())
                } else {
                    Err(AuthError::MissingRole(String::from("admin")))
                }
            }

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
            | ActionKind::RefreshFees => {
                if self.is_allocator(caller) {
                    Ok(())
                } else {
                    Err(AuthError::MissingRole(String::from("allocator")))
                }
            }

            // Admin-only actions
            ActionKind::ManualReconcile => {
                if self.is_admin(caller) {
                    Ok(())
                } else {
                    Err(AuthError::MissingRole(String::from("admin")))
                }
            }
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

    #[test]
    fn test_action_kind_user_facing() {
        assert!(ActionKind::Deposit.is_user_facing());
        assert!(ActionKind::RequestWithdraw.is_user_facing());
        assert!(ActionKind::ExecuteWithdraw.is_user_facing());

        assert!(!ActionKind::Pause.is_user_facing());
        assert!(!ActionKind::SetRestrictions.is_user_facing());
        assert!(!ActionKind::BeginAllocating.is_user_facing());
        assert!(!ActionKind::FinishAllocating.is_user_facing());
        assert!(!ActionKind::ManualReconcile.is_user_facing());
    }

    #[test]
    fn test_action_kind_privileged() {
        assert!(!ActionKind::Deposit.is_privileged());
        assert!(!ActionKind::RequestWithdraw.is_privileged());

        assert!(ActionKind::Pause.is_privileged());
        assert!(ActionKind::SetRestrictions.is_privileged());
        assert!(ActionKind::BeginAllocating.is_privileged());
        assert!(ActionKind::AbortAllocating.is_privileged());
        assert!(ActionKind::ManualReconcile.is_privileged());
    }

    #[test]
    fn test_permissive_auth() {
        let auth = PermissiveAuth;
        let caller = [0u8; 32];

        assert!(auth.authorize(ActionKind::Deposit, caller, None).is_ok());
        assert!(auth.authorize(ActionKind::Pause, caller, None).is_ok());
        assert!(auth
            .authorize(ActionKind::BeginAllocating, caller, None)
            .is_ok());
        assert!(!auth.is_paused());
    }

    #[test]
    fn test_strict_auth_allows_user_actions() {
        let auth = StrictAuth::new();
        let caller = [0u8; 32];

        assert!(auth.authorize(ActionKind::Deposit, caller, None).is_ok());
        assert!(auth
            .authorize(ActionKind::RequestWithdraw, caller, None)
            .is_ok());
        assert!(auth
            .authorize(ActionKind::ExecuteWithdraw, caller, None)
            .is_ok());
    }

    #[test]
    fn test_strict_auth_denies_privileged_actions() {
        let auth = StrictAuth::new();
        let caller = [0u8; 32];

        let result = auth.authorize(ActionKind::Pause, caller, None);
        assert!(matches!(result, Err(AuthError::NotAuthorized { .. })));

        let result = auth.authorize(ActionKind::BeginAllocating, caller, None);
        assert!(matches!(result, Err(AuthError::NotAuthorized { .. })));
    }

    #[test]
    fn test_strict_auth_paused() {
        let auth = StrictAuth::paused();
        let caller = [0u8; 32];

        assert!(auth.is_paused());

        // Pause action is allowed even when paused
        assert!(auth.authorize(ActionKind::Pause, caller, None).is_err()); // Still denied because privileged

        // User actions denied when paused
        let result = auth.authorize(ActionKind::Deposit, caller, None);
        assert!(matches!(result, Err(AuthError::VaultPaused)));
    }

    // -------------------------------------------------------------------------
    // SorobanAuth tests
    // -------------------------------------------------------------------------

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
        assert!(auth
            .check_role(ActionKind::BeginAllocating, &admin)
            .is_ok());

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
