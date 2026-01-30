//! Refresh plan for updating market principal data.
//!
//! Refresh plans define which markets need their principal data updated
//! to maintain accurate AUM calculations. This is used during the
//! Refreshing state of the vault operation state machine.
//!
//! # Example
//!
//! ```ignore
//! use templar_curator_primitives::policy::refresh_plan::*;
//!
//! let plan = RefreshPlan::new(vec![1, 2, 3])
//!     .with_cooldown(1000);
//!
//! assert!(plan.validate().is_ok());
//! assert!(plan.is_ready(500)); // First refresh always allowed
//! ```

use alloc::{collections::BTreeSet, vec::Vec};
use templar_vault_kernel::TargetId;

use super::cooldown::Cooldown;

/// A plan for refreshing market principal data.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct RefreshPlan {
    /// Ordered list of target IDs to refresh.
    pub targets: Vec<TargetId>,
    /// Cooldown tracking for rate-limiting refreshes.
    pub cooldown: Cooldown,
}

impl RefreshPlan {
    /// Create a new refresh plan from a list of targets.
    #[must_use]
    pub fn new(targets: Vec<TargetId>) -> Self {
        Self {
            targets,
            cooldown: Cooldown::unlimited(),
        }
    }

    /// Create an empty refresh plan.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Builder method: set cooldown interval.
    #[must_use]
    pub fn with_cooldown(mut self, cooldown_ns: u64) -> Self {
        self.cooldown = Cooldown::new(cooldown_ns);
        self
    }

    /// Builder method: set last refresh time (for resuming).
    #[must_use]
    pub fn with_last_refresh(mut self, last_refresh_ns: u64) -> Self {
        self.cooldown = self.cooldown.record(last_refresh_ns);
        self
    }

    /// Returns true if the plan is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Returns the number of targets to refresh.
    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    /// Check if a refresh is ready (cooldown has elapsed).
    #[must_use]
    pub fn is_ready(&self, current_ns: u64) -> bool {
        self.cooldown.is_ready(current_ns)
    }

    /// Validate a refresh plan.
    ///
    /// Checks:
    /// - Plan is not empty
    /// - No duplicate targets
    pub fn validate(&self) -> Result<(), RefreshPlanError> {
        if self.is_empty() {
            return Err(RefreshPlanError::EmptyPlan);
        }

        if let Some(dup) = find_duplicate(&self.targets) {
            return Err(RefreshPlanError::DuplicateTarget { target_id: dup });
        }

        Ok(())
    }

    /// Check if a refresh is allowed based on cooldown.
    pub fn check_cooldown(&self, current_ns: u64) -> Result<(), RefreshPlanError> {
        self.cooldown.check(current_ns).map_err(|e| match e {
            super::cooldown::CooldownError::OnCooldown {
                last_event_ns,
                interval_ns,
                current_ns,
            } => RefreshPlanError::OnCooldown {
                last_refresh_ns: last_event_ns,
                cooldown_ns: interval_ns,
                current_ns,
            },
        })
    }

    /// Record completion time for a refresh plan.
    #[must_use]
    pub fn record_completion(&self, timestamp_ns: u64) -> Self {
        Self {
            targets: self.targets.clone(),
            cooldown: self.cooldown.record(timestamp_ns),
        }
    }

    /// Convert to a list of target IDs.
    #[must_use]
    pub fn to_target_list(&self) -> Vec<TargetId> {
        self.targets.clone()
    }

    /// Get the cooldown interval in nanoseconds.
    #[must_use]
    pub fn cooldown_ns(&self) -> u64 {
        self.cooldown.interval_ns
    }

    /// Get the last refresh timestamp, if any.
    #[must_use]
    pub fn last_refresh_ns(&self) -> Option<u64> {
        self.cooldown.last_event_ns
    }
}

impl From<Vec<TargetId>> for RefreshPlan {
    fn from(targets: Vec<TargetId>) -> Self {
        Self::new(targets)
    }
}

/// Helper to find the first duplicate in a slice.
fn find_duplicate<T: Ord + Copy>(items: &[T]) -> Option<T> {
    let mut seen = BTreeSet::new();
    for item in items {
        if !seen.insert(*item) {
            return Some(*item);
        }
    }
    None
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

    let plan = RefreshPlan::new(enabled_targets.to_vec());
    let plan = match cooldown_ns {
        Some(ns) => plan.with_cooldown(ns),
        None => plan,
    };

    Ok(plan)
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

    // Use BTreeSet for O(log n) lookup
    let enabled_set: BTreeSet<_> = enabled_targets.iter().copied().collect();

    // Validate all targets are enabled
    for target in targets {
        if !enabled_set.contains(target) {
            return Err(RefreshPlanError::TargetNotFound { target_id: *target });
        }
    }

    // Check for duplicates
    if let Some(dup) = find_duplicate(targets) {
        return Err(RefreshPlanError::DuplicateTarget { target_id: dup });
    }

    Ok(RefreshPlan::new(targets.to_vec()))
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
#[must_use]
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
        assert!(matches!(
            plan.validate(),
            Err(RefreshPlanError::EmptyPlan)
        ));
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
    fn test_find_duplicate_helper() {
        assert_eq!(find_duplicate(&[1, 2, 3]), None);
        assert_eq!(find_duplicate(&[1, 2, 1]), Some(1));
        assert_eq!(find_duplicate(&[1, 2, 2, 3]), Some(2));
        assert_eq!(find_duplicate::<i32>(&[]), None);
    }
}
