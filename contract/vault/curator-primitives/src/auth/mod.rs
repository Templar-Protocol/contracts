//! Chain-agnostic authentication and authorization primitives.
//!
//! This module provides a pluggable auth surface so curator and strategy vaults
//! can share the runtime while using different authorization mechanisms.
//!
//! The core trait [`AuthAdapter`] allows each chain executor to implement its own
//! signature verification while sharing the same action kinds and error types.

use templar_vault_kernel::{Address, KernelAction};

/// Shared authorization class for an action.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AuthPolicyClass {
    /// User-facing/public action (no special role requirement).
    Public,
    /// Sentinel/emergency-governance privileged action.
    Sentinel,
    /// Allocator-level privileged action.
    Allocator,
    /// Emergency allocator path (allocator + emergency role on some executors).
    AllocatorEmergency,
    /// Curator/owner-only privileged action.
    Curator,
}

/// Canonical shared policy class for an action.
#[inline]
#[must_use]
pub const fn canonical_policy_class(action: ActionKind) -> AuthPolicyClass {
    match action {
        ActionKind::Deposit
        | ActionKind::RequestWithdraw
        | ActionKind::AtomicWithdraw
        | ActionKind::AtomicRedeem => AuthPolicyClass::Public,
        #[cfg(feature = "action-epoch-settlement")]
        ActionKind::CancelPendingWithdrawal => AuthPolicyClass::Public,
        ActionKind::ExecuteWithdraw
        | ActionKind::BeginAllocating
        | ActionKind::FinishAllocating
        | ActionKind::SyncExternalAssets
        | ActionKind::RebalanceWithdraw
        | ActionKind::BeginRefreshing
        | ActionKind::FinishRefreshing
        | ActionKind::SettlePayout
        | ActionKind::RefreshFees => AuthPolicyClass::Allocator,
        #[cfg(feature = "action-epoch-settlement")]
        ActionKind::BeginEpochCutoff
        | ActionKind::SettleEpoch
        | ActionKind::AdmitPendingDeposit => AuthPolicyClass::Allocator,
        ActionKind::Pause | ActionKind::SetRestrictions => AuthPolicyClass::Sentinel,
        ActionKind::AbortAllocating
        | ActionKind::AbortWithdrawing
        | ActionKind::AbortRefreshing => AuthPolicyClass::AllocatorEmergency,
        #[cfg(feature = "action-epoch-settlement")]
        ActionKind::SeedEpochSupply => AuthPolicyClass::Curator,
        ActionKind::ManualReconcile | ActionKind::EmergencyReset | ActionKind::PolicyAdmin => {
            AuthPolicyClass::Curator
        }
    }
}

/// Boundary executor policy class for an action.
#[inline]
#[must_use]
pub const fn boundary_policy_class(action: ActionKind) -> AuthPolicyClass {
    canonical_policy_class(action)
}

#[inline]
#[must_use]
pub const fn allowed_while_paused(action: ActionKind) -> bool {
    #[cfg(feature = "action-epoch-settlement")]
    if matches!(action, ActionKind::CancelPendingWithdrawal) {
        return true;
    }
    #[cfg(feature = "action-epoch-settlement")]
    if matches!(action, ActionKind::SeedEpochSupply) {
        return true;
    }
    matches!(
        action,
        ActionKind::Pause
            | ActionKind::SetRestrictions
            | ActionKind::AbortAllocating
            | ActionKind::AbortWithdrawing
            | ActionKind::AbortRefreshing
            | ActionKind::ManualReconcile
            | ActionKind::EmergencyReset
    )
}

