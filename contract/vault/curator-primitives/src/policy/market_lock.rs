//! Market locks for preventing concurrent operations on the same market.
//!
//! Market locks ensure that only one operation can be in progress for a given
//! market at a time, preventing race conditions in allocation and withdrawal flows.

use alloc::vec::Vec;
use templar_vault_kernel::TargetId;

/// A lock on a specific market/target.
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketLock {
    /// The target ID that is locked.
    pub target_id: TargetId,
    /// Optional operation ID that holds the lock.
    pub op_id: Option<u64>,
    /// Timestamp when the lock was acquired (nanoseconds).
    pub locked_at_ns: u64,
    /// Optional expiry timestamp (nanoseconds). 0 means no expiry.
    pub expires_at_ns: u64,
}

impl MarketLock {
    /// Create a new market lock.
    pub fn new(target_id: TargetId, locked_at_ns: u64) -> Self {
        Self {
            target_id,
            op_id: None,
            locked_at_ns,
            expires_at_ns: 0,
        }
    }

    /// Create a new lock with an operation ID.
    pub fn with_op_id(target_id: TargetId, op_id: u64, locked_at_ns: u64) -> Self {
        Self {
            target_id,
            op_id: Some(op_id),
            locked_at_ns,
            expires_at_ns: 0,
        }
    }

    /// Create a new lock with expiry.
    pub fn with_expiry(target_id: TargetId, locked_at_ns: u64, expires_at_ns: u64) -> Self {
        Self {
            target_id,
            op_id: None,
            locked_at_ns,
            expires_at_ns,
        }
    }

    /// Check if the lock has expired.
    pub fn is_expired(&self, current_ns: u64) -> bool {
        self.expires_at_ns > 0 && current_ns >= self.expires_at_ns
    }
}

/// A set of market locks.
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct MarketLockSet {
    /// Active locks.
    pub locks: Vec<MarketLock>,
}

impl MarketLockSet {
    /// Create a new empty lock set.
    pub fn new() -> Self {
        Self { locks: Vec::new() }
    }

    /// Returns true if there are no active locks.
    pub fn is_empty(&self) -> bool {
        self.locks.is_empty()
    }

    /// Returns the number of active locks.
    pub fn len(&self) -> usize {
        self.locks.len()
    }
}

/// Check if a market is locked.
///
/// # Arguments
/// * `lock_set` - The set of active locks
/// * `target_id` - The target to check
/// * `current_ns` - Current timestamp for expiry checking
///
/// # Returns
/// `true` if the market is locked, `false` otherwise.
pub fn is_market_locked(lock_set: &MarketLockSet, target_id: TargetId, current_ns: u64) -> bool {
    lock_set
        .locks
        .iter()
        .any(|lock| lock.target_id == target_id && !lock.is_expired(current_ns))
}

/// Check if a market is locked by a specific operation.
///
/// # Arguments
/// * `lock_set` - The set of active locks
/// * `target_id` - The target to check
/// * `op_id` - The operation ID to check for
///
/// # Returns
/// `true` if the market is locked by the specified operation.
pub fn is_locked_by_op(lock_set: &MarketLockSet, target_id: TargetId, op_id: u64) -> bool {
    lock_set
        .locks
        .iter()
        .any(|lock| lock.target_id == target_id && lock.op_id == Some(op_id))
}

/// Acquire a lock on a market.
///
/// # Arguments
/// * `lock_set` - The current set of locks
/// * `lock` - The lock to acquire
/// * `current_ns` - Current timestamp for expiry checking
///
/// # Returns
/// Updated lock set if successful, or the existing lock if the market is already locked.
pub fn acquire_lock(
    lock_set: &MarketLockSet,
    lock: MarketLock,
    current_ns: u64,
) -> Result<MarketLockSet, MarketLock> {
    // Check if already locked (and not expired)
    if let Some(existing) = lock_set
        .locks
        .iter()
        .find(|l| l.target_id == lock.target_id && !l.is_expired(current_ns))
    {
        return Err(existing.clone());
    }

    let mut new_set = lock_set.clone();

    // Remove any expired locks for this target
    new_set
        .locks
        .retain(|l| l.target_id != lock.target_id || !l.is_expired(current_ns));

    // Add the new lock
    new_set.locks.push(lock);

    Ok(new_set)
}

/// Release a lock on a market.
///
/// # Arguments
/// * `lock_set` - The current set of locks
/// * `target_id` - The target to unlock
///
/// # Returns
/// Updated lock set with the lock removed.
pub fn release_lock(lock_set: &MarketLockSet, target_id: TargetId) -> MarketLockSet {
    let mut new_set = lock_set.clone();
    new_set.locks.retain(|l| l.target_id != target_id);
    new_set
}

