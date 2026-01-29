//! Refresh plan for updating market principal data.
//!
//! Refresh plans define which markets need their principal data updated
//! to maintain accurate AUM calculations. This is used during the
//! Refreshing state of the vault operation state machine.

use alloc::vec::Vec;
use templar_vault_kernel::TargetId;

/// A plan for refreshing market principal data.
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct RefreshPlan {
    /// Ordered list of target IDs to refresh.
    pub targets: Vec<TargetId>,
    /// Last refresh timestamp (nanoseconds), if known.
    pub last_refresh_ns: Option<u64>,
    /// Minimum interval between refreshes (nanoseconds).
    pub cooldown_ns: u64,
}

impl RefreshPlan {
    /// Create a new empty refresh plan.
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
            last_refresh_ns: None,
            cooldown_ns: 0,
        }
    }

    /// Create a refresh plan from a list of targets.
    pub fn from_targets(targets: Vec<TargetId>) -> Self {
        Self {
            targets,
            last_refresh_ns: None,
            cooldown_ns: 0,
        }
    }

    /// Create a refresh plan with cooldown.
    pub fn with_cooldown(targets: Vec<TargetId>, cooldown_ns: u64) -> Self {
        Self {
            targets,
            last_refresh_ns: None,
            cooldown_ns,
        }
    }

    /// Returns true if the plan is empty.
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Returns the number of targets to refresh.
    pub fn len(&self) -> usize {
        self.targets.len()
    }
}

/// Errors that can occur during refresh plan operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefreshPlanError {
    /// Plan is empty.
    EmptyPlan,
    /// Refresh is still on cooldown.
    OnCooldown {
        last_refresh_ns: u64,
        cooldown_ns: u64,
        current_ns: u64,
    },
    /// Duplicate target in plan.
    DuplicateTarget { target_id: TargetId },
    /// Target not found.
    TargetNotFound { target_id: TargetId },
}

/// Validate a refresh plan.
///
/// Checks:
/// - Plan is not empty
/// - No duplicate targets
///
/// # Arguments
/// * `plan` - The refresh plan to validate
///
/// # Returns
/// `Ok(())` if valid, or the first error found.
pub fn validate_refresh_plan(plan: &RefreshPlan) -> Result<(), RefreshPlanError> {
    if plan.is_empty() {
        return Err(RefreshPlanError::EmptyPlan);
    }

    // Check for duplicates
    let mut seen: Vec<TargetId> = Vec::new();
    for target_id in &plan.targets {
        if seen.contains(target_id) {
            return Err(RefreshPlanError::DuplicateTarget {
                target_id: *target_id,
            });
        }
        seen.push(*target_id);
    }

    Ok(())
}

/// Check if a refresh is allowed based on cooldown.
///
/// # Arguments
/// * `plan` - The refresh plan with cooldown settings
/// * `current_ns` - Current timestamp in nanoseconds
///
/// # Returns
/// `Ok(())` if refresh is allowed, or `Err` if still on cooldown.
pub fn check_refresh_cooldown(plan: &RefreshPlan, current_ns: u64) -> Result<(), RefreshPlanError> {
    if plan.cooldown_ns == 0 {
        return Ok(());
    }

    if let Some(last_refresh) = plan.last_refresh_ns {
        let elapsed = current_ns.saturating_sub(last_refresh);
        if elapsed < plan.cooldown_ns {
            return Err(RefreshPlanError::OnCooldown {
                last_refresh_ns: last_refresh,
                cooldown_ns: plan.cooldown_ns,
                current_ns,
            });
        }
    }

    Ok(())
}

/// Build a refresh plan from a list of enabled markets.
///
/// # Arguments
/// * `enabled_targets` - List of target IDs that are enabled
/// * `cooldown_ns` - Optional cooldown between refreshes
///
/// # Returns
/// A refresh plan for all enabled targets.
pub fn build_refresh_plan(
    enabled_targets: &[TargetId],
    cooldown_ns: Option<u64>,
) -> Result<RefreshPlan, RefreshPlanError> {
    if enabled_targets.is_empty() {
        return Err(RefreshPlanError::EmptyPlan);
    }

    Ok(RefreshPlan {
        targets: enabled_targets.to_vec(),
        last_refresh_ns: None,
        cooldown_ns: cooldown_ns.unwrap_or(0),
    })
}

/// Build a refresh plan for specific targets only.
///
/// # Arguments
/// * `targets` - Specific targets to refresh
/// * `enabled_targets` - All enabled targets (for validation)
///
/// # Returns
/// A refresh plan if all specified targets are valid.
pub fn build_targeted_refresh_plan(
    targets: &[TargetId],
    enabled_targets: &[TargetId],
) -> Result<RefreshPlan, RefreshPlanError> {
    if targets.is_empty() {
        return Err(RefreshPlanError::EmptyPlan);
    }

    // Validate all targets are enabled
    for target in targets {
        if !enabled_targets.contains(target) {
            return Err(RefreshPlanError::TargetNotFound { target_id: *target });
        }
    }

    // Check for duplicates
    let mut seen: Vec<TargetId> = Vec::new();
    for target_id in targets {
        if seen.contains(target_id) {
            return Err(RefreshPlanError::DuplicateTarget {
                target_id: *target_id,
            });
        }
        seen.push(*target_id);
    }

    Ok(RefreshPlan::from_targets(targets.to_vec()))
}

