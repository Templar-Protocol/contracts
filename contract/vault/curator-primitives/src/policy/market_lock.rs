//! Market locks for preventing concurrent operations on the same market.

use alloc::vec::Vec;
use templar_vault_kernel::TargetId;
use typed_builder::TypedBuilder;

/// A lock on a specific market/target.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq, TypedBuilder)]
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
    #[must_use]
    pub fn new(target_id: TargetId, locked_at_ns: u64) -> Self {
        Self {
            target_id,
            op_id: None,
            locked_at_ns,
            expires_at_ns: None,
        }
    }

    #[must_use]
    pub fn with_op_id(mut self, op_id: u64) -> Self {
        self.op_id = Some(op_id);
        self
    }

    #[must_use]
    pub fn with_expiry(mut self, expires_at_ns: u64) -> Self {
        self.expires_at_ns = Some(expires_at_ns);
        self
    }

    /// Fluent method: set time-to-live from locked_at timestamp.
    /// This computes `expires_at_ns = locked_at_ns + ttl_ns`.
    #[must_use]
    pub fn with_ttl(mut self, ttl_ns: u64) -> Self {
        self.expires_at_ns = Some(self.locked_at_ns.saturating_add(ttl_ns));
        self
    }

    #[must_use]
    pub fn is_expired(&self, current_ns: u64) -> bool {
        match self.expires_at_ns {
            Some(expiry) => current_ns >= expiry,
            None => false,
        }
    }

    #[must_use]
    pub fn is_active(&self, current_ns: u64) -> bool {
        !self.is_expired(current_ns)
    }

    #[must_use]
    pub fn remaining(&self, current_ns: u64) -> Option<u64> {
        self.expires_at_ns.map(|expiry| expiry.saturating_sub(current_ns))
    }
}

