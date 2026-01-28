//! Authentication and authorization adapters.
//!
//! This module provides a pluggable auth surface so curator and strategy vaults
//! can share the runtime while using different authorization mechanisms.

use alloc::string::String;
use templar_vault_kernel::Address;

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

/// Authorization error details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// Caller is not authorized for this action.
    NotAuthorized {
        caller: Address,
        action: ActionKind,
    },
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
/// # Example
///
/// ```ignore
/// impl AuthAdapter for RbacAdapter {
///     fn authorize(&self, action: ActionKind, caller: Address, proof: Option<&[u8]>) -> AuthResult<()> {
///         match action {
///             ActionKind::Pause => self.require_role(caller, Role::Guardian),
///             ActionKind::BeginAllocating => self.require_role(caller, Role::Allocator),
///             _ => Ok(()),
///         }
///     }
/// }
/// ```
pub trait AuthAdapter {
    /// Authorize an action for a caller.
    ///
    /// # Arguments
    ///
    /// * `action` - The kind of action being attempted.
    /// * `caller` - The address of the caller.
    /// * `proof` - Optional proof data (e.g., Merkle proof for strategy vaults).
    ///
    /// # Returns
    ///
    /// `Ok(())` if authorized, `Err(AuthError)` otherwise.
    fn authorize(&self, action: ActionKind, caller: Address, proof: Option<&[u8]>) -> AuthResult<()>;

    /// Check if the vault is currently paused.
    fn is_paused(&self) -> bool;
}

/// A permissive auth adapter that allows all actions (for testing).
#[derive(Clone, Copy, Debug, Default)]
pub struct PermissiveAuth;

impl AuthAdapter for PermissiveAuth {
    fn authorize(&self, _action: ActionKind, _caller: Address, _proof: Option<&[u8]>) -> AuthResult<()> {
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
    fn authorize(&self, action: ActionKind, caller: Address, _proof: Option<&[u8]>) -> AuthResult<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_kind_user_facing() {
        assert!(ActionKind::Deposit.is_user_facing());
        assert!(ActionKind::RequestWithdraw.is_user_facing());
        assert!(ActionKind::ExecuteWithdraw.is_user_facing());

        assert!(!ActionKind::Pause.is_user_facing());
        assert!(!ActionKind::BeginAllocating.is_user_facing());
        assert!(!ActionKind::FinishAllocating.is_user_facing());
    }

    #[test]
    fn test_action_kind_privileged() {
        assert!(!ActionKind::Deposit.is_privileged());
        assert!(!ActionKind::RequestWithdraw.is_privileged());

        assert!(ActionKind::Pause.is_privileged());
        assert!(ActionKind::BeginAllocating.is_privileged());
        assert!(ActionKind::AbortAllocating.is_privileged());
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
}
