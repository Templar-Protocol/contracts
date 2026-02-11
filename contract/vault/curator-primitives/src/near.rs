use alloc::{format, string::String};

use crate::auth::{action_policy_class, ActionKind, AuthError, AuthPolicyClass, AuthPolicyProfile};
use crate::governance::{
    CapChangeError, FeeChangeError, MembershipChangeError, RelativeCapChangeError,
    TimelockConfigError,
};
use crate::recovery::RecoveryOutcome;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "near",
    derive(near_sdk::borsh::BorshDeserialize, near_sdk::borsh::BorshSerialize)
)]
#[cfg_attr(feature = "near", derive(near_sdk::BorshStorageKey))]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NearAuthPattern {
    OwnerOnly,
    GuardianOrOwner,
    Allocator,
    AllocatorOrSentinel,
}

#[must_use]
pub const fn near_auth_pattern_for(action: ActionKind) -> NearAuthPattern {
    match action_policy_class(action, AuthPolicyProfile::Near) {
        AuthPolicyClass::Guardian => NearAuthPattern::GuardianOrOwner,
        AuthPolicyClass::Allocator => NearAuthPattern::Allocator,
        AuthPolicyClass::AllocatorEmergency => NearAuthPattern::AllocatorOrSentinel,
        AuthPolicyClass::Public | AuthPolicyClass::Curator => NearAuthPattern::OwnerOnly,
    }
}

#[must_use]
pub fn auth_error_message(error: &AuthError) -> String {
    match error {
        AuthError::NotAuthorized { caller, .. } => format!("Not authorized: {caller:?}"),
        AuthError::InvalidProof => String::from("Invalid proof"),
        AuthError::MissingRole(role) => format!("Missing role: {role}"),
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
mod tests {
    use super::*;
    use crate::recovery::RecoveryOutcome;
    use templar_vault_kernel::KernelAction;

    #[test]
    fn governance_error_messages_match_expected_strings() {
        assert_eq!(
            timelock_config_error_message(TimelockConfigError::NoChange),
            "Already set to this value"
        );
        assert_eq!(
            timelock_config_error_message(TimelockConfigError::OutOfBounds),
            "Timelock out of bounds"
        );
        assert_eq!(
            fee_change_error_message(FeeChangeError::PerformanceFeeTooHigh),
            "performance fee too high"
        );
        assert_eq!(
            fee_change_error_message(FeeChangeError::ManagementFeeTooHigh),
            "management fee too high"
        );
        assert_eq!(
            fee_change_error_message(FeeChangeError::NoChange),
            "No fee changes"
        );
        assert_eq!(
            cap_change_error_message(CapChangeError::NoChange),
            "New cap is same as current"
        );
        assert_eq!(
            relative_cap_change_error_message(RelativeCapChangeError::RelativeCapTooHigh),
            "relative cap too high"
        );
        assert_eq!(
            relative_cap_change_error_message(RelativeCapChangeError::NoChange),
            "New relative cap is same as current"
        );
        assert_eq!(
            membership_change_error_message(MembershipChangeError::NoChange),
            "Market already assigned to this cap group"
        );
    }

    #[test]
    fn near_auth_pattern_mapping_matches_policy() {
        assert_eq!(
            near_auth_pattern_for(ActionKind::Pause),
            NearAuthPattern::GuardianOrOwner
        );
        assert_eq!(
            near_auth_pattern_for(ActionKind::ExecuteWithdraw),
            NearAuthPattern::Allocator
        );
        assert_eq!(
            near_auth_pattern_for(ActionKind::AbortAllocating),
            NearAuthPattern::AllocatorOrSentinel
        );
        assert_eq!(
            near_auth_pattern_for(ActionKind::ManualReconcile),
            NearAuthPattern::OwnerOnly
        );
    }

    #[test]
    fn auth_and_recovery_adapters_emit_expected_messages() {
        assert_eq!(
            auth_error_message(&AuthError::InvalidProof),
            "Invalid proof"
        );
        assert_eq!(
            auth_error_message(&AuthError::VaultPaused),
            "Vault is paused"
        );
        assert_eq!(
            auth_error_message(&AuthError::MissingRole(String::from("guardian"))),
            "Missing role: guardian"
        );

        let success = RecoveryOutcome::success(KernelAction::EmergencyReset);
        assert_eq!(recovery_outcome_message(&success), "Recovery completed");

        let failure =
            RecoveryOutcome::failure(KernelAction::EmergencyReset, "market callback failed");
        assert_eq!(recovery_outcome_message(&failure), "market callback failed");
    }

    #[cfg(feature = "near")]
    #[test]
    fn storage_key_borsh_encoding_is_stable() {
        let encoded = near_sdk::borsh::to_vec(&VaultStorageKey::PendingWithdrawals)
            .unwrap_or_else(|_| unreachable!("storage key must serialize"));
        assert_eq!(encoded, vec![0]);
    }
}
