use super::*;
use soroban_sdk::testutils::Address as _;

#[test]
fn test_soroban_auth_new() {
    let env = Env::default();
    let curator = SdkAddress::generate(&env);

    let auth = SorobanAuth::new(&env, curator.clone());

    assert_eq!(auth.curator(), &curator);
    assert!(!auth.paused());
}

#[test]
fn test_soroban_auth_curator_role() {
    let env = Env::default();
    let curator = SdkAddress::generate(&env);
    let user = SdkAddress::generate(&env);

    let auth = SorobanAuth::new(&env, curator.clone());

    assert!(auth.has_role(Role::Curator, &curator));
    assert!(!auth.has_role(Role::Curator, &user));
}

#[test]
fn test_soroban_auth_guardian_role() {
    let env = Env::default();
    let curator = SdkAddress::generate(&env);
    let guardian = SdkAddress::generate(&env);
    let user = SdkAddress::generate(&env);

    let auth = SorobanAuth::with_roles(&env, curator.clone(), Some(guardian.clone()), None);

    // Curator is always a guardian
    assert!(auth.has_role(Role::Guardian, &curator));
    // Designated guardian
    assert!(auth.has_role(Role::Guardian, &guardian));
    // Regular user is not
    assert!(!auth.has_role(Role::Guardian, &user));
}

#[test]
fn test_soroban_auth_allocator_role() {
    let env = Env::default();
    let curator = SdkAddress::generate(&env);
    let allocator = SdkAddress::generate(&env);
    let user = SdkAddress::generate(&env);

    let auth = SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));

    // Curator is always an allocator
    assert!(auth.has_role(Role::Allocator, &curator));
    // Designated allocator
    assert!(auth.has_role(Role::Allocator, &allocator));
    // Regular user is not
    assert!(!auth.has_role(Role::Allocator, &user));
}

#[test]
fn test_soroban_auth_check_role_user_actions() {
    let env = Env::default();
    let curator = SdkAddress::generate(&env);
    let user = SdkAddress::generate(&env);

    let auth = SorobanAuth::new(&env, curator);

    // User actions allowed for anyone
    assert!(auth.check_role(ActionKind::Deposit, &user).is_ok());
    assert!(auth.check_role(ActionKind::RequestWithdraw, &user).is_ok());
    assert!(auth.check_role(ActionKind::ExecuteWithdraw, &user).is_ok());
}

#[test]
fn test_soroban_auth_check_role_guardian_actions() {
    let env = Env::default();
    let curator = SdkAddress::generate(&env);
    let guardian = SdkAddress::generate(&env);
    let user = SdkAddress::generate(&env);

    let auth = SorobanAuth::with_roles(&env, curator.clone(), Some(guardian.clone()), None);

    // Guardian can pause
    assert!(auth.check_role(ActionKind::Pause, &guardian).is_ok());
    // Curator can pause (curator is always guardian)
    assert!(auth.check_role(ActionKind::Pause, &curator).is_ok());
    // User cannot pause
    let result = auth.check_role(ActionKind::Pause, &user);
    assert!(matches!(result, Err(AuthError::MissingRole(_))));
}

#[test]
fn test_soroban_auth_check_role_allocator_actions() {
    let env = Env::default();
    let curator = SdkAddress::generate(&env);
    let allocator = SdkAddress::generate(&env);
    let user = SdkAddress::generate(&env);

    let auth = SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));

    // Allocator can do allocation operations
    assert!(auth
        .check_role(ActionKind::BeginAllocating, &allocator)
        .is_ok());
    assert!(auth
        .check_role(ActionKind::FinishAllocating, &allocator)
        .is_ok());
    assert!(auth
        .check_role(ActionKind::BeginRefreshing, &allocator)
        .is_ok());
    assert!(auth
        .check_role(ActionKind::SyncExternalAssets, &allocator)
        .is_ok());

    // Curator can too
    assert!(auth
        .check_role(ActionKind::BeginAllocating, &curator)
        .is_ok());

    // User cannot
    let result = auth.check_role(ActionKind::BeginAllocating, &user);
    assert!(matches!(result, Err(AuthError::MissingRole(_))));
}

#[test]
fn test_soroban_auth_check_role_curator_only() {
    let env = Env::default();
    let curator = SdkAddress::generate(&env);
    let allocator = SdkAddress::generate(&env);

    let auth = SorobanAuth::with_roles(&env, curator.clone(), None, Some(allocator.clone()));

    // Only curator can do manual reconcile
    assert!(auth
        .check_role(ActionKind::ManualReconcile, &curator)
        .is_ok());

    // Allocator cannot
    let result = auth.check_role(ActionKind::ManualReconcile, &allocator);
    assert!(matches!(result, Err(AuthError::MissingRole(_))));
}

#[test]
fn test_soroban_auth_set_paused() {
    let env = Env::default();
    let curator = SdkAddress::generate(&env);

    let mut auth = SorobanAuth::new(&env, curator);

    assert!(!auth.paused());
    auth.set_paused(true);
    assert!(auth.paused());
    auth.set_paused(false);
    assert!(!auth.paused());
}
