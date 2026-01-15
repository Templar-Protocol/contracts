//! Key pool with least-loaded selection strategy.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use near_crypto::InMemorySigner;

use super::slot::KeySlot;

/// Error returned when the key pool cannot satisfy a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolError {
    /// All keys in the pool are marked unhealthy.
    AllKeysUnhealthy,

    /// The pool is empty (no keys configured).
    EmptyPool,
}

impl std::fmt::Display for PoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PoolError::AllKeysUnhealthy => write!(f, "All keys in pool are unhealthy"),
            PoolError::EmptyPool => write!(f, "Key pool is empty"),
        }
    }
}

impl std::error::Error for PoolError {}

/// A pool of NEAR access keys with automatic selection.
///
/// The pool uses a least-loaded selection strategy with round-robin as a tiebreaker:
/// 1. Filter to healthy keys only
/// 2. Find keys with minimum in-flight transaction count
/// 3. Among those, select round-robin
///
/// This ensures even load distribution while preferring keys that are currently idle.
pub struct KeyPool {
    /// All key slots in the pool.
    slots: Vec<Arc<KeySlot>>,

    /// Round-robin index (used as tiebreaker for least-loaded).
    next_index: AtomicUsize,
}

impl KeyPool {
    /// Create a new key pool from a list of signers.
    ///
    /// # Errors
    ///
    /// Returns `PoolError::EmptyPool` if the signers list is empty.
    pub fn new(signers: Vec<InMemorySigner>) -> Result<Self, PoolError> {
        if signers.is_empty() {
            return Err(PoolError::EmptyPool);
        }

        let slots = signers
            .into_iter()
            .map(|signer| Arc::new(KeySlot::new(signer)))
            .collect();

        Ok(Self {
            slots,
            next_index: AtomicUsize::new(0),
        })
    }

    /// Create a new key pool from pre-configured key slots.
    ///
    /// Use this when you need custom configuration per slot (e.g., block hash TTL).
    ///
    /// # Errors
    ///
    /// Returns `PoolError::EmptyPool` if the slots list is empty.
    pub fn from_slots(slots: Vec<Arc<KeySlot>>) -> Result<Self, PoolError> {
        if slots.is_empty() {
            return Err(PoolError::EmptyPool);
        }

        Ok(Self {
            slots,
            next_index: AtomicUsize::new(0),
        })
    }

    /// Select the best available key for a transaction.
    ///
    /// Selection strategy:
    /// 1. Filter to healthy keys only
    /// 2. Find keys with minimum in-flight count
    /// 3. Among those, select round-robin
    ///
    /// # Errors
    ///
    /// Returns `PoolError::AllKeysUnhealthy` if no healthy keys are available.
    pub fn select(&self) -> Result<Arc<KeySlot>, PoolError> {
        let healthy: Vec<_> = self.slots.iter().filter(|s| s.is_healthy()).collect();

        if healthy.is_empty() {
            return Err(PoolError::AllKeysUnhealthy);
        }

        // Find minimum in-flight count
        let min_in_flight = healthy.iter().map(|s| s.in_flight_count()).min().unwrap(); // Safe: healthy is non-empty

        // Among keys with min in-flight, collect candidates
        let candidates: Vec<_> = healthy
            .into_iter()
            .filter(|s| s.in_flight_count() == min_in_flight)
            .collect();

        // Round-robin among candidates
        let idx = self.next_index.fetch_add(1, Ordering::Relaxed) % candidates.len();
        Ok(candidates[idx].clone())
    }

    /// Get the total number of keys in the pool.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Check if the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Get the number of healthy keys in the pool.
    pub fn healthy_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_healthy()).count()
    }

    /// Get all key slots (for health reporting).
    pub fn slots(&self) -> &[Arc<KeySlot>] {
        &self.slots
    }

    /// Get total in-flight count across all keys.
    pub fn total_in_flight(&self) -> u32 {
        self.slots.iter().map(|s| s.in_flight_count()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_crypto::{KeyType, SecretKey};

    fn test_signer(suffix: &str) -> InMemorySigner {
        let account_id: near_account_id::AccountId =
            format!("test{}.near", suffix).parse().unwrap();
        let secret_key = SecretKey::from_random(KeyType::ED25519);
        InMemorySigner {
            account_id,
            public_key: secret_key.public_key(),
            secret_key,
        }
    }

    #[test]
    fn empty_pool_returns_error() {
        let result = KeyPool::new(vec![]);
        assert!(matches!(result, Err(PoolError::EmptyPool)));
    }

    #[test]
    fn single_key_pool_works() {
        let pool = KeyPool::new(vec![test_signer("1")]).unwrap();
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.healthy_count(), 1);

        let slot = pool.select().unwrap();
        assert!(slot.is_healthy());
    }

    #[test]
    fn select_round_robins_when_all_idle() {
        let pool =
            KeyPool::new(vec![test_signer("1"), test_signer("2"), test_signer("3")]).unwrap();

        // All keys have 0 in-flight, so should round-robin
        let slot1 = pool.select().unwrap();
        let slot2 = pool.select().unwrap();
        let slot3 = pool.select().unwrap();
        let slot4 = pool.select().unwrap();

        // slot4 should wrap around to slot1's key
        assert_eq!(
            slot1.public_key().to_string(),
            slot4.public_key().to_string()
        );

        // All three should be different
        assert_ne!(
            slot1.public_key().to_string(),
            slot2.public_key().to_string()
        );
        assert_ne!(
            slot2.public_key().to_string(),
            slot3.public_key().to_string()
        );
    }

    #[test]
    fn unhealthy_keys_are_skipped() {
        let pool = KeyPool::new(vec![test_signer("1"), test_signer("2")]).unwrap();

        // Mark first key unhealthy
        pool.slots()[0].mark_unhealthy();

        assert_eq!(pool.healthy_count(), 1);

        // Should always select the healthy key
        let slot = pool.select().unwrap();
        assert_eq!(
            slot.public_key().to_string(),
            pool.slots()[1].public_key().to_string()
        );
    }

    #[test]
    fn all_unhealthy_returns_error() {
        let pool = KeyPool::new(vec![test_signer("1"), test_signer("2")]).unwrap();

        pool.slots()[0].mark_unhealthy();
        pool.slots()[1].mark_unhealthy();

        let result = pool.select();
        assert!(matches!(result, Err(PoolError::AllKeysUnhealthy)));
    }

    #[tokio::test]
    async fn select_prefers_least_loaded() {
        let pool = KeyPool::new(vec![test_signer("1"), test_signer("2")]).unwrap();

        // Acquire key 0 (simulates in-flight transaction)
        let _guard = pool.slots()[0].acquire().await;
        assert_eq!(pool.slots()[0].in_flight_count(), 1);
        assert_eq!(pool.slots()[1].in_flight_count(), 0);

        // Select should prefer key 1 (least loaded)
        let selected = pool.select().unwrap();
        assert_eq!(
            selected.public_key().to_string(),
            pool.slots()[1].public_key().to_string()
        );
    }
}