/// A set of market locks.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
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
        self.locks.iter().filter(move |l| l.is_active(current_ns))
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
            .retain(|l| l.target_id != lock.target_id || l.is_active(current_ns));
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
        new_set.locks.retain(|l| l.is_active(current_ns));
        new_set
    }

    /// Get all currently locked target IDs.
    #[must_use]
    pub fn locked_targets(&self, current_ns: u64) -> Vec<TargetId> {
        self.active_iter(current_ns)
            .map(|l| l.target_id)
            .collect()
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


#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_new_lock_set_is_empty() {
        let set = MarketLockSet::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert_eq!(set.active_count(0), 0);
    }

    #[test]
    fn test_acquire_lock() {
        let set = MarketLockSet::new();
        let lock = MarketLock::new(1, 1000);

        let result = set.acquire(lock, 1000).unwrap();

        assert_eq!(result.len(), 1);
        assert!(result.is_locked(1, 1000));
    }

    #[test]
    fn test_acquire_lock_already_locked() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::new(1, 2000);

        let set = set.acquire(lock1, 1000).unwrap();
        let result = set.acquire(lock2, 2000);

        assert!(result.is_err());
    }

    #[test]
    fn test_acquire_lock_different_target() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::new(2, 2000);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 2000).unwrap();

        assert_eq!(set.len(), 2);
        assert!(set.is_locked(1, 2000));
        assert!(set.is_locked(2, 2000));
    }

    #[test]
    fn test_acquire_lock_after_expiry() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000).with_expiry(2000);
        let lock2 = MarketLock::new(1, 3000);

        let set = set.acquire(lock1, 1000).unwrap();

        // Should fail before expiry
        let result = set.acquire(lock2.clone(), 1500);
        assert!(result.is_err());

        // Should succeed after expiry
        let set = set.acquire(lock2, 3000).unwrap();
        assert_eq!(set.len(), 1); // Old expired lock removed
        assert!(set.is_locked(1, 3000));
    }

    #[test]
    fn test_release_lock() {
        let set = MarketLockSet::new();
        let lock = MarketLock::new(1, 1000);

        let set = set.acquire(lock, 1000).unwrap();
        let set = set.release(1);

        assert!(set.is_empty());
        assert!(!set.is_locked(1, 2000));
    }

    #[test]
    fn test_release_lock_by_op() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000).with_op_id(100);
        let lock2 = MarketLock::new(2, 1000).with_op_id(200);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();

        // Release only the lock held by op 100
        let set = set.release_by_op(1, 100);

        assert_eq!(set.len(), 1);
        assert!(!set.is_locked(1, 2000));
        assert!(set.is_locked(2, 2000));
    }

    #[test]
    fn test_release_all_by_op() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000).with_op_id(100);
        let lock2 = MarketLock::new(2, 1000).with_op_id(100);
        let lock3 = MarketLock::new(3, 1000).with_op_id(200);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();
        let set = set.acquire(lock3, 1000).unwrap();

        let set = set.release_all_by_op(100);

        assert_eq!(set.len(), 1);
        assert!(!set.is_locked(1, 2000));
        assert!(!set.is_locked(2, 2000));
        assert!(set.is_locked(3, 2000));
    }

    #[test]
    fn test_is_locked_by_op() {
        let set = MarketLockSet::new();
        let lock = MarketLock::new(1, 1000).with_op_id(100);

        let set = set.acquire(lock, 1000).unwrap();

        assert!(set.is_locked_by_op(1, 100));
        assert!(!set.is_locked_by_op(1, 200));
        assert!(!set.is_locked_by_op(2, 100));
    }

    #[test]
    fn test_lock_expiry() {
        let lock = MarketLock::new(1, 1000).with_expiry(2000);

        assert!(!lock.is_expired(1000));
        assert!(!lock.is_expired(1999));
        assert!(lock.is_expired(2000));
        assert!(lock.is_expired(3000));
    }

    #[test]
    fn test_lock_no_expiry() {
        let lock = MarketLock::new(1, 1000);

        // No expiry means never expires
        assert!(!lock.is_expired(u64::MAX));
        assert!(lock.expires_at_ns.is_none());
    }

    #[test]
    fn test_lock_with_ttl() {
        let lock = MarketLock::new(1, 1000).with_ttl(500);
        assert_eq!(lock.expires_at_ns, Some(1500));
    }

    #[test]
    fn test_lock_remaining() {
        let lock = MarketLock::new(1, 1000).with_expiry(2000);
        assert_eq!(lock.remaining(1000), Some(1000));
        assert_eq!(lock.remaining(1500), Some(500));
        assert_eq!(lock.remaining(2000), Some(0));

        let no_expiry = MarketLock::new(1, 1000);
        assert_eq!(no_expiry.remaining(5000), None);
    }

    #[test]
    fn test_cleanup_expired_locks() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000).with_expiry(2000);
        let lock2 = MarketLock::new(2, 1000).with_expiry(3000);
        let lock3 = MarketLock::new(3, 1000); // no expiry

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();
        let set = set.acquire(lock3, 1000).unwrap();

        let cleaned = set.cleanup_expired(2500);

        assert_eq!(cleaned.len(), 2);
        assert!(!cleaned.is_locked(1, 2500)); // expired
        assert!(cleaned.is_locked(2, 2500)); // not yet expired
        assert!(cleaned.is_locked(3, 2500)); // no expiry
    }

    #[test]
    fn test_get_locked_targets() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::new(2, 1000).with_expiry(1500);
        let lock3 = MarketLock::new(3, 1000);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();
        let set = set.acquire(lock3, 1000).unwrap();

        let locked = set.locked_targets(2000);

        assert_eq!(locked.len(), 2);
        assert!(locked.contains(&1));
        assert!(!locked.contains(&2)); // expired
        assert!(locked.contains(&3));
    }

    #[test]
    fn test_find_locked_targets() {
        let set = MarketLockSet::new();
        let lock = MarketLock::new(2, 1000);

        let set = set.acquire(lock, 1000).unwrap();

        let to_check = vec![1, 2, 3, 4];
        let locked = set.find_locked_targets(&to_check, 2000);

        assert_eq!(locked, vec![2]);
    }

    #[test]
    fn test_clear_all_locks() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::new(2, 1000);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();

        let cleared = set.clear();

        assert!(cleared.is_empty());
    }

    #[test]
    fn test_active_count() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000).with_expiry(2000);
        let lock2 = MarketLock::new(2, 1000).with_expiry(3000);
        let lock3 = MarketLock::new(3, 1000);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();
        let set = set.acquire(lock3, 1000).unwrap();

        assert_eq!(set.len(), 3); // Total locks
        assert_eq!(set.active_count(1500), 3); // All active
        assert_eq!(set.active_count(2500), 2); // lock1 expired
        assert_eq!(set.active_count(3500), 1); // lock1 and lock2 expired
    }

    #[test]
    fn test_get_lock() {
        let set = MarketLockSet::new();
        let lock = MarketLock::new(1, 1000).with_op_id(42);

        let set = set.acquire(lock, 1000).unwrap();

        let found = set.get_lock(1, 1500);
        assert!(found.is_some());
        assert_eq!(found.unwrap().op_id, Some(42));

        let not_found = set.get_lock(2, 1500);
        assert!(not_found.is_none());
    }

    #[test]
    fn test_is_all_expired() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000).with_expiry(2000);
        let lock2 = MarketLock::new(2, 1000).with_expiry(2000);

        let set = set.acquire(lock1, 1000).unwrap();
        let set = set.acquire(lock2, 1000).unwrap();

        assert!(!set.is_all_expired(1500));
        assert!(set.is_all_expired(2500));
    }
}
