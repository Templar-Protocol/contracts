//! Authentication and authorization for the NEAR vault.
//!
//! Re-exports chain-agnostic auth types from `templar-curator-primitives`
//! and provides NEAR-specific authorization helpers that integrate with
//! NEAR's `Owner` + `Rbac` derive macros.

// Re-export chain-agnostic types from curator-primitives
pub use templar_curator_primitives::auth::{
    ActionKind, AuthAdapter, AuthError, AuthResult, PermissiveAuth, StrictAuth,
};

use super::*;

/// NEAR-specific auth patterns used across the vault.
///
/// Each variant encodes a specific combination of roles allowed to perform
/// an action. The Owner (contract singleton) always passes all checks.
///
/// Role hierarchy: Owner > Curator > Guardian/Sentinel > Allocator
/// Note: Curator implicitly has Allocator privileges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthPattern {
    /// Only the contract owner.
    OwnerOnly,
    /// Guardian or Owner.
    GuardianOrOwner,
    /// Guardian, Sentinel, or Owner.
    GuardianOrSentinelOrOwner,
    /// Curator or Owner.
    CuratorOrOwner,
    /// Curator, Sentinel, or Owner.
    CuratorOrSentinelOrOwner,
    /// Allocator, Curator, or Owner.
    Allocator,
    /// Allocator, Curator, Sentinel, or Owner.
    AllocatorOrSentinel,
}

impl AuthPattern {
    /// Require the caller to match this auth pattern. Panics if unauthorized.
    pub fn require(self) {
        let caller = env::predecessor_account_id();
        match self {
            AuthPattern::OwnerOnly => {
                Contract::require_owner();
            }
            AuthPattern::GuardianOrOwner => {
                if !Contract::has_role(&caller, &Role::Guardian) {
                    Contract::require_owner();
                }
            }
            AuthPattern::GuardianOrSentinelOrOwner => {
                if !Contract::has_role(&caller, &Role::Guardian)
                    && !Contract::has_role(&caller, &Role::Sentinel)
                {
                    Contract::require_owner();
                }
            }
            AuthPattern::CuratorOrOwner => {
                if !Contract::has_role(&caller, &Role::Curator) {
                    Contract::require_owner();
                }
            }
            AuthPattern::CuratorOrSentinelOrOwner => {
                if !Contract::has_role(&caller, &Role::Curator)
                    && !Contract::has_role(&caller, &Role::Sentinel)
                {
                    Contract::require_owner();
                }
            }
            AuthPattern::Allocator => {
                if !Contract::has_role(&caller, &Role::Allocator)
                    && !Contract::has_role(&caller, &Role::Curator)
                {
                    Contract::require_owner();
                }
            }
            AuthPattern::AllocatorOrSentinel => {
                if !Contract::has_role(&caller, &Role::Allocator)
                    && !Contract::has_role(&caller, &Role::Curator)
                    && !Contract::has_role(&caller, &Role::Sentinel)
                {
                    Contract::require_owner();
                }
            }
        }
    }
}

/// Map an operational `ActionKind` to the NEAR-specific `AuthPattern`.
///
/// NEAR's mapping differs from the curator-primitives defaults:
/// - `ExecuteWithdraw` is allocator-operated (not user-facing)
/// - Abort actions allow Sentinel in addition to Allocator
/// - `Pause`/`SetRestrictions` are guardian-level (handled via governance)
#[must_use]
pub fn auth_pattern_for(action: ActionKind) -> AuthPattern {
    match action {
        // User-facing — no privileged auth required
        ActionKind::Deposit | ActionKind::RequestWithdraw => AuthPattern::OwnerOnly,

        // In NEAR, execution is allocator-driven
        ActionKind::ExecuteWithdraw
        | ActionKind::BeginAllocating
        | ActionKind::FinishAllocating
        | ActionKind::SyncExternalAssets
        | ActionKind::BeginRefreshing
        | ActionKind::FinishRefreshing
        | ActionKind::RefreshFees
        | ActionKind::SettlePayout => AuthPattern::Allocator,

        // Emergency: Allocator, Curator, Sentinel, or Owner
        ActionKind::AbortAllocating
        | ActionKind::AbortWithdrawing
        | ActionKind::AbortRefreshing => AuthPattern::AllocatorOrSentinel,

        // Guardian-level
        ActionKind::Pause | ActionKind::SetRestrictions => AuthPattern::GuardianOrOwner,

        // Owner-only
        ActionKind::ManualReconcile | ActionKind::EmergencyReset => AuthPattern::OwnerOnly,
    }
}
