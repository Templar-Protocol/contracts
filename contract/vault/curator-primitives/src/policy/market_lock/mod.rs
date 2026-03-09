//! Market locks for preventing concurrent operations on the same market.

use alloc::vec::Vec;
use templar_vault_kernel::TargetId;
use templar_vault_kernel::TimeGate;
use typed_builder::TypedBuilder;

pub fn validate_lock_expiry(current_ns: u64, expiry_ns: u64, max_duration_ns: u64) -> bool {
    let max_expiry_ns = TimeGate::schedule_from(current_ns, max_duration_ns)
        .ready_at_ns()
        .unwrap_or(current_ns);
    expiry_ns > current_ns && expiry_ns <= max_expiry_ns
}

/// A lock on a specific market/target.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[cfg_attr(all(feature = "borsh", feature = "std"), derive(borsh::BorshSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[derive(Clone, PartialEq, Eq, TypedBuilder)]
#[builder(field_defaults(setter(into)))]
pub struct MarketLock {
    pub target_id: TargetId,
    #[builder(default, setter(strip_option))]
    pub op_id: Option<u64>,
    pub locked_at_ns: u64,
    /// Optional expiry timestamp (nanoseconds). `None` means no expiry.
    #[builder(default, setter(strip_option))]
    pub expires_at_ns: Option<u64>,
}

impl MarketLock {
    fn expiry_gate(&self) -> Option<TimeGate> {
        self.expires_at_ns.map(TimeGate::from_ready_at)
    }

    #[must_use]
    pub fn new(target_id: TargetId, locked_at_ns: u64) -> Self {
        Self {
            target_id,
            op_id: None,
            locked_at_ns,
            expires_at_ns: None,
        }
    }

    /// Fluent method: set time-to-live from locked_at timestamp.
    /// This computes `expires_at_ns = locked_at_ns + ttl_ns`.
    #[must_use]
    pub fn with_ttl(mut self, ttl_ns: u64) -> Self {
        self.expires_at_ns = TimeGate::schedule_from(self.locked_at_ns, ttl_ns).ready_at_ns();
        self
    }

    #[must_use]
    pub fn is_expired(&self, current_ns: u64) -> bool {
        self.expiry_gate()
            .map_or(false, |gate| gate.is_ready(current_ns))
    }

    #[must_use]
    pub fn remaining(&self, current_ns: u64) -> Option<u64> {
        self.expiry_gate().map(|gate| gate.remaining(current_ns))
    }
}

/// A set of market locks.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[cfg_attr(all(feature = "borsh", feature = "std"), derive(borsh::BorshSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[derive(Clone, Default)]
pub struct MarketLockSet {
    pub locks: Vec<MarketLock>,
}

impl MarketLockSet {
    #[must_use]
    pub fn new() -> Self {
        Self { locks: Vec::new() }
    }

    /// Iterator over active (non-expired) locks.
    fn active_iter(&self, current_ns: u64) -> impl Iterator<Item = &MarketLock> + '_ {
        self.locks.iter().filter(move |l| !l.is_expired(current_ns))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.locks.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.locks.len()
    }

    #[must_use]
    pub fn active_count(&self, current_ns: u64) -> usize {
        self.active_iter(current_ns).count()
    }

    #[must_use]
    pub fn is_all_expired(&self, current_ns: u64) -> bool {
        self.active_count(current_ns) == 0
    }

    #[must_use]
    pub fn is_locked(&self, target_id: TargetId, current_ns: u64) -> bool {
        self.active_iter(current_ns)
            .any(|lock| lock.target_id == target_id)
    }

    #[must_use]
    pub fn is_locked_by_op(&self, target_id: TargetId, op_id: u64) -> bool {
        self.locks
            .iter()
            .any(|lock| lock.target_id == target_id && lock.op_id == Some(op_id))
    }

    #[must_use]
    pub fn get_lock(&self, target_id: TargetId, current_ns: u64) -> Option<&MarketLock> {
        self.active_iter(current_ns)
            .find(|l| l.target_id == target_id)
    }

    /// Acquire a lock, returning an updated lock set or the existing lock on conflict.
    pub fn acquire(&self, lock: MarketLock, current_ns: u64) -> Result<Self, MarketLock> {
        if let Some(existing) = self
            .active_iter(current_ns)
            .find(|l| l.target_id == lock.target_id)
        {
            return Err(existing.clone());
        }

        let mut new_set = self.clone();
        // Remove any expired locks for this target
        new_set
            .locks
            .retain(|l| l.target_id != lock.target_id || !l.is_expired(current_ns));
        new_set.locks.push(lock);
        Ok(new_set)
    }

    #[must_use]
    pub fn release(&self, target_id: TargetId) -> Self {
        let mut new_set = self.clone();
        new_set.locks.retain(|l| l.target_id != target_id);
        new_set
    }

    /// Release a lock held by a specific operation.
    #[must_use]
    pub fn release_by_op(&self, target_id: TargetId, op_id: u64) -> Self {
        let mut new_set = self.clone();
        new_set
            .locks
            .retain(|l| l.target_id != target_id || l.op_id != Some(op_id));
        new_set
    }

    /// Release all locks held by a specific operation.
    #[must_use]
    pub fn release_all_by_op(&self, op_id: u64) -> Self {
        let mut new_set = self.clone();
        new_set.locks.retain(|l| l.op_id != Some(op_id));
        new_set
    }

    /// Clear all locks (emergency reset).
    #[must_use]
    pub fn clear(&self) -> Self {
        Self::new()
    }

    /// Clean up expired locks.
    #[must_use]
    pub fn cleanup_expired(&self, current_ns: u64) -> Self {
        let mut new_set = self.clone();
        new_set.locks.retain(|l| !l.is_expired(current_ns));
        new_set
    }

    /// Get all currently locked target IDs.
    #[must_use]
    pub fn locked_targets(&self, current_ns: u64) -> Vec<TargetId> {
        self.active_iter(current_ns).map(|l| l.target_id).collect()
    }

    /// Check if any of the targets in a list are locked.
    #[must_use]
    pub fn find_locked_targets(&self, targets: &[TargetId], current_ns: u64) -> Vec<TargetId> {
        targets
            .iter()
            .copied()
            .filter(|t| self.is_locked(*t, current_ns))
            .collect()
    }
}

impl From<Vec<MarketLock>> for MarketLockSet {
    fn from(locks: Vec<MarketLock>) -> Self {
        Self { locks }
    }
}