/// Release a lock held by a specific operation.
///
/// # Arguments
/// * `lock_set` - The current set of locks
/// * `target_id` - The target to unlock
/// * `op_id` - The operation ID that should hold the lock
///
/// # Returns
/// Updated lock set. Only releases if the lock is held by the specified operation.
pub fn release_lock_by_op(
    lock_set: &MarketLockSet,
    target_id: TargetId,
    op_id: u64,
) -> MarketLockSet {
    let mut new_set = lock_set.clone();
    new_set
        .locks
        .retain(|l| l.target_id != target_id || l.op_id != Some(op_id));
    new_set
}

/// Release all locks held by a specific operation.
///
/// # Arguments
/// * `lock_set` - The current set of locks
/// * `op_id` - The operation ID whose locks should be released
///
/// # Returns
/// Updated lock set with all locks from the operation removed.
pub fn release_all_by_op(lock_set: &MarketLockSet, op_id: u64) -> MarketLockSet {
    let mut new_set = lock_set.clone();
    new_set.locks.retain(|l| l.op_id != Some(op_id));
    new_set
}

/// Clear all locks (emergency reset).
///
/// # Arguments
/// * `lock_set` - The current set of locks
///
/// # Returns
/// Empty lock set.
pub fn clear_all_locks(_lock_set: &MarketLockSet) -> MarketLockSet {
    MarketLockSet::new()
}

/// Clean up expired locks.
///
/// # Arguments
/// * `lock_set` - The current set of locks
/// * `current_ns` - Current timestamp
///
/// # Returns
/// Lock set with expired locks removed.
pub fn cleanup_expired_locks(lock_set: &MarketLockSet, current_ns: u64) -> MarketLockSet {
    let mut new_set = lock_set.clone();
    new_set.locks.retain(|l| !l.is_expired(current_ns));
    new_set
}

/// Get all currently locked target IDs.
///
/// # Arguments
/// * `lock_set` - The set of locks
/// * `current_ns` - Current timestamp for expiry checking
///
/// # Returns
/// List of target IDs that are currently locked.
pub fn get_locked_targets(lock_set: &MarketLockSet, current_ns: u64) -> Vec<TargetId> {
    lock_set
        .locks
        .iter()
        .filter(|l| !l.is_expired(current_ns))
        .map(|l| l.target_id)
        .collect()
}

