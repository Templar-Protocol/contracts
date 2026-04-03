use alloc::{collections::BTreeSet, vec::Vec};
use core::num::NonZeroU64;
use templar_vault_kernel::TargetId;

use super::cooldown::Cooldown;
use super::target_set::find_first_duplicate;

#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone)]
pub struct RefreshPlan {
    targets: Vec<TargetId>,
}

#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RefreshThrottle {
    cooldown: Cooldown,
}

#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RefreshTargetStatus {
    target_id: TargetId,
    last_refresh_ns: Option<u64>,
}

#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone)]
pub struct RefreshExecutionPlan {
    plan: RefreshPlan,
    throttle: RefreshThrottle,
}

impl RefreshPlan {
    pub fn new(targets: Vec<TargetId>) -> Result<Self, RefreshPlanError> {
        if targets.is_empty() {
            return Err(RefreshPlanError::EmptyPlan);
        }

        if let Some(dup) = find_first_duplicate(&targets) {
            return Err(RefreshPlanError::DuplicateTarget { target_id: dup });
        }

        Ok(Self { targets })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    #[must_use]
    pub fn targets(&self) -> &[TargetId] {
        &self.targets
    }

    #[must_use]
    pub fn into_targets(self) -> Vec<TargetId> {
        self.targets
    }
}

impl RefreshThrottle {
    #[must_use]
    pub fn default_unlimited() -> Self {
        Self {
            cooldown: Cooldown::unlimited(),
        }
    }

    #[must_use]
    pub fn new(cooldown_ns: u64, last_refresh_ns: Option<u64>) -> Self {
        let cooldown = NonZeroU64::new(cooldown_ns)
            .map_or_else(Cooldown::unlimited, Cooldown::new)
            .with_last_event_ns(last_refresh_ns);

        Self { cooldown }
    }

    #[must_use]
    pub fn cooldown(&self) -> &Cooldown {
        &self.cooldown
    }

    #[must_use]
    pub fn is_ready(&self, current_ns: u64) -> bool {
        self.cooldown.is_ready(current_ns)
    }

    pub fn check(&self, current_ns: u64) -> Result<(), RefreshPlanError> {
        self.cooldown.check(current_ns).map_err(|e| match e {
            super::cooldown::CooldownError::OnCooldown {
                ready_at_ns,
                remaining_ns,
            } => RefreshPlanError::OnCooldown {
                ready_at_ns,
                remaining_ns,
            },
        })
    }

    pub fn try_acquire(self, current_ns: u64) -> Result<Self, RefreshPlanError> {
        self.cooldown
            .try_acquire(current_ns)
            .map(|cooldown| Self { cooldown })
            .map_err(|e| match e {
                super::cooldown::CooldownError::OnCooldown {
                    ready_at_ns,
                    remaining_ns,
                } => RefreshPlanError::OnCooldown {
                    ready_at_ns,
                    remaining_ns,
                },
            })
    }

    #[must_use]
    pub fn record_completion(mut self, timestamp_ns: u64) -> Self {
        self.cooldown = self.cooldown.recorded_at(timestamp_ns);
        self
    }

    #[must_use]
    pub fn cooldown_ns(&self) -> u64 {
        self.cooldown.interval_ns().map_or(0, NonZeroU64::get)
    }

    #[must_use]
    pub fn last_refresh_ns(&self) -> Option<u64> {
        self.cooldown.last_event_ns()
    }
}

impl Default for RefreshThrottle {
    fn default() -> Self {
        Self::default_unlimited()
    }
}

impl RefreshTargetStatus {
    #[must_use]
    pub const fn new(target_id: TargetId, last_refresh_ns: Option<u64>) -> Self {
        Self {
            target_id,
            last_refresh_ns,
        }
    }

    #[must_use]
    pub const fn target_id(&self) -> TargetId {
        self.target_id
    }

    #[must_use]
    pub const fn last_refresh_ns(&self) -> Option<u64> {
        self.last_refresh_ns
    }
}

impl RefreshExecutionPlan {
    #[must_use]
    pub const fn new(plan: RefreshPlan, throttle: RefreshThrottle) -> Self {
        Self { plan, throttle }
    }

    #[must_use]
    pub const fn plan(&self) -> &RefreshPlan {
        &self.plan
    }

    #[must_use]
    pub const fn throttle(&self) -> &RefreshThrottle {
        &self.throttle
    }

    #[must_use]
    pub fn into_parts(self) -> (RefreshPlan, RefreshThrottle) {
        (self.plan, self.throttle)
    }
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum RefreshPlanError {
    EmptyPlan,
    OnCooldown {
        ready_at_ns: u64,
        remaining_ns: u64,
    },
    DuplicateTarget {
        target_id: TargetId,
    },
    TargetNotFound {
        target_id: TargetId,
    },
    FutureRefreshTimestamp {
        target_id: TargetId,
        last_refresh_ns: u64,
        current_ns: u64,
    },
}

pub fn build_refresh_plan(enabled_targets: &[TargetId]) -> Result<RefreshPlan, RefreshPlanError> {
    RefreshPlan::new(enabled_targets.to_vec())
}

pub fn build_targeted_refresh_plan(
    targets: &[TargetId],
    enabled_targets: &[TargetId],
) -> Result<RefreshPlan, RefreshPlanError> {
    let plan = RefreshPlan::new(targets.to_vec())?;
    let enabled_set: BTreeSet<_> = enabled_targets.iter().copied().collect();

    for target in plan.targets() {
        if !enabled_set.contains(target) {
            return Err(RefreshPlanError::TargetNotFound { target_id: *target });
        }
    }

    Ok(plan)
}

pub fn refresh_execution_plan(
    targets: &[TargetId],
    cooldown_ns: u64,
    last_refresh_ns: Option<u64>,
) -> Result<RefreshExecutionPlan, RefreshPlanError> {
    let plan = RefreshPlan::new(targets.to_vec())?;
    let throttle = RefreshThrottle::new(cooldown_ns, last_refresh_ns);
    Ok(RefreshExecutionPlan::new(plan, throttle))
}

pub fn build_stale_refresh_plan(
    all_targets: &[RefreshTargetStatus],
    max_age_ns: u64,
    current_ns: u64,
    enabled_targets: &[TargetId],
) -> Result<Option<RefreshPlan>, RefreshPlanError> {
    let stale_targets = filter_stale_targets(all_targets, max_age_ns, current_ns)?;
    if stale_targets.is_empty() {
        return Ok(None);
    }

    build_targeted_refresh_plan(&stale_targets, enabled_targets).map(Some)
}

pub fn filter_stale_targets(
    all_targets: &[RefreshTargetStatus],
    max_age_ns: u64,
    current_ns: u64,
) -> Result<Vec<TargetId>, RefreshPlanError> {
    let mut stale_targets = Vec::new();

    for target in all_targets {
        match target.last_refresh_ns() {
            None => stale_targets.push(target.target_id()),
            Some(last_refresh_ns) if last_refresh_ns > current_ns => {
                return Err(RefreshPlanError::FutureRefreshTimestamp {
                    target_id: target.target_id(),
                    last_refresh_ns,
                    current_ns,
                });
            }
            Some(last_refresh_ns) if current_ns - last_refresh_ns > max_age_ns => {
                stale_targets.push(target.target_id());
            }
            Some(_) => {}
        }
    }

    Ok(stale_targets)
}
