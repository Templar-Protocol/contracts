use alloc::{format, string::String};

use crate::auth::{action_policy_class, ActionKind, AuthError, AuthPolicyClass, AuthPolicyProfile};
use crate::governance::{
    CapChangeError, FeeChangeError, MembershipChangeError, RelativeCapChangeError,
    TimelockConfigError,
};
use crate::recovery::RecoveryOutcome;

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "boundary",
    derive(near_sdk::borsh::BorshDeserialize, near_sdk::borsh::BorshSerialize)
)]
#[cfg_attr(feature = "boundary", derive(near_sdk::BorshStorageKey))]
pub enum VaultStorageKey {
    PendingWithdrawals,
}

#[must_use]
pub const fn timelock_config_error_message(error: TimelockConfigError) -> &'static str {
    match error {
        TimelockConfigError::NoChange => "Already set to this value",
        TimelockConfigError::OutOfBounds => "Timelock out of bounds",
    }
}

#[must_use]
pub const fn fee_change_error_message(error: FeeChangeError) -> &'static str {
    match error {
        FeeChangeError::PerformanceFeeTooHigh => "performance fee too high",
        FeeChangeError::ManagementFeeTooHigh => "management fee too high",
        FeeChangeError::NoChange => "No fee changes",
    }
}

#[must_use]
pub const fn cap_change_error_message(error: CapChangeError) -> &'static str {
    match error {
        CapChangeError::NoChange => "New cap is same as current",
    }
}

#[must_use]
pub const fn relative_cap_change_error_message(error: RelativeCapChangeError) -> &'static str {
    match error {
        RelativeCapChangeError::RelativeCapTooHigh => "relative cap too high",
        RelativeCapChangeError::NoChange => "New relative cap is same as current",
    }
}

#[must_use]
pub const fn membership_change_error_message(error: MembershipChangeError) -> &'static str {
    match error {
        MembershipChangeError::NoChange => "Market already assigned to this cap group",
    }
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoundaryAuthPattern {
    OwnerOnly,
    GuardianOrOwner,
    Allocator,
    AllocatorOrSentinel,
}

#[must_use]
pub const fn boundary_auth_pattern_for(action: ActionKind) -> BoundaryAuthPattern {
    match action_policy_class(action, AuthPolicyProfile::Boundary) {
        AuthPolicyClass::Guardian => BoundaryAuthPattern::GuardianOrOwner,
        AuthPolicyClass::Allocator => BoundaryAuthPattern::Allocator,
        AuthPolicyClass::AllocatorEmergency => BoundaryAuthPattern::AllocatorOrSentinel,
        AuthPolicyClass::Public | AuthPolicyClass::Curator => BoundaryAuthPattern::OwnerOnly,
    }
}

#[must_use]
pub fn auth_error_message(error: &AuthError) -> String {
    match error {
        AuthError::NotAuthorized { caller, .. } => format!("Not authorized: {caller}"),
        AuthError::InvalidProof => String::from("Invalid proof"),
        AuthError::MissingRole => String::from("Missing role"),
        AuthError::VaultPaused => String::from("Vault is paused"),
    }
}

#[must_use]
pub fn recovery_outcome_message(outcome: &RecoveryOutcome) -> String {
    if let Some(message) = &outcome.message {
        return message.clone();
    }
    if outcome.success {
        String::from("Recovery completed")
    } else {
        String::from("Recovery failed")
    }
}

#[cfg(test)]
mod tests;