/// Compute the "total" of a refresh plan.
///
/// For refresh plans, this is simply the number of targets,
/// as there's no monetary amount involved.
///
/// # Arguments
/// * `plan` - The refresh plan
///
/// # Returns
/// Number of targets in the plan.
pub fn compute_refresh_plan_total(plan: &RefreshPlan) -> usize {
    plan.len()
}

/// Update refresh plan with new last refresh timestamp.
///
/// # Arguments
/// * `plan` - The current refresh plan
/// * `timestamp_ns` - The timestamp of the completed refresh
///
/// # Returns
/// Updated refresh plan with new last_refresh_ns.
pub fn record_refresh_completion(plan: &RefreshPlan, timestamp_ns: u64) -> RefreshPlan {
    RefreshPlan {
        targets: plan.targets.clone(),
        last_refresh_ns: Some(timestamp_ns),
        cooldown_ns: plan.cooldown_ns,
    }
}

/// Convert a refresh plan to a list of target IDs.
///
/// This is useful for passing to the refresh state machine.
pub fn to_target_list(plan: &RefreshPlan) -> Vec<TargetId> {
    plan.targets.clone()
}

/// Filter targets that need refresh based on staleness.
///
/// # Arguments
/// * `all_targets` - List of (target_id, last_refresh_ns) pairs
/// * `max_age_ns` - Maximum age before a target is considered stale
/// * `current_ns` - Current timestamp in nanoseconds
///
/// # Returns
/// List of target IDs that are stale and need refresh.
pub fn filter_stale_targets(
    all_targets: &[(TargetId, u64)],
    max_age_ns: u64,
    current_ns: u64,
) -> Vec<TargetId> {
    all_targets
        .iter()
        .filter_map(|(target_id, last_refresh)| {
            let age = current_ns.saturating_sub(*last_refresh);
            if age > max_age_ns {
                Some(*target_id)
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_new_plan_is_empty() {
        let plan = RefreshPlan::new();
        assert!(plan.is_empty());
        assert_eq!(plan.len(), 0);
    }

    #[test]
    fn test_from_targets() {
        let plan = RefreshPlan::from_targets(vec![1, 2, 3]);
        assert!(!plan.is_empty());
        assert_eq!(plan.len(), 3);
    }

    #[test]
    fn test_validate_refresh_plan_success() {
        let plan = RefreshPlan::from_targets(vec![1, 2, 3]);
        assert!(validate_refresh_plan(&plan).is_ok());
    }

    #[test]
    fn test_validate_refresh_plan_empty() {
        let plan = RefreshPlan::new();
        assert!(matches!(
            validate_refresh_plan(&plan),
            Err(RefreshPlanError::EmptyPlan)
        ));
    }

    #[test]
    fn test_validate_refresh_plan_duplicate() {
        let plan = RefreshPlan::from_targets(vec![1, 2, 1]);
        assert!(matches!(
            validate_refresh_plan(&plan),
            Err(RefreshPlanError::DuplicateTarget { target_id: 1 })
        ));
    }

    #[test]
    fn test_check_refresh_cooldown_no_cooldown() {
        let plan = RefreshPlan::from_targets(vec![1, 2]);
        assert!(check_refresh_cooldown(&plan, 1000).is_ok());
    }

    #[test]
    fn test_check_refresh_cooldown_first_refresh() {
        let plan = RefreshPlan::with_cooldown(vec![1, 2], 1000);
        // No last_refresh_ns, so first refresh should be allowed
        assert!(check_refresh_cooldown(&plan, 100).is_ok());
    }

    #[test]
    fn test_check_refresh_cooldown_on_cooldown() {
        let mut plan = RefreshPlan::with_cooldown(vec![1, 2], 1000);
        plan.last_refresh_ns = Some(100);

        // Only 500ns elapsed, cooldown is 1000ns
        let result = check_refresh_cooldown(&plan, 600);
        assert!(matches!(result, Err(RefreshPlanError::OnCooldown { .. })));
    }

    #[test]
    fn test_check_refresh_cooldown_after_cooldown() {
        let mut plan = RefreshPlan::with_cooldown(vec![1, 2], 1000);
        plan.last_refresh_ns = Some(100);

        // 1100ns elapsed, cooldown is 1000ns
        assert!(check_refresh_cooldown(&plan, 1200).is_ok());
    }

    #[test]
    fn test_build_refresh_plan() {
        let enabled = vec![1, 2, 3];
        let plan = build_refresh_plan(&enabled, Some(5000)).unwrap();

        assert_eq!(plan.targets, vec![1, 2, 3]);
        assert_eq!(plan.cooldown_ns, 5000);
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
    fn test_compute_refresh_plan_total() {
        let plan = RefreshPlan::from_targets(vec![1, 2, 3, 4, 5]);
        assert_eq!(compute_refresh_plan_total(&plan), 5);
    }

    #[test]
    fn test_record_refresh_completion() {
        let plan = RefreshPlan::with_cooldown(vec![1, 2], 1000);
        let updated = record_refresh_completion(&plan, 5000);

        assert_eq!(updated.last_refresh_ns, Some(5000));
        assert_eq!(updated.cooldown_ns, 1000);
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
        let plan = RefreshPlan::from_targets(vec![5, 3, 1]);
        let list = to_target_list(&plan);
        assert_eq!(list, vec![5, 3, 1]);
    }
}
