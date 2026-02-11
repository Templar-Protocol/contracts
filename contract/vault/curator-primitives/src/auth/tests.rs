use super::*;

#[test]
fn test_action_kind_is_privileged_by_profile() {
    assert!(!ActionKind::Deposit.is_privileged(AuthPolicyProfile::Canonical));
    assert!(!ActionKind::RequestWithdraw.is_privileged(AuthPolicyProfile::Canonical));
    assert!(!ActionKind::ExecuteWithdraw.is_privileged(AuthPolicyProfile::Canonical));

    assert!(ActionKind::Pause.is_privileged(AuthPolicyProfile::Canonical));
    assert!(ActionKind::SetRestrictions.is_privileged(AuthPolicyProfile::Canonical));
    assert!(ActionKind::FinishAllocating.is_privileged(AuthPolicyProfile::Canonical));
    assert!(ActionKind::BeginAllocating.is_privileged(AuthPolicyProfile::Canonical));
    assert!(ActionKind::AbortAllocating.is_privileged(AuthPolicyProfile::Canonical));
    assert!(ActionKind::ManualReconcile.is_privileged(AuthPolicyProfile::Canonical));

    assert!(ActionKind::ExecuteWithdraw.is_privileged(AuthPolicyProfile::Near));
}

#[test]
fn test_policy_class_canonical() {
    assert_eq!(
        action_policy_class(ActionKind::ExecuteWithdraw, AuthPolicyProfile::Canonical),
        AuthPolicyClass::Public
    );
    assert_eq!(
        action_policy_class(ActionKind::Pause, AuthPolicyProfile::Canonical),
        AuthPolicyClass::Guardian
    );
    assert_eq!(
        action_policy_class(ActionKind::AbortRefreshing, AuthPolicyProfile::Canonical),
        AuthPolicyClass::Allocator
    );
    assert_eq!(
        action_policy_class(ActionKind::ManualReconcile, AuthPolicyProfile::Canonical),
        AuthPolicyClass::Curator
    );
}

#[test]
fn test_policy_class_near_profile() {
    assert_eq!(
        action_policy_class(ActionKind::ExecuteWithdraw, AuthPolicyProfile::Near),
        AuthPolicyClass::Allocator
    );
    assert_eq!(
        action_policy_class(ActionKind::AbortRefreshing, AuthPolicyProfile::Near),
        AuthPolicyClass::AllocatorEmergency
    );
    assert_eq!(
        action_policy_class(ActionKind::SetRestrictions, AuthPolicyProfile::Near),
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
    assert!(auth
        .authorize(ActionKind::ExecuteWithdraw, caller, None)
        .is_ok());
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
