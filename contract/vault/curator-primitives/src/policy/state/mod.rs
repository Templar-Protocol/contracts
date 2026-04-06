//! Policy state container for curator executors.
//!
//! This module defines a lightweight, chain-agnostic policy state that
//! executors can persist alongside the vault kernel state.

use alloc::vec::Vec;

use templar_vault_kernel::TargetId;

use super::cap_group::{CapGroupError, CapGroupId, CapGroupRecord};
use super::market_lock::MarketLeaseRegistry;
use super::supply_queue::{SupplyQueue, SupplyQueueError};

#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde, postcard, schemars)]
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
        Self::default()
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

    pub fn retain(&mut self, mut f: impl FnMut(&K, &V) -> bool) {
        self.entries.retain(|(key, value)| f(key, value));
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let index = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate == key)?;
        Some(self.entries.remove(index).1)
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
        let mut map = Self::default();
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

#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct MarketConfig {
    pub enabled: bool,
    pub cap: u128,
    pub cap_group_id: Option<CapGroupId>,
}

impl MarketConfig {
    #[must_use]
    pub fn new(enabled: bool, cap: u128, cap_group_id: Option<CapGroupId>) -> Self {
        Self {
            enabled,
            cap,
            cap_group_id,
        }
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
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Default)]
pub struct PolicyState {
    markets: OrderedMap<TargetId, MarketConfig>,
    principals: OrderedMap<TargetId, u128>,
    cap_groups: OrderedMap<CapGroupId, CapGroupRecord>,
    supply_queue: SupplyQueue,
    leases: MarketLeaseRegistry,
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum PolicyStateError {
    UnknownMarket { target_id: TargetId },
    UnknownCapGroup { id: CapGroupId },
    CapGroupInUse { id: CapGroupId },
    PrincipalOverflow { target_id: TargetId },
    InvalidSupplyQueue { source: SupplyQueueError },
    SupplyQueueUnknownMarket { target_id: TargetId },
    SupplyQueueDisabledMarket { target_id: TargetId },
    SupplyQueueUnauthorizedMarket { target_id: TargetId },
}

impl PolicyState {
    pub fn from_parts(
        markets: OrderedMap<TargetId, MarketConfig>,
        principals: OrderedMap<TargetId, u128>,
        cap_groups: OrderedMap<CapGroupId, CapGroupRecord>,
        leases: MarketLeaseRegistry,
        supply_queue: SupplyQueue,
    ) -> Result<Self, PolicyStateError> {
        let mut state = Self {
            markets,
            principals,
            cap_groups,
            supply_queue,
            leases,
        };
        state
            .supply_queue
            .validate()
            .map_err(|source| PolicyStateError::InvalidSupplyQueue { source })?;
        state.validate_supply_queue_targets()?;
        state.prune_orphan_principals();
        state.initialize_missing_principals();
        state.recompute_cap_group_principals()?;
        Ok(state)
    }

    #[must_use]
    pub fn markets(&self) -> &OrderedMap<TargetId, MarketConfig> {
        &self.markets
    }

    #[must_use]
    pub fn principals(&self) -> &OrderedMap<TargetId, u128> {
        &self.principals
    }

    #[must_use]
    pub fn cap_groups(&self) -> &OrderedMap<CapGroupId, CapGroupRecord> {
        &self.cap_groups
    }

    #[must_use]
    pub fn supply_queue(&self) -> &SupplyQueue {
        &self.supply_queue
    }

    #[must_use]
    pub fn leases(&self) -> &MarketLeaseRegistry {
        &self.leases
    }

    #[must_use]
    pub fn market_config(&self, target_id: TargetId) -> Option<&MarketConfig> {
        self.markets.get(&target_id)
    }

    #[must_use]
    pub fn principal_entry(&self, target_id: TargetId) -> Option<u128> {
        self.principals.get(&target_id).copied()
    }

    #[must_use]
    pub fn cap_group(&self, cap_group_id: &CapGroupId) -> Option<&CapGroupRecord> {
        self.cap_groups.get(cap_group_id)
    }

    pub fn replace_supply_queue(
        &mut self,
        supply_queue: SupplyQueue,
    ) -> Result<(), PolicyStateError> {
        supply_queue
            .validate()
            .map_err(|source| PolicyStateError::InvalidSupplyQueue { source })?;
        self.validate_supply_queue_targets_with(&supply_queue)?;
        self.supply_queue = supply_queue;
        Ok(())
    }

