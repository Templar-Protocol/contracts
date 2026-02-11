use super::*;
use alloc::vec;

#[test]
fn test_new_plan() {
    let plan = RefreshPlan::new(vec![1, 2, 3]);
    assert!(!plan.is_empty());
    assert_eq!(plan.len(), 3);
    assert!(plan.cooldown.is_unlimited());
}

#[test]
fn test_empty_plan() {
    let plan = RefreshPlan::empty();
    assert!(plan.is_empty());
    assert_eq!(plan.len(), 0);
}

#[test]
fn test_validate_refresh_plan_success() {
    let plan = RefreshPlan::new(vec![1, 2, 3]);
    assert!(plan.validate().is_ok());
}

#[test]
fn test_validate_refresh_plan_empty() {
    let plan = RefreshPlan::empty();
    assert!(matches!(plan.validate(), Err(RefreshPlanError::EmptyPlan)));
}

#[test]
fn test_validate_refresh_plan_duplicate() {
    let plan = RefreshPlan::new(vec![1, 2, 1]);
    assert!(matches!(
        plan.validate(),
        Err(RefreshPlanError::DuplicateTarget { target_id: 1 })
    ));
}

#[test]
fn test_check_refresh_cooldown_no_cooldown() {
    let plan = RefreshPlan::new(vec![1, 2]);
    assert!(plan.check_cooldown(1000).is_ok());
    assert!(plan.is_ready(1000));
}

#[test]
fn test_check_refresh_cooldown_first_refresh() {
    let plan = RefreshPlan::new(vec![1, 2]).with_cooldown(1000);
    // No last_refresh_ns, so first refresh should be allowed
    assert!(plan.check_cooldown(100).is_ok());
    assert!(plan.is_ready(100));
}

#[test]
fn test_check_refresh_cooldown_on_cooldown() {
    let plan = RefreshPlan::new(vec![1, 2])
        .with_cooldown(1000)
        .with_last_refresh(100);

    // Only 500ns elapsed, cooldown is 1000ns
    let result = plan.check_cooldown(600);
    assert!(matches!(result, Err(RefreshPlanError::OnCooldown { .. })));
    assert!(!plan.is_ready(600));
}

#[test]
fn test_check_refresh_cooldown_after_cooldown() {
    let plan = RefreshPlan::new(vec![1, 2])
        .with_cooldown(1000)
        .with_last_refresh(100);

    // 1100ns elapsed, cooldown is 1000ns
    assert!(plan.check_cooldown(1200).is_ok());
    assert!(plan.is_ready(1200));
}

#[test]
fn test_build_refresh_plan() {
    let enabled = vec![1, 2, 3];
    let plan = build_refresh_plan(&enabled, Some(5000)).unwrap();

    assert_eq!(plan.targets, vec![1, 2, 3]);
    assert_eq!(plan.cooldown_ns(), 5000);
}

#[test]
fn test_build_refresh_plan_empty() {
    let enabled: Vec<TargetId> = vec![];
    let result = build_refresh_plan(&enabled, None);

    assert!(matches!(result, Err(RefreshPlanError::EmptyPlan)));
}

#[test]
fn test_build_targeted_refresh_plan() {
    let enabled = vec![1, 2, 3, 4];
    let targets = vec![2, 4];

    let plan = build_targeted_refresh_plan(&targets, &enabled).unwrap();

    assert_eq!(plan.targets, vec![2, 4]);
}

#[test]
fn test_build_targeted_refresh_plan_invalid_target() {
    let enabled = vec![1, 2, 3];
    let targets = vec![2, 5]; // 5 is not enabled

    let result = build_targeted_refresh_plan(&targets, &enabled);

    assert!(matches!(
        result,
        Err(RefreshPlanError::TargetNotFound { target_id: 5 })
    ));
}

#[test]
fn test_build_targeted_refresh_plan_duplicate() {
    let enabled = vec![1, 2, 3];
    let targets = vec![1, 2, 1]; // duplicate 1

    let result = build_targeted_refresh_plan(&targets, &enabled);

    assert!(matches!(
        result,
        Err(RefreshPlanError::DuplicateTarget { target_id: 1 })
    ));
}

#[test]
fn test_record_refresh_completion() {
    let plan = RefreshPlan::new(vec![1, 2]).with_cooldown(1000);
    let updated = plan.record_completion(5000);

    assert_eq!(updated.last_refresh_ns(), Some(5000));
    assert_eq!(updated.cooldown_ns(), 1000);
    assert_eq!(updated.targets, vec![1, 2]);
}

#[test]
fn test_filter_stale_targets() {
    let targets = vec![
        (1, 1000), // refreshed at 1000
        (2, 500),  // refreshed at 500
        (3, 2000), // refreshed at 2000
    ];

    // Current time is 3000, max age is 1500
    // Target 2 (age 2500) is stale
    // Target 1 (age 2000) is stale
    // Target 3 (age 1000) is fresh
    let stale = filter_stale_targets(&targets, 1500, 3000);

    assert_eq!(stale.len(), 2);
    assert!(stale.contains(&1));
    assert!(stale.contains(&2));
    assert!(!stale.contains(&3));
}

#[test]
fn test_to_target_list() {
    let plan = RefreshPlan::new(vec![5, 3, 1]);
    let list = plan.to_target_list();
    assert_eq!(list, vec![5, 3, 1]);
}

#[test]
fn test_find_first_duplicate_shared_helper() {
    assert_eq!(find_first_duplicate(&[1, 2, 3]), None);
    assert_eq!(find_first_duplicate(&[1, 2, 1]), Some(1));
    assert_eq!(find_first_duplicate(&[1, 2, 2, 3]), Some(2));
    assert_eq!(find_first_duplicate::<i32>(&[]), None);
}
