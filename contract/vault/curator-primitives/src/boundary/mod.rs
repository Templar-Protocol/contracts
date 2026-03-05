use crate::auth::{ActionKind, AuthPolicyClass, AuthPolicyProfile};

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "boundary", derive(near_sdk::BorshStorageKey))]
pub enum VaultStorageKey {
    PendingWithdrawals,
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
    match action.policy_class(AuthPolicyProfile::Boundary) {
        AuthPolicyClass::Guardian => BoundaryAuthPattern::GuardianOrOwner,
        AuthPolicyClass::Allocator => BoundaryAuthPattern::Allocator,
        AuthPolicyClass::AllocatorEmergency => BoundaryAuthPattern::AllocatorOrSentinel,
        AuthPolicyClass::Public | AuthPolicyClass::Curator => BoundaryAuthPattern::OwnerOnly,
    }
}

#[cfg(test)]
mod tests;
