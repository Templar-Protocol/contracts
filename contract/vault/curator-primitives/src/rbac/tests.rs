use super::*;

fn curator_addr() -> Address {
    [1u8; 32]
}

fn guardian_addr() -> Address {
    [2u8; 32]
}

fn allocator_addr() -> Address {
    [3u8; 32]
}

fn user_addr() -> Address {
    [4u8; 32]
}

fn sentinel_addr() -> Address {
    [5u8; 32]
}

fn test_rbac() -> RbacAuth {
    let mut config = RbacConfig::with_curator(curator_addr());
    config.add_role(guardian_addr(), Role::Guardian);
    config.add_role(allocator_addr(), Role::Allocator);
    config.add_role(sentinel_addr(), Role::Sentinel);
    RbacAuth::new(config)
}

#[test]
fn test_role_assignment() {
    let config = RbacConfig::with_curator(curator_addr());

    assert!(config.has_role(&curator_addr(), Role::Curator));
    assert!(!config.has_role(&user_addr(), Role::Curator));
}

#[test]
fn test_add_remove_role() {
    let mut config = RbacConfig::new();

    config.add_role(guardian_addr(), Role::Guardian);
    assert!(config.has_role(&guardian_addr(), Role::Guardian));

    config.remove_role(&guardian_addr(), Role::Guardian);
    assert!(!config.has_role(&guardian_addr(), Role::Guardian));
}

#[test]
fn test_get_roles() {
    let mut config = RbacConfig::with_curator(curator_addr());
    config.add_role(curator_addr(), Role::Guardian); // Curator also guardian

    let roles = config.get_roles(&curator_addr());
    assert_eq!(roles.len(), 2);
    assert!(roles.contains(&Role::Curator));
    assert!(roles.contains(&Role::Guardian));
}

#[test]
fn test_sentinel_role() {
    let mut config = RbacConfig::with_curator(curator_addr());
    config.add_role(sentinel_addr(), Role::Sentinel);

    assert!(config.has_role(&sentinel_addr(), Role::Sentinel));
    assert!(!config.has_role(&user_addr(), Role::Sentinel));
    assert!(!config.has_role(&guardian_addr(), Role::Sentinel));

    assert_eq!(Role::Sentinel.as_str(), "sentinel");

    let roles = config.get_roles(&sentinel_addr());
    assert_eq!(roles.len(), 1);
    assert!(roles.contains(&Role::Sentinel));
}

#[test]
fn test_sentinel_add_remove() {
    let mut config = RbacConfig::new();

    config.add_role(sentinel_addr(), Role::Sentinel);
    assert!(config.has_role(&sentinel_addr(), Role::Sentinel));

    config.remove_role(&sentinel_addr(), Role::Sentinel);
    assert!(!config.has_role(&sentinel_addr(), Role::Sentinel));
}

#[test]
fn test_user_actions_allowed() {
    let auth = test_rbac();

    // Any user can deposit
    assert!(auth
        .authorize(ActionKind::Deposit, user_addr(), None)
        .is_ok());
    assert!(auth
        .authorize(ActionKind::RequestWithdraw, user_addr(), None)
        .is_ok());
}

#[test]
fn test_execute_withdraw_allocator_only() {
    let auth = test_rbac();

    assert!(auth
        .authorize(ActionKind::ExecuteWithdraw, allocator_addr(), None)
        .is_ok());
    assert!(auth
        .authorize(ActionKind::ExecuteWithdraw, curator_addr(), None)
        .is_ok());

    let result = auth.authorize(ActionKind::ExecuteWithdraw, user_addr(), None);
    assert!(matches!(result, Err(AuthError::MissingRole)));
}

#[test]
fn test_abort_actions_allow_allocator_or_sentinel() {
    let auth = test_rbac();

    assert!(auth
        .authorize(ActionKind::AbortAllocating, allocator_addr(), None)
        .is_ok());
    assert!(auth
        .authorize(ActionKind::AbortAllocating, sentinel_addr(), None)
        .is_ok());

    let result = auth.authorize(ActionKind::AbortAllocating, user_addr(), None);
    assert!(matches!(result, Err(AuthError::MissingRole)));
}

