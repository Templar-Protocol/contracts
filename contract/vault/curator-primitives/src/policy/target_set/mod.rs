use alloc::vec::Vec;
use templar_vault_kernel::TargetId;

use super::{
    refresh_plan::{RefreshPlan, RefreshPlanError, RefreshThrottle},
    withdraw_route::{build_withdraw_route, WithdrawRouteError},
};

/// Build a withdraw plan from target principals.
pub fn build_withdraw_plan_from_target_principals(
    principals: &[(TargetId, u128)],
    target_amount: u128,
) -> Result<Vec<(TargetId, u128)>, WithdrawRouteError> {
    build_withdraw_route(principals, target_amount).map(|route| route.to_target_amount_pairs())
}

/// Build and validate a refresh plan from target IDs.
pub fn build_refresh_plan_from_targets(
    targets: &[TargetId],
    cooldown_ns: u64,
    last_refresh_ns: Option<u64>,
) -> Result<(RefreshPlan, RefreshThrottle), RefreshPlanError> {
    let plan = RefreshPlan::new(targets.to_vec())?;
    let throttle = RefreshThrottle::new(cooldown_ns, last_refresh_ns);
    Ok((plan, throttle))
}
