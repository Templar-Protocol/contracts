//! Chain-agnostic authentication and authorization primitives.
//!
//! This module provides a pluggable auth surface so curator and strategy vaults
//! can share the runtime while using different authorization mechanisms.
//!
//! The core trait [`AuthAdapter`] allows each chain executor to implement its own
//! signature verification while sharing the same action kinds and error types.

use templar_vault_kernel::{Address, KernelAction};

/// Shared auth policy profile used to classify action authorization behavior.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum AuthPolicyProfile {
    /// Canonical policy used by shared RBAC adapters.
    Canonical,
    /// Boundary executor policy (allocator-driven execute-withdraw and sentinel emergency paths).
    Boundary,
}

/// Shared authorization class for an action.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum AuthPolicyClass {
    /// User-facing/public action (no special role requirement).
    Public,
    /// Guardian-level privileged action.
    Guardian,
    /// Allocator-level privileged action.
    Allocator,
    /// Emergency allocator path (allocator + emergency role on some executors).
    AllocatorEmergency,
    /// Curator/owner-only privileged action.
    Curator,
}

/// Classify an action under a specific auth policy profile.
#[inline]
#[must_use]
pub const fn action_policy_class(
    action: ActionKind,
    profile: AuthPolicyProfile,
) -> AuthPolicyClass {
    match profile {
        AuthPolicyProfile::Canonical => canonical_policy_class(action),
        AuthPolicyProfile::Boundary => boundary_policy_class(action),
    }
}

/// Canonical shared policy class for an action.
#[inline]
#[must_use]
pub const fn canonical_policy_class(action: ActionKind) -> AuthPolicyClass {
    match action {
        ActionKind::Deposit | ActionKind::RequestWithdraw | ActionKind::ExecuteWithdraw => {
            AuthPolicyClass::Public
        }
        ActionKind::Pause => AuthPolicyClass::Guardian,
        ActionKind::BeginAllocating
        | ActionKind::FinishAllocating
        | ActionKind::SyncExternalAssets
        | ActionKind::BeginRefreshing
        | ActionKind::FinishRefreshing
        | ActionKind::AbortAllocating
        | ActionKind::AbortWithdrawing
        | ActionKind::AbortRefreshing
        | ActionKind::SettlePayout
        | ActionKind::RefreshFees => AuthPolicyClass::Allocator,
        ActionKind::ManualReconcile | ActionKind::SetRestrictions | ActionKind::EmergencyReset => {
            AuthPolicyClass::Curator
        }
    }
}

/// Boundary executor policy class for an action.
#[inline]
#[must_use]
pub const fn boundary_policy_class(action: ActionKind) -> AuthPolicyClass {
    match action {
        ActionKind::Deposit | ActionKind::RequestWithdraw => AuthPolicyClass::Public,
        ActionKind::ExecuteWithdraw
        | ActionKind::BeginAllocating
        | ActionKind::FinishAllocating
        | ActionKind::SyncExternalAssets
        | ActionKind::BeginRefreshing
        | ActionKind::FinishRefreshing
        | ActionKind::RefreshFees
        | ActionKind::SettlePayout => AuthPolicyClass::Allocator,
        ActionKind::AbortAllocating
        | ActionKind::AbortWithdrawing
        | ActionKind::AbortRefreshing => AuthPolicyClass::AllocatorEmergency,
        ActionKind::Pause | ActionKind::SetRestrictions => AuthPolicyClass::Guardian,
        ActionKind::ManualReconcile | ActionKind::EmergencyReset => AuthPolicyClass::Curator,
    }
}

/// Kinds of actions that require authorization.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
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
    /// Emergency reset to force-idle a stuck vault.
    EmergencyReset,
}

impl ActionKind {
    /// Returns this action's auth policy class under the provided profile.
    #[inline]
    #[must_use]
    pub const fn policy_class(&self, profile: AuthPolicyProfile) -> AuthPolicyClass {
        action_policy_class(*self, profile)
    }

    /// Returns true if this action requires privileged access under the provided profile.
    #[inline]
    #[must_use]
    pub const fn is_privileged(&self, profile: AuthPolicyProfile) -> bool {
        !matches!(self.policy_class(profile), AuthPolicyClass::Public)
    }
}

macro_rules! impl_action_kind_from_kernel_action {
    ($($variant:ident),+ $(,)?) => {
        impl From<&KernelAction> for ActionKind {
            #[inline]
            fn from(action: &KernelAction) -> Self {
                match action {
                    $(KernelAction::$variant { .. } => Self::$variant,)+
                    KernelAction::EmergencyReset => Self::EmergencyReset,
                }
            }
        }

        impl From<KernelAction> for ActionKind {
            #[inline]
            fn from(action: KernelAction) -> Self {
                Self::from(&action)
            }
        }
    };
}

impl_action_kind_from_kernel_action!(
    BeginAllocating,
    Deposit,
    RequestWithdraw,
    ExecuteWithdraw,
    BeginRefreshing,
    FinishAllocating,
    SyncExternalAssets,
    FinishRefreshing,
    AbortRefreshing,
    SettlePayout,
    AbortAllocating,
    AbortWithdrawing,
    RefreshFees,
    Pause,
);

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Admin,
    Curator,
    Guardian,
    Sentinel,
    Allocator,
    User,
}

impl From<Address> for Caller {
    fn from(_: Address) -> Self {
        Self::User
    }
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum AuthError {
    NotAuthorized { caller: Caller, action: ActionKind },
    InvalidProof,
    MissingRole,
    VaultPaused,
}

/// Result type for auth operations.
pub type AuthResult<T> = Result<T, AuthError>;

/// Pluggable authorization adapter interface.
///
/// Curator vaults use RBAC checks while strategy vaults use Merkle proof
/// verification against a globally updatable root.
pub trait AuthAdapter {
    /// Authorize an action for a caller.
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
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, Default)]
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
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, Default)]
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

        if action.is_privileged(AuthPolicyProfile::Canonical) {
            return Err(AuthError::NotAuthorized {
                caller: caller.into(),
                action,
            });
        }

        Ok(())
    }

    fn is_paused(&self) -> bool {
        self.paused
    }
}

#[cfg(test)]
mod tests;