/// Kinds of actions that require authorization.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// User deposit action.
    Deposit,
    /// User withdraw request.
    RequestWithdraw,
    /// Execute pending withdrawal.
    ExecuteWithdraw,
    /// Move the open epoch to cutoff and block new settlement intake.
    #[cfg(feature = "action-epoch-settlement")]
    BeginEpochCutoff,
    /// Settle a cutoff epoch from an accepted valuation report.
    #[cfg(feature = "action-epoch-settlement")]
    SettleEpoch,
    /// Admit an already-custodied deposit against an immutable epoch snapshot.
    #[cfg(feature = "action-epoch-settlement")]
    AdmitPendingDeposit,
    /// Cancel an unsettled withdrawal and refund its escrowed shares.
    #[cfg(feature = "action-epoch-settlement")]
    CancelPendingWithdrawal,
    /// Pause/unpause the vault.
    Pause,
    /// Set kernel restrictions (pause/allowlist/denylist).
    SetRestrictions,
    /// Curator-only policy/state administration outside kernel restrictions.
    PolicyAdmin,
    /// Begin allocation operation.
    BeginAllocating,
    /// Finish allocation operation.
    FinishAllocating,
    /// Sync external assets.
    SyncExternalAssets,
    RebalanceWithdraw,
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
    /// Atomic withdraw (by assets, idle-only fast path).
    AtomicWithdraw,
    AtomicRedeem,
    /// Governed one-time backed seed for a fresh epoch deployment.
    #[cfg(feature = "action-epoch-settlement")]
    SeedEpochSupply,
}

impl ActionKind {
    /// Returns true if this action requires privileged access under the canonical policy.
    #[inline]
    #[must_use]
    pub const fn is_privileged(&self) -> bool {
        !matches!(canonical_policy_class(*self), AuthPolicyClass::Public)
    }
}

impl From<&KernelAction> for ActionKind {
    #[inline]
    fn from(action: &KernelAction) -> Self {
        match action {
            KernelAction::BeginAllocating { .. } => Self::BeginAllocating,
            KernelAction::Deposit { .. } => Self::Deposit,
            KernelAction::AtomicWithdraw { .. } => Self::AtomicWithdraw,
            KernelAction::AtomicRedeem { .. } => Self::AtomicRedeem,
            KernelAction::RequestWithdraw { .. } => Self::RequestWithdraw,
            KernelAction::ExecuteWithdraw { .. } => Self::ExecuteWithdraw,
            KernelAction::BeginRefreshing { .. } => Self::BeginRefreshing,
            KernelAction::FinishAllocating { .. } => Self::FinishAllocating,
            KernelAction::SyncExternalAssets { .. } => Self::SyncExternalAssets,
            KernelAction::RebalanceWithdraw { .. } => Self::RebalanceWithdraw,
            KernelAction::FinishRefreshing { .. } => Self::FinishRefreshing,
            KernelAction::AbortRefreshing { .. } => Self::AbortRefreshing,
            KernelAction::SettlePayout { .. } => Self::SettlePayout,
            KernelAction::AbortAllocating { .. } => Self::AbortAllocating,
            KernelAction::AbortWithdrawing { .. } => Self::AbortWithdrawing,
            KernelAction::RefreshFees { .. } => Self::RefreshFees,
            #[cfg(feature = "action-epoch-settlement")]
            KernelAction::BeginEpochCutoff { .. } => Self::BeginEpochCutoff,
            #[cfg(feature = "action-epoch-settlement")]
            KernelAction::SettleEpoch { .. } => Self::SettleEpoch,
            #[cfg(feature = "action-epoch-settlement")]
            KernelAction::AdmitPendingDeposit { .. } => Self::AdmitPendingDeposit,
            #[cfg(feature = "action-epoch-settlement")]
            KernelAction::CancelPendingWithdrawal { .. } => Self::CancelPendingWithdrawal,
            #[cfg(feature = "action-epoch-settlement")]
            KernelAction::SeedEpochSupply { .. } => Self::SeedEpochSupply,
            KernelAction::Pause { .. } => Self::Pause,
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

#[templar_vault_macros::vault_derive]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Admin,
    Curator,
    Sentinel,
    Allocator,
    User,
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum AuthError {
    NotAuthorized {
        caller: Caller,
        action: ActionKind,
    },
    InvalidProof,
    MissingRole {
        action: ActionKind,
        policy_class: AuthPolicyClass,
    },
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