    pub fn set_market_config(
        &mut self,
        target_id: TargetId,
        config: MarketConfig,
    ) -> Result<(), PolicyStateError> {
        self.ensure_known_cap_group(config.cap_group_id.as_ref())?;
        self.markets.insert(target_id, config);
        let _ = self
            .principals
            .insert(target_id, self.principal_entry(target_id).unwrap_or(0));
        self.recompute_cap_group_principals()
    }

    pub fn set_market_cap(
        &mut self,
        target_id: TargetId,
        cap: u128,
    ) -> Result<(), PolicyStateError> {
        let config = self
            .markets
            .get_mut(&target_id)
            .ok_or(PolicyStateError::UnknownMarket { target_id })?;
        config.cap = cap;
        if cap == 0 {
            self.supply_queue.remove_target(target_id);
        }
        Ok(())
    }

    pub fn set_market_enabled(
        &mut self,
        target_id: TargetId,
        enabled: bool,
    ) -> Result<(), PolicyStateError> {
        let config = self
            .markets
            .get_mut(&target_id)
            .ok_or(PolicyStateError::UnknownMarket { target_id })?;
        config.enabled = enabled;
        if !enabled {
            self.supply_queue.remove_target(target_id);
        }
        Ok(())
    }

    pub fn set_market_cap_group(
        &mut self,
        target_id: TargetId,
        cap_group_id: Option<CapGroupId>,
    ) -> Result<(), PolicyStateError> {
        self.ensure_known_cap_group(cap_group_id.as_ref())?;
        let config = self
            .markets
            .get_mut(&target_id)
            .ok_or(PolicyStateError::UnknownMarket { target_id })?;
        config.cap_group_id = cap_group_id;
        self.recompute_cap_group_principals()
    }

    pub fn set_principal(
        &mut self,
        target_id: TargetId,
        principal: u128,
    ) -> Result<(), PolicyStateError> {
        if !self.markets.contains_key(&target_id) {
            return Err(PolicyStateError::UnknownMarket { target_id });
        }
        let _ = self.principals.insert(target_id, principal);
        self.recompute_cap_group_principals()
    }

    pub fn remove_market(
        &mut self,
        target_id: TargetId,
    ) -> Result<Option<MarketConfig>, PolicyStateError> {
        let removed = self.markets.remove(&target_id);
        if removed.is_some() {
            let _ = self.principals.remove(&target_id);
            self.supply_queue.remove_target(target_id);
            self.recompute_cap_group_principals()?;
        }
        Ok(removed)
    }

    pub fn remove_cap_group(
        &mut self,
        cap_group_id: &CapGroupId,
    ) -> Result<Option<CapGroupRecord>, PolicyStateError> {
        if self
            .markets
            .values()
            .any(|config| config.cap_group_id.as_ref() == Some(cap_group_id))
        {
            return Err(PolicyStateError::CapGroupInUse {
                id: cap_group_id.clone(),
            });
        }

        Ok(self.cap_groups.remove(cap_group_id))
    }

    #[must_use]
    pub fn principal_for(&self, target_id: TargetId) -> Option<u128> {
        self.principal_entry(target_id)
    }

    /// Compute total external assets from all principals.
    #[must_use]
    pub fn external_assets(&self) -> u128 {
        self.principals
            .values()
            .fold(0u128, |acc, p| acc.checked_add(*p).unwrap())
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
            let principal = self.principal_entry(*target_id).unwrap_or(0);
            if let Some((_, sum)) = totals
                .iter_mut()
                .find(|(existing_group_id, _)| *existing_group_id == group_id)
            {
                *sum = sum.checked_add(principal).unwrap();
            } else {
                totals.push((group_id, principal));
            }
        }

        totals
    }

    pub fn ensure_cap_group(&mut self, cap_group_id: CapGroupId) {
        if !self.cap_groups.contains_key(&cap_group_id) {
            let _ = self
                .cap_groups
                .insert(cap_group_id, CapGroupRecord::default());
        }
    }

    pub fn set_cap_group_absolute_cap(&mut self, cap_group_id: CapGroupId, new_cap: Option<u128>) {
        self.ensure_cap_group(cap_group_id.clone());
        let record = self.cap_groups.get_mut(&cap_group_id).unwrap();
        record.cap.set_absolute_cap(new_cap);
    }

