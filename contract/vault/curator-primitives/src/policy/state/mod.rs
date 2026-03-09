//! Policy state container for curator executors.
//!
//! This module defines a lightweight, chain-agnostic policy state that
//! executors can persist alongside the vault kernel state.

use alloc::vec::Vec;

use templar_vault_kernel::TargetId;

use super::cap_group::{CapGroupId, CapGroupRecord};
use super::market_lock::MarketLockSet;
use super::supply_queue::SupplyQueue;

#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq)]
pub struct OrderedMap<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> Default for OrderedMap<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K: PartialEq, V> OrderedMap<K, V> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some((_, existing)) = self
            .entries
            .iter_mut()
            .find(|(candidate, _)| candidate == &key)
        {
            return Some(core::mem::replace(existing, value));
        }
        self.entries.push((key, value));
        None
    }

    #[must_use]
    pub fn get(&self, key: &K) -> Option<&V> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    #[must_use]
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.entries
            .iter_mut()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    #[must_use]
    pub fn contains_key(&self, key: &K) -> bool {
        self.entries.iter().any(|(candidate, _)| candidate == key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(key, value)| (key, value))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.entries.iter_mut().map(|(key, value)| (&*key, value))
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.iter().map(|(key, _)| key)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, value)| value)
    }
}

impl<K: PartialEq, V> FromIterator<(K, V)> for OrderedMap<K, V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut map = Self::new();
        for (key, value) in iter {
            let _ = map.insert(key, value);
        }
        map
    }
}

impl<K, V> IntoIterator for OrderedMap<K, V> {
    type Item = (K, V);
    type IntoIter = alloc::vec::IntoIter<(K, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq)]
pub struct MarketConfig {
    pub enabled: bool,
    pub cap: u128,
    pub cap_group_id: Option<CapGroupId>,
}

impl MarketConfig {
    pub fn new(enabled: bool, cap_group_id: Option<CapGroupId>) -> Self {
        Self {
            enabled,
            cap: 0,
            cap_group_id,
        }
    }

    #[must_use]
    pub fn with_cap(mut self, cap: u128) -> Self {
        self.cap = cap;
        self
    }
}

impl Default for MarketConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cap: 0,
            cap_group_id: None,
        }
    }
}

/// Curator policy state used by executors.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, Default)]
pub struct PolicyState {
    pub markets: OrderedMap<TargetId, MarketConfig>,
    pub principals: OrderedMap<TargetId, u128>,
    pub cap_groups: OrderedMap<CapGroupId, CapGroupRecord>,
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
    pub fn compute_cap_group_totals(&self) -> Vec<(CapGroupId, u128)> {
        let mut totals: Vec<(CapGroupId, u128)> = Vec::new();

        for (target_id, config) in self.markets.iter() {
            let group_id = match &config.cap_group_id {
                Some(id) => id.clone(),
                None => continue,
            };
            let principal = self.principal_for(*target_id);
            if let Some((_, sum)) = totals
                .iter_mut()
                .find(|(existing_group_id, _)| *existing_group_id == group_id)
            {
                *sum = sum.saturating_add(principal);
            } else {
                totals.push((group_id, principal));
            }
        }

        totals
    }

    /// Recompute and update cap group principals in-place.
    pub fn refresh_cap_group_principals(&mut self) {
        let totals = self.compute_cap_group_totals();
        for (group_id, record) in self.cap_groups.iter_mut() {
            let total = totals
                .iter()
                .find(|(candidate, _)| candidate == group_id)
                .map(|(_, sum)| *sum)
                .unwrap_or(0);
            record.principal = total;
        }
    }
}
