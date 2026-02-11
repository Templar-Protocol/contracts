//! Refresh plan for updating market principal data.

use alloc::{collections::BTreeSet, vec::Vec};
use templar_vault_kernel::TargetId;

use super::cooldown::Cooldown;
use super::target_set::find_first_duplicate;

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
    #[must_use]
    pub fn new(targets: Vec<TargetId>) -> Self {
        Self {
            targets,
            cooldown: Cooldown::unlimited(),
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_cooldown(mut self, cooldown_ns: u64) -> Self {
        self.cooldown = Cooldown::new(cooldown_ns);
        self
    }

    #[must_use]
    pub fn with_last_refresh(mut self, last_refresh_ns: u64) -> Self {
        self.cooldown = self.cooldown.record(last_refresh_ns);
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }

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

        if let Some(dup) = find_first_duplicate(&self.targets) {
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

    #[must_use]
    pub fn to_target_list(&self) -> Vec<TargetId> {
        self.targets.clone()
    }

    #[must_use]
    pub fn cooldown_ns(&self) -> u64 {
        self.cooldown.interval_ns
    }

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

    let enabled_set: BTreeSet<_> = enabled_targets.iter().copied().collect();

    // Validate all targets are enabled
    for target in targets {
        if !enabled_set.contains(target) {
            return Err(RefreshPlanError::TargetNotFound { target_id: *target });
        }
    }

    // Check for duplicates
    if let Some(dup) = find_first_duplicate(targets) {
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
mod tests;
