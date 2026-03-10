//! Shared helpers for filtering locked targets from plans.

use alloc::vec::Vec;

use templar_vault_kernel::TargetId;

use super::{
    market_lock::MarketLockSet,
    supply_queue::{SupplyQueue, SupplyQueueEntry},
    withdraw_route::{WithdrawRoute, WithdrawRouteEntry, WithdrawRouteError},
};

impl MarketLockSet {
    /// Filter a target list to only unlocked targets.
    #[must_use]
    pub fn filter_unlocked_targets(&self, targets: &[TargetId], current_ns: u64) -> Vec<TargetId> {
        targets
            .iter()
            .copied()
            .filter(|target| !self.is_locked(*target, current_ns))
            .collect()
    }

    /// Filter an allocation plan to only unlocked targets.
    #[must_use]
    pub fn filter_allocation_plan(
        &self,
        plan: &[(TargetId, u128)],
        current_ns: u64,
    ) -> Vec<(TargetId, u128)> {
        plan.iter()
            .copied()
            .filter(|(target_id, _)| !self.is_locked(*target_id, current_ns))
            .collect()
    }

    /// Filter a supply queue to only unlocked targets.
    #[must_use]
    pub fn filter_supply_queue(&self, queue: &SupplyQueue, current_ns: u64) -> SupplyQueue {
        let entries: Vec<SupplyQueueEntry> = queue
            .entries
            .iter()
            .filter(|entry| !self.is_locked(entry.target_id, current_ns))
            .cloned()
            .collect();

        SupplyQueue {
            entries,
            max_length: queue.max_length,
        }
    }

    /// Filter a withdraw route to only unlocked targets.
    pub fn filter_withdraw_route(
        &self,
        route: &WithdrawRoute,
        current_ns: u64,
    ) -> Result<WithdrawRoute, WithdrawRouteError> {
        let entries: Vec<WithdrawRouteEntry> = route
            .entries
            .iter()
            .filter(|entry| !self.is_locked(entry.target_id, current_ns))
            .cloned()
            .collect();

        let filtered = WithdrawRoute::from_entries(entries, route.target_amount);
        filtered.validate()?;
        Ok(filtered)
    }

    /// Build an allocation plan from a queue while excluding locked targets.
    #[must_use]
    pub fn build_allocation_plan_with_locks(
        &self,
        queue: &SupplyQueue,
        current_ns: u64,
    ) -> Vec<(TargetId, u128)> {
        self.filter_supply_queue(queue, current_ns)
            .to_allocation_plan()
    }

    /// Build a withdraw plan from a route while excluding locked targets.
    pub fn build_withdrawal_plan_with_locks(
        &self,
        route: &WithdrawRoute,
        current_ns: u64,
    ) -> Result<Vec<(TargetId, u128)>, WithdrawRouteError> {
        Ok(self
            .filter_withdraw_route(route, current_ns)?
            .to_withdrawal_plan())
    }

    /// Build a refresh target list while excluding locked targets.
    #[must_use]
    pub fn build_refresh_plan_with_locks(
        &self,
        targets: &[TargetId],
        current_ns: u64,
    ) -> Vec<TargetId> {
        self.filter_unlocked_targets(targets, current_ns)
    }
}
