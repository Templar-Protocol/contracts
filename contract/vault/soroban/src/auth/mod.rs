//! Authentication and authorization adapters.
//!
//! This module re-exports chain-agnostic auth types from `templar-curator-primitives`
//! and provides the Soroban-specific [`SorobanAuth`] adapter that integrates with
//! Soroban's native `require_auth()` authentication.

use soroban_sdk::{Address as SdkAddress, Env};

pub use templar_curator_primitives::auth::{
    allowed_while_paused, canonical_policy_class, ActionKind, AuthAdapter, AuthError,
    AuthPolicyClass, AuthResult,
};
pub use templar_curator_primitives::rbac::Role;

/// Soroban native authentication adapter.
///
/// This adapter integrates with Soroban's native authentication using
/// `require_auth()`. It verifies that callers have signed the transaction.
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
    /// Optional sentinel address (emergency backstop).
    sentinel: Option<SdkAddress>,
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
    pub(crate) fn has_role(&self, role: Role, caller: &SdkAddress) -> bool {
        match role {
            Role::Curator => caller == &self.curator,
            Role::Sentinel => Self::is_curator_or(caller, &self.sentinel, &self.curator),
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
            sentinel: None,
            allocator: None,
        }
    }

    /// Create a new Soroban auth adapter with RBAC roles.
    #[inline]
    #[must_use]
    pub fn with_roles(
        env: &'a Env,
        curator: SdkAddress,
        sentinel: Option<SdkAddress>,
        allocator: Option<SdkAddress>,
    ) -> Self {
        Self {
            env,
            curator,
            paused: false,
            sentinel,
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
    /// Returns `AuthError::VaultPaused` if the vault is paused and the action is
    /// outside the shared paused-action whitelist.
    pub fn verify_and_authorize(&self, action: ActionKind, caller: &SdkAddress) -> AuthResult<()> {
        // Verify the caller has signed the transaction
        caller.require_auth();

        // Check role-based permissions
        self.check_role(action, caller)
    }

    /// Check role-based permissions without calling require_auth.
    ///
    /// Uses the shared paused whitelist and canonical action policy class, then
    /// checks Soroban-specific role holders.
    pub fn check_role(&self, action: ActionKind, caller: &SdkAddress) -> AuthResult<()> {
        if self.paused && !allowed_while_paused(action) {
            return Err(AuthError::VaultPaused);
        }

        let policy_class = canonical_policy_class(action);
        let has_role = match policy_class {
            AuthPolicyClass::Public => true,
            AuthPolicyClass::Sentinel => self.has_role(Role::Sentinel, caller),
            AuthPolicyClass::Allocator => self.has_role(Role::Allocator, caller),
            AuthPolicyClass::AllocatorEmergency => {
                self.has_role(Role::Allocator, caller) || self.has_role(Role::Sentinel, caller)
            }
            AuthPolicyClass::Curator => self.has_role(Role::Curator, caller),
        };

        if has_role {
            Ok(())
        } else {
            Err(AuthError::MissingRole {
                action,
                policy_class,
            })
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
