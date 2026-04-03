use alloc::vec::Vec;
use core::num::NonZeroU64;
use templar_vault_kernel::{DurationNs, TargetId, TimestampNs};

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
    last_refresh_at: Option<TimestampNs>,
}

#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RefreshTiming {
    cooldown: DurationNs,
    last_refresh_at: Option<TimestampNs>,
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
    pub fn new(cooldown: DurationNs, last_refresh_at: Option<TimestampNs>) -> Self {
        let cooldown = NonZeroU64::new(cooldown.as_u64())
            .map_or_else(Cooldown::unlimited, Cooldown::new)
            .with_last_event_ns(last_refresh_at.map(TimestampNs::as_u64));

        Self { cooldown }
    }

    #[must_use]
    pub fn cooldown(&self) -> &Cooldown {
        &self.cooldown
    }

    #[must_use]
    pub fn is_ready(&self, current_time: TimestampNs) -> bool {
        self.cooldown.is_ready(current_time.as_u64())
    }

    pub fn check(&self, current_time: TimestampNs) -> Result<(), RefreshPlanError> {
        self.cooldown
            .check(current_time.as_u64())
            .map_err(|e| match e {
                super::cooldown::CooldownError::OnCooldown {
                    ready_at_ns,
                    remaining_ns,
                } => RefreshPlanError::OnCooldown {
                    ready_at: TimestampNs(ready_at_ns),
                    remaining: DurationNs(remaining_ns),
                },
            })
    }

    pub fn try_acquire(self, current_time: TimestampNs) -> Result<Self, RefreshPlanError> {
        self.cooldown
            .try_acquire(current_time.as_u64())
            .map(|cooldown| Self { cooldown })
            .map_err(|e| match e {
                super::cooldown::CooldownError::OnCooldown {
                    ready_at_ns,
                    remaining_ns,
                } => RefreshPlanError::OnCooldown {
                    ready_at: TimestampNs(ready_at_ns),
                    remaining: DurationNs(remaining_ns),
                },
            })
    }

    #[must_use]
    pub fn record_completion(mut self, completed_at: TimestampNs) -> Self {
        self.cooldown = self.cooldown.recorded_at(completed_at.as_u64());
        self
    }

    #[must_use]
    pub fn cooldown_duration(&self) -> DurationNs {
        DurationNs(self.cooldown.interval_ns().map_or(0, NonZeroU64::get))
    }

    #[must_use]
    pub fn last_refresh_at(&self) -> Option<TimestampNs> {
        self.cooldown.last_event_ns().map(TimestampNs)
    }

    #[must_use]
    pub fn cooldown_ns(&self) -> u64 {
        self.cooldown_duration().as_u64()
    }

    #[must_use]
    pub fn last_refresh_ns(&self) -> Option<u64> {
        self.last_refresh_at().map(TimestampNs::as_u64)
    }
}

impl Default for RefreshThrottle {
    fn default() -> Self {
        Self::default_unlimited()
    }
}

impl RefreshTargetStatus {
    #[must_use]
    pub const fn new(target_id: TargetId, last_refresh_at: Option<TimestampNs>) -> Self {
        Self {
            target_id,
            last_refresh_at,
        }
    }

    #[must_use]
    pub const fn target_id(&self) -> TargetId {
        self.target_id
    }

    #[must_use]
    pub const fn last_refresh_at(&self) -> Option<TimestampNs> {
        self.last_refresh_at
    }

    #[must_use]
    pub const fn last_refresh_ns(&self) -> Option<u64> {
        match self.last_refresh_at {
            Some(last_refresh_at) => Some(last_refresh_at.as_u64()),
            None => None,
        }
    }
}

impl RefreshTiming {
    #[must_use]
    pub const fn new(cooldown: DurationNs, last_refresh_at: Option<TimestampNs>) -> Self {
        Self {
            cooldown,
            last_refresh_at,
        }
    }

    #[must_use]
    pub const fn cooldown(&self) -> DurationNs {
        self.cooldown
    }

    #[must_use]
    pub const fn last_refresh_at(&self) -> Option<TimestampNs> {
        self.last_refresh_at
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
        ready_at: TimestampNs,
        remaining: DurationNs,
    },
    DuplicateTarget {
        target_id: TargetId,
    },
    TargetNotFound {
        target_id: TargetId,
    },
    FutureRefreshTimestamp {
        target_id: TargetId,
        last_refresh_at: TimestampNs,
        current_time: TimestampNs,
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

    for target in plan.targets() {
        if !enabled_targets.contains(target) {
            return Err(RefreshPlanError::TargetNotFound { target_id: *target });
        }
    }

    Ok(plan)
}

pub fn refresh_execution_plan(
    targets: &[TargetId],
    timing: RefreshTiming,
) -> Result<RefreshExecutionPlan, RefreshPlanError> {
    let plan = RefreshPlan::new(targets.to_vec())?;
    let throttle = RefreshThrottle::new(timing.cooldown(), timing.last_refresh_at());
    Ok(RefreshExecutionPlan::new(plan, throttle))
}

pub fn build_stale_refresh_plan(
    all_targets: &[RefreshTargetStatus],
    max_age: DurationNs,
    current_time: TimestampNs,
    enabled_targets: &[TargetId],
) -> Result<Option<RefreshPlan>, RefreshPlanError> {
    let stale_targets = filter_stale_targets(all_targets, max_age, current_time)?;
    if stale_targets.is_empty() {
        return Ok(None);
    }

    build_targeted_refresh_plan(&stale_targets, enabled_targets).map(Some)
}

pub fn filter_stale_targets(
    all_targets: &[RefreshTargetStatus],
    max_age: DurationNs,
    current_time: TimestampNs,
) -> Result<Vec<TargetId>, RefreshPlanError> {
    let mut stale_targets = Vec::new();

    for target in all_targets {
        match target.last_refresh_at() {
            None => stale_targets.push(target.target_id()),
            Some(last_refresh_at) if last_refresh_at > current_time => {
                return Err(RefreshPlanError::FutureRefreshTimestamp {
                    target_id: target.target_id(),
                    last_refresh_at,
                    current_time,
                });
            }
            Some(last_refresh_at)
                if current_time
                    .as_u64()
                    .saturating_sub(last_refresh_at.as_u64())
                    > max_age.as_u64() =>
            {
                stale_targets.push(target.target_id());
            }
            Some(_) => {}
        }
    }

    Ok(stale_targets)
}