#[test]
fn test_guardian_can_pause() {
    let auth = test_rbac();

    // Guardian can pause
    assert!(auth
        .authorize(ActionKind::Pause, guardian_addr(), None)
        .is_ok());

    // User cannot pause
    let result = auth.authorize(ActionKind::Pause, user_addr(), None);
    assert!(matches!(result, Err(AuthError::MissingRole)));
}

#[test]
fn test_allocator_actions() {
    let auth = test_rbac();

    // Allocator can do allocation operations
    assert!(auth
        .authorize(ActionKind::BeginAllocating, allocator_addr(), None)
        .is_ok());
    assert!(auth
        .authorize(ActionKind::FinishAllocating, allocator_addr(), None)
        .is_ok());
    assert!(auth
        .authorize(ActionKind::SyncExternalAssets, allocator_addr(), None)
        .is_ok());
    assert!(auth
        .authorize(ActionKind::BeginRefreshing, allocator_addr(), None)
        .is_ok());
    assert!(auth
        .authorize(ActionKind::FinishRefreshing, allocator_addr(), None)
        .is_ok());

    // User cannot do allocation operations
    let result = auth.authorize(ActionKind::BeginAllocating, user_addr(), None);
    assert!(matches!(result, Err(AuthError::MissingRole)));
}

#[test]
fn test_curator_can_do_everything() {
    let auth = test_rbac();

    // Curator can do all privileged actions
    assert!(auth
        .authorize(ActionKind::Pause, curator_addr(), None)
        .is_ok());
    assert!(auth
        .authorize(ActionKind::BeginAllocating, curator_addr(), None)
        .is_ok());
    assert!(auth
        .authorize(ActionKind::ManualReconcile, curator_addr(), None)
        .is_ok());
    assert!(auth
        .authorize(ActionKind::Deposit, curator_addr(), None)
        .is_ok());
}

#[test]
fn test_manual_reconcile_curator_only() {
    let auth = test_rbac();

    // Only curator can do manual reconcile
    assert!(auth
        .authorize(ActionKind::ManualReconcile, curator_addr(), None)
        .is_ok());

    // Allocator cannot
    let result = auth.authorize(ActionKind::ManualReconcile, allocator_addr(), None);
    assert!(matches!(result, Err(AuthError::MissingRole)));

    // Guardian cannot
    let result = auth.authorize(ActionKind::ManualReconcile, guardian_addr(), None);
    assert!(matches!(result, Err(AuthError::MissingRole)));
}

#[test]
fn test_paused_blocks_user_actions() {
    let mut auth = test_rbac();
    auth.config.set_paused(true);

    // User actions blocked
    let result = auth.authorize(ActionKind::Deposit, user_addr(), None);
    assert!(matches!(result, Err(AuthError::VaultPaused)));

    // Curator can still act when paused
    assert!(auth
        .authorize(ActionKind::BeginAllocating, curator_addr(), None)
        .is_ok());
}

#[test]
fn test_paused_allows_pause_action() {
    let mut auth = test_rbac();
    auth.config.set_paused(true);

    // Guardian can still trigger pause action (to unpause)
    assert!(auth
        .authorize(ActionKind::Pause, guardian_addr(), None)
        .is_ok());
}

#[test]
fn test_is_paused() {
    let mut auth = test_rbac();

    assert!(!auth.is_paused());

    auth.config.set_paused(true);
    assert!(auth.is_paused());
}

#[test]
fn test_role_as_str() {
    assert_eq!(Role::Curator.as_str(), "curator");
    assert_eq!(Role::Guardian.as_str(), "guardian");
    assert_eq!(Role::Sentinel.as_str(), "sentinel");
    assert_eq!(Role::Allocator.as_str(), "allocator");
}
