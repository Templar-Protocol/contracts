use crate::governance::{
    CapChangeError, FeeChangeError, MembershipChangeError, RelativeCapChangeError,
    TimelockConfigError,
};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
