//! Authentication and authorization for the NEAR vault.
//!
//! Re-exports chain-agnostic auth types from `templar-curator-primitives`
//! and provides NEAR-specific authorization helpers that integrate with
//! NEAR's `Owner` + `Rbac` derive macros.

pub use templar_curator_primitives::auth::ActionKind;
use templar_curator_primitives::near::{near_auth_pattern_for, NearAuthPattern};

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
    /// Returns the set of non-owner roles that are permitted for this pattern.
    ///
    /// The Owner is *always* allowed as a fallback and is not listed here.
    /// An empty slice means only the Owner may call the action.
    pub fn allowed_roles(self) -> &'static [Role] {
        match self {
            AuthPattern::OwnerOnly => &[],
            AuthPattern::GuardianOrOwner => &[Role::Guardian],
            AuthPattern::GuardianOrSentinelOrOwner => &[Role::Guardian, Role::Sentinel],
            AuthPattern::CuratorOrOwner => &[Role::Curator],
            AuthPattern::CuratorOrSentinelOrOwner => &[Role::Curator, Role::Sentinel],
            AuthPattern::Allocator => &[Role::Allocator, Role::Curator],
            AuthPattern::AllocatorOrSentinel => &[Role::Allocator, Role::Curator, Role::Sentinel],
        }
    }

    /// Require the caller to match this auth pattern. Panics if unauthorized.
    pub fn require(self) {
        let caller = env::predecessor_account_id();
        let roles = self.allowed_roles();
        if roles.iter().any(|r| Contract::has_role(&caller, r)) {
            return;
        }
        Contract::require_owner();
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
    match near_auth_pattern_for(action) {
        NearAuthPattern::OwnerOnly => AuthPattern::OwnerOnly,
        NearAuthPattern::GuardianOrOwner => AuthPattern::GuardianOrOwner,
        NearAuthPattern::Allocator => AuthPattern::Allocator,
        NearAuthPattern::AllocatorOrSentinel => AuthPattern::AllocatorOrSentinel,
    }
}

#[inline]
pub fn require_action(action: ActionKind) {
    auth_pattern_for(action).require();
}
