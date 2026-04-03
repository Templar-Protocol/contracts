use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use templar_vault_kernel::TargetId;

use super::{
    refresh_plan::{refresh_execution_plan, RefreshExecutionPlan, RefreshPlanError},
    withdraw_route::{withdraw_plan_from_principals, WithdrawPlanEntry, WithdrawRouteError},
};

#[must_use]
pub fn find_first_duplicate<T: Ord + Copy>(items: &[T]) -> Option<T> {
    let mut seen = BTreeSet::new();

    for item in items {
        if !seen.insert(*item) {
            return Some(*item);
        }
    }

    None
}

#[must_use]
pub fn has_unique_items<T: Ord + Copy>(items: &[T]) -> bool {
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
    cooldown_ns: u64,
    last_refresh_ns: Option<u64>,
) -> Result<
    (
        super::refresh_plan::RefreshPlan,
        super::refresh_plan::RefreshThrottle,
    ),
    RefreshPlanError,
> {
    refresh_execution_plan(targets, cooldown_ns, last_refresh_ns)
        .map(RefreshExecutionPlan::into_parts)
}

pub fn refresh_plan(
    targets: &[TargetId],
    cooldown_ns: u64,
    last_refresh_ns: Option<u64>,
) -> Result<RefreshExecutionPlan, RefreshPlanError> {
    refresh_execution_plan(targets, cooldown_ns, last_refresh_ns)
}