    pub fn set_cap_group_relative_cap(
        &mut self,
        cap_group_id: CapGroupId,
        new_relative_cap: Option<templar_vault_kernel::Wad>,
    ) {
        self.ensure_cap_group(cap_group_id.clone());
        let record = self.cap_groups.get_mut(&cap_group_id).unwrap();
        record.cap.set_relative_cap(new_relative_cap);
    }

    pub fn recompute_cap_group_principals(&mut self) -> Result<(), PolicyStateError> {
        self.validate_cap_group_memberships()?;
        let totals = self.compute_cap_group_totals();

        for (group_id, record) in self.cap_groups.iter_mut() {
            let total = totals
                .iter()
                .find(|(candidate, _)| candidate == group_id)
                .map(|(_, sum)| *sum)
                .unwrap_or(0);
            record.principal = total;
        }

        Ok(())
    }

    pub fn prune_unused_cap_groups(&mut self) {
        self.cap_groups.retain(|group_id, _| {
            self.markets
                .values()
                .any(|config| config.cap_group_id.as_ref() == Some(group_id))
        });
    }

    pub fn prune_zero_principals(&mut self) {
        self.principals
            .retain(|target_id, principal| *principal != 0 && self.markets.contains_key(target_id));
    }

    fn initialize_missing_principals(&mut self) {
        let market_ids: Vec<TargetId> = self.markets.keys().copied().collect();
        for target_id in market_ids {
            if !self.principals.contains_key(&target_id) {
                let _ = self.principals.insert(target_id, 0);
            }
        }
    }

    fn validate_supply_queue_targets(&self) -> Result<(), PolicyStateError> {
        self.validate_supply_queue_targets_with(&self.supply_queue)
    }

    fn validate_supply_queue_targets_with(
        &self,
        supply_queue: &SupplyQueue,
    ) -> Result<(), PolicyStateError> {
        for entry in supply_queue.entries() {
            let target_id = entry.target_id;
            let config = self
                .market_config(target_id)
                .ok_or(PolicyStateError::SupplyQueueUnknownMarket { target_id })?;

            if !config.enabled {
                return Err(PolicyStateError::SupplyQueueDisabledMarket { target_id });
            }

            if config.cap == 0 {
                return Err(PolicyStateError::SupplyQueueUnauthorizedMarket { target_id });
            }
        }

        Ok(())
    }

    fn prune_orphan_principals(&mut self) {
        self.principals
            .retain(|target_id, _| self.markets.contains_key(target_id));
    }

    fn ensure_known_cap_group(
        &self,
        cap_group_id: Option<&CapGroupId>,
    ) -> Result<(), PolicyStateError> {
        match cap_group_id {
            Some(id) if !self.cap_groups.contains_key(id) => {
                Err(PolicyStateError::UnknownCapGroup { id: id.clone() })
            }
            _ => Ok(()),
        }
    }

    fn validate_cap_group_memberships(&self) -> Result<(), PolicyStateError> {
        for (_, config) in self.markets.iter() {
            self.ensure_known_cap_group(config.cap_group_id.as_ref())?;
        }

        Ok(())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.leases.is_empty()
            && self.markets.is_empty()
            && self.principals.is_empty()
            && self.cap_groups.is_empty()
            && self.supply_queue.is_empty()
    }
}

impl From<PolicyStateError> for CapGroupError {
    fn from(err: PolicyStateError) -> Self {
        match err {
            PolicyStateError::UnknownCapGroup { id } => Self::NotFound { id },
            PolicyStateError::CapGroupInUse { id } => Self::InconsistentRecord { id },
            PolicyStateError::PrincipalOverflow { target_id: _ }
            | PolicyStateError::UnknownMarket { target_id: _ }
            | PolicyStateError::InvalidSupplyQueue { source: _ }
            | PolicyStateError::SupplyQueueUnknownMarket { target_id: _ }
            | PolicyStateError::SupplyQueueDisabledMarket { target_id: _ }
            | PolicyStateError::SupplyQueueUnauthorizedMarket { target_id: _ } => {
                Self::InconsistentRecord {
                    id: CapGroupId::policy_state_sentinel(),
                }
            }
        }
    }
}
