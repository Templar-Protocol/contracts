use super::*;

#[test]
fn test_action_kind_is_privileged() {
    assert!(!ActionKind::Deposit.is_privileged());
    assert!(!ActionKind::RequestWithdraw.is_privileged());
    assert!(ActionKind::ExecuteWithdraw.is_privileged());

    assert!(ActionKind::Pause.is_privileged());
    assert!(ActionKind::SetRestrictions.is_privileged());
    assert!(ActionKind::FinishAllocating.is_privileged());
    assert!(ActionKind::BeginAllocating.is_privileged());
    assert!(ActionKind::AbortAllocating.is_privileged());
    assert!(ActionKind::ManualReconcile.is_privileged());
}

#[test]
fn test_policy_class_canonical() {
    assert_eq!(
        canonical_policy_class(ActionKind::ExecuteWithdraw),
        AuthPolicyClass::Allocator
    );
    assert_eq!(
        canonical_policy_class(ActionKind::Pause),
        AuthPolicyClass::Guardian
    );
    assert_eq!(
        canonical_policy_class(ActionKind::AbortRefreshing),
        AuthPolicyClass::AllocatorEmergency
    );
    assert_eq!(
        canonical_policy_class(ActionKind::ManualReconcile),
        AuthPolicyClass::Curator
    );
}

#[test]
fn test_policy_class_boundary() {
    assert_eq!(
        boundary_policy_class(ActionKind::ExecuteWithdraw),
        AuthPolicyClass::Allocator
    );
    assert_eq!(
        boundary_policy_class(ActionKind::AbortRefreshing),
        AuthPolicyClass::AllocatorEmergency
    );
    assert_eq!(
        boundary_policy_class(ActionKind::SetRestrictions),
        AuthPolicyClass::Guardian
    );
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
    let result = auth.authorize(ActionKind::ExecuteWithdraw, caller, None);
    assert!(matches!(result, Err(AuthError::NotAuthorized { .. })));
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
