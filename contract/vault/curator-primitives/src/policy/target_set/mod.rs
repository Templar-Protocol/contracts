use alloc::vec::Vec;
use templar_vault_kernel::{DurationNs, TargetId, TimestampNs};

use super::{
    refresh_plan::{refresh_execution_plan, RefreshExecutionPlan, RefreshPlanError, RefreshTiming},
    withdraw_route::{withdraw_plan_from_principals, WithdrawPlanEntry, WithdrawRouteError},
};

#[must_use]
pub fn find_first_duplicate<T: PartialEq + Copy>(items: &[T]) -> Option<T> {
    for (index, item) in items.iter().enumerate() {
        if items[index + 1..].contains(item) {
            return Some(*item);
        }
    }

    None
}

#[must_use]
pub fn has_unique_items<T: PartialEq + Copy>(items: &[T]) -> bool {
    find_first_duplicate(items).is_none()
}

pub fn build_withdraw_capacity_pairs_from_target_principals(
    principals: &[(TargetId, u128)],
    target_amount: u128,
) -> Result<Vec<(TargetId, u128)>, WithdrawRouteError> {
    withdraw_plan_from_principals(principals, target_amount)
        .map(|plan| plan.into_iter().map(Into::into).collect())
}

pub fn withdraw_plan(
    principals: &[(TargetId, u128)],
    target_amount: u128,
) -> Result<Vec<WithdrawPlanEntry>, WithdrawRouteError> {
    withdraw_plan_from_principals(principals, target_amount)
}

pub fn build_refresh_plan_from_targets(
    targets: &[TargetId],
    cooldown: DurationNs,
    last_refresh_at: Option<TimestampNs>,
) -> Result<
    (
        super::refresh_plan::RefreshPlan,
        super::refresh_plan::RefreshThrottle,
    ),
    RefreshPlanError,
> {
    refresh_execution_plan(targets, RefreshTiming::new(cooldown, last_refresh_at))
        .map(RefreshExecutionPlan::into_parts)
}

pub fn refresh_plan(
    targets: &[TargetId],
    cooldown: DurationNs,
    last_refresh_at: Option<TimestampNs>,
) -> Result<RefreshExecutionPlan, RefreshPlanError> {
    refresh_execution_plan(targets, RefreshTiming::new(cooldown, last_refresh_at))
}

pub fn refresh_plan_with_timing(
    targets: &[TargetId],
    timing: RefreshTiming,
) -> Result<RefreshExecutionPlan, RefreshPlanError> {
    refresh_execution_plan(targets, timing)
}