/// Check if any of the targets in a list are locked.
///
/// # Arguments
/// * `lock_set` - The set of locks
/// * `targets` - List of targets to check
/// * `current_ns` - Current timestamp for expiry checking
///
/// # Returns
/// List of targets that are locked.
pub fn find_locked_targets(
    lock_set: &MarketLockSet,
    targets: &[TargetId],
    current_ns: u64,
) -> Vec<TargetId> {
    targets
        .iter()
        .filter(|t| is_market_locked(lock_set, **t, current_ns))
        .copied()
        .collect()
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
    }

    #[test]
    fn test_acquire_lock() {
        let set = MarketLockSet::new();
        let lock = MarketLock::new(1, 1000);

        let result = acquire_lock(&set, lock, 1000).unwrap();

        assert_eq!(result.len(), 1);
        assert!(is_market_locked(&result, 1, 1000));
    }

    #[test]
    fn test_acquire_lock_already_locked() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::new(1, 2000);

        let set = acquire_lock(&set, lock1, 1000).unwrap();
        let result = acquire_lock(&set, lock2, 2000);

        assert!(result.is_err());
    }

    #[test]
    fn test_acquire_lock_different_target() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::new(2, 2000);

        let set = acquire_lock(&set, lock1, 1000).unwrap();
        let set = acquire_lock(&set, lock2, 2000).unwrap();

        assert_eq!(set.len(), 2);
        assert!(is_market_locked(&set, 1, 2000));
        assert!(is_market_locked(&set, 2, 2000));
    }

    #[test]
    fn test_acquire_lock_after_expiry() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::with_expiry(1, 1000, 2000); // expires at 2000
        let lock2 = MarketLock::new(1, 3000);

        let set = acquire_lock(&set, lock1, 1000).unwrap();

        // Should fail before expiry
        let result = acquire_lock(&set, lock2.clone(), 1500);
        assert!(result.is_err());

        // Should succeed after expiry
        let set = acquire_lock(&set, lock2, 3000).unwrap();
        assert_eq!(set.len(), 1); // Old expired lock removed
        assert!(is_market_locked(&set, 1, 3000));
    }

    #[test]
    fn test_release_lock() {
        let set = MarketLockSet::new();
        let lock = MarketLock::new(1, 1000);

        let set = acquire_lock(&set, lock, 1000).unwrap();
        let set = release_lock(&set, 1);

        assert!(set.is_empty());
        assert!(!is_market_locked(&set, 1, 2000));
    }

    #[test]
    fn test_release_lock_by_op() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::with_op_id(1, 100, 1000);
        let lock2 = MarketLock::with_op_id(2, 200, 1000);

        let set = acquire_lock(&set, lock1, 1000).unwrap();
        let set = acquire_lock(&set, lock2, 1000).unwrap();

        // Release only the lock held by op 100
        let set = release_lock_by_op(&set, 1, 100);

        assert_eq!(set.len(), 1);
        assert!(!is_market_locked(&set, 1, 2000));
        assert!(is_market_locked(&set, 2, 2000));
    }

    #[test]
    fn test_release_all_by_op() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::with_op_id(1, 100, 1000);
        let lock2 = MarketLock::with_op_id(2, 100, 1000);
        let lock3 = MarketLock::with_op_id(3, 200, 1000);

        let set = acquire_lock(&set, lock1, 1000).unwrap();
        let set = acquire_lock(&set, lock2, 1000).unwrap();
        let set = acquire_lock(&set, lock3, 1000).unwrap();

        let set = release_all_by_op(&set, 100);

        assert_eq!(set.len(), 1);
        assert!(!is_market_locked(&set, 1, 2000));
        assert!(!is_market_locked(&set, 2, 2000));
        assert!(is_market_locked(&set, 3, 2000));
    }

    #[test]
    fn test_is_locked_by_op() {
        let set = MarketLockSet::new();
        let lock = MarketLock::with_op_id(1, 100, 1000);

        let set = acquire_lock(&set, lock, 1000).unwrap();

        assert!(is_locked_by_op(&set, 1, 100));
        assert!(!is_locked_by_op(&set, 1, 200));
        assert!(!is_locked_by_op(&set, 2, 100));
    }

    #[test]
    fn test_lock_expiry() {
        let lock = MarketLock::with_expiry(1, 1000, 2000);

        assert!(!lock.is_expired(1000));
        assert!(!lock.is_expired(1999));
        assert!(lock.is_expired(2000));
        assert!(lock.is_expired(3000));
    }

    #[test]
    fn test_lock_no_expiry() {
        let lock = MarketLock::new(1, 1000);

        // expires_at_ns = 0 means no expiry
        assert!(!lock.is_expired(u64::MAX));
    }

    #[test]
    fn test_cleanup_expired_locks() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::with_expiry(1, 1000, 2000);
        let lock2 = MarketLock::with_expiry(2, 1000, 3000);
        let lock3 = MarketLock::new(3, 1000); // no expiry

        let set = acquire_lock(&set, lock1, 1000).unwrap();
        let set = acquire_lock(&set, lock2, 1000).unwrap();
        let set = acquire_lock(&set, lock3, 1000).unwrap();

        let cleaned = cleanup_expired_locks(&set, 2500);

        assert_eq!(cleaned.len(), 2);
        assert!(!is_market_locked(&cleaned, 1, 2500)); // expired
        assert!(is_market_locked(&cleaned, 2, 2500)); // not yet expired
        assert!(is_market_locked(&cleaned, 3, 2500)); // no expiry
    }

    #[test]
    fn test_get_locked_targets() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::with_expiry(2, 1000, 1500);
        let lock3 = MarketLock::new(3, 1000);

        let set = acquire_lock(&set, lock1, 1000).unwrap();
        let set = acquire_lock(&set, lock2, 1000).unwrap();
        let set = acquire_lock(&set, lock3, 1000).unwrap();

        let locked = get_locked_targets(&set, 2000);

        assert_eq!(locked.len(), 2);
        assert!(locked.contains(&1));
        assert!(!locked.contains(&2)); // expired
        assert!(locked.contains(&3));
    }

    #[test]
    fn test_find_locked_targets() {
        let set = MarketLockSet::new();
        let lock = MarketLock::new(2, 1000);

        let set = acquire_lock(&set, lock, 1000).unwrap();

        let to_check = vec![1, 2, 3, 4];
        let locked = find_locked_targets(&set, &to_check, 2000);

        assert_eq!(locked, vec![2]);
    }

    #[test]
    fn test_clear_all_locks() {
        let set = MarketLockSet::new();
        let lock1 = MarketLock::new(1, 1000);
        let lock2 = MarketLock::new(2, 1000);

        let set = acquire_lock(&set, lock1, 1000).unwrap();
        let set = acquire_lock(&set, lock2, 1000).unwrap();

        let cleared = clear_all_locks(&set);

        assert!(cleared.is_empty());
    }
}
