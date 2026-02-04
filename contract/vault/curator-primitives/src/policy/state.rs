//! Policy state container for curator executors.
//!
//! This module defines a lightweight, chain-agnostic policy state that
//! executors can persist alongside the vault kernel state.

use alloc::collections::BTreeMap;

use templar_vault_kernel::TargetId;

use super::cap_group::{CapGroupId, CapGroupRecord};
use super::market_lock::MarketLockSet;
use super::supply_queue::SupplyQueue;

#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketConfig {
    pub enabled: bool,
    pub cap_group_id: Option<CapGroupId>,
}

impl MarketConfig {
    pub fn new(enabled: bool, cap_group_id: Option<CapGroupId>) -> Self {
        Self {
            enabled,
            cap_group_id,
        }
    }
}

impl Default for MarketConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cap_group_id: None,
        }
    }
}

/// Curator policy state used by executors.
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct PolicyState {
    pub markets: BTreeMap<TargetId, MarketConfig>,
    pub principals: BTreeMap<TargetId, u128>,
    pub cap_groups: BTreeMap<CapGroupId, CapGroupRecord>,
    pub supply_queue: SupplyQueue,
    pub locks: MarketLockSet,
}

impl PolicyState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_market_config(&mut self, target_id: TargetId, config: MarketConfig) {
        self.markets.insert(target_id, config);
    }

    pub fn set_principal(&mut self, target_id: TargetId, principal: u128) {
        self.principals.insert(target_id, principal);
    }

    /// Return the principal for a market (0 if missing).
    pub fn principal_for(&self, target_id: TargetId) -> u128 {
        self.principals.get(&target_id).copied().unwrap_or(0)
    }

    /// Compute total external assets from all principals.
    #[must_use]
    pub fn external_assets(&self) -> u128 {
        self.principals
            .values()
            .fold(0u128, |acc, p| acc.saturating_add(*p))
    }

    /// Compute principal totals per cap group.
    ///
    /// Aggregates principals for all markets assigned to each cap group.
    #[must_use]
    pub fn compute_cap_group_totals(&self) -> BTreeMap<CapGroupId, u128> {
        let mut totals: BTreeMap<CapGroupId, u128> = BTreeMap::new();

        for (target_id, config) in &self.markets {
            let group_id = match &config.cap_group_id {
                Some(id) => id.clone(),
                None => continue,
            };
            let principal = self.principal_for(*target_id);
            let entry = totals.entry(group_id).or_insert(0);
            *entry = entry.saturating_add(principal);
        }

        totals
    }

    /// Recompute and update cap group principals in-place.
    pub fn refresh_cap_group_principals(&mut self) {
        let totals = self.compute_cap_group_totals();
        for (group_id, record) in self.cap_groups.iter_mut() {
            let total = totals.get(group_id).copied().unwrap_or(0);
            record.principal = total;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn external_assets_sums_principals() {
        let mut state = PolicyState::new();
        state.set_principal(1, 100);
        state.set_principal(2, 250);
        state.set_principal(3, 50);

        assert_eq!(state.external_assets(), 400);
    }

    #[test]
    fn cap_group_totals_aggregate_by_group() {
        let mut state = PolicyState::new();
        let group_a = CapGroupId::new("group-a");
        let group_b = CapGroupId::new("group-b");

        state.set_market_config(1, MarketConfig::new(true, Some(group_a.clone())));
        state.set_market_config(2, MarketConfig::new(true, Some(group_a.clone())));
        state.set_market_config(3, MarketConfig::new(true, Some(group_b.clone())));

        state.set_principal(1, 10);
        state.set_principal(2, 20);
        state.set_principal(3, 40);

        let totals = state.compute_cap_group_totals();
        assert_eq!(totals.get(&group_a).copied().unwrap_or(0), 30);
        assert_eq!(totals.get(&group_b).copied().unwrap_or(0), 40);
    }

    #[test]
    fn refresh_cap_group_principals_updates_records() {
        let mut state = PolicyState::new();
        let group = CapGroupId::new(String::from("group"));
        state.cap_groups.insert(group.clone(), CapGroupRecord::default());
        state.set_market_config(1, MarketConfig::new(true, Some(group.clone())));
        state.set_principal(1, 123);

        state.refresh_cap_group_principals();

        let record = state.cap_groups.get(&group).expect("cap group");
        assert_eq!(record.principal, 123);
    }
}
