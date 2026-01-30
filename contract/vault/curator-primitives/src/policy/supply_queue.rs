//! Supply queue for managing pending allocation requests.
//!
//! The supply queue holds pending supply requests that will be processed
//! during the next allocation cycle. This allows batching of deposits
//! and efficient allocation planning.
//!
//! # Example
//!
//! ```ignore
//! use templar_curator_primitives::policy::supply_queue::*;
//!
//! // Using the fluent API
//! let entry = SupplyQueueEntry::new(1, 100)
//!     .with_priority(10)
//!     .with_timestamp(1000);
//!
//! // Or using TypedBuilder
//! let entry = SupplyQueueEntry::builder()
//!     .target_id(1)
//!     .amount(100)
//!     .priority(10)
//!     .queued_at_ns(1000)
//!     .build();
//!
//! let queue = SupplyQueue::new();
//! let queue = queue.enqueue(entry).unwrap();
//! assert_eq!(queue.total(), 100);
//! ```

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use templar_vault_kernel::TargetId;
use typed_builder::TypedBuilder;

/// An entry in the supply queue representing a pending allocation.
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq, TypedBuilder)]
#[builder(field_defaults(setter(into)))]
pub struct SupplyQueueEntry {
    /// Target market/strategy ID to allocate to.
    pub target_id: TargetId,
    /// Amount to allocate in underlying asset units.
    pub amount: u128,
    /// Priority (higher = process first). Default is 0.
    #[builder(default)]
    pub priority: u8,
    /// Timestamp when this entry was queued (nanoseconds).
    #[builder(default)]
    pub queued_at_ns: u64,
}

impl SupplyQueueEntry {
    /// Create a new supply queue entry with default priority and timestamp.
    #[must_use]
    pub fn new(target_id: TargetId, amount: u128) -> Self {
        Self {
            target_id,
            amount,
            priority: 0,
            queued_at_ns: 0,
        }
    }

    /// Fluent method: set priority.
    #[must_use]
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Fluent method: set timestamp.
    #[must_use]
    pub fn with_timestamp(mut self, queued_at_ns: u64) -> Self {
        self.queued_at_ns = queued_at_ns;
        self
    }
}

impl From<(TargetId, u128)> for SupplyQueueEntry {
    fn from(value: (TargetId, u128)) -> Self {
        Self::new(value.0, value.1)
    }
}

/// A queue of pending supply requests.
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct SupplyQueue {
    /// The queue of pending supply requests.
    pub entries: VecDeque<SupplyQueueEntry>,
    /// Maximum queue length (0 = unlimited).
    pub max_length: usize,
}

impl SupplyQueue {
    /// Create a new empty supply queue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            max_length: 0,
        }
    }

    /// Create a new supply queue with a maximum length.
    #[must_use]
    pub fn with_max_length(max_length: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_length,
        }
    }

    /// Returns true if the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the number of entries in the queue.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the queue is at maximum capacity.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.max_length > 0 && self.entries.len() >= self.max_length
    }

    /// Add an entry to the supply queue.
    ///
    /// Entries are inserted in priority order (higher priority first).
    /// Within the same priority, FIFO order is maintained.
    pub fn enqueue(&self, entry: SupplyQueueEntry) -> Result<Self, SupplyQueueError> {
        if entry.amount == 0 {
            return Err(SupplyQueueError::ZeroAmount);
        }

        if self.is_full() {
            return Err(SupplyQueueError::QueueFull {
                max_length: self.max_length,
            });
        }

        let mut new_queue = self.clone();

        // Insert maintaining priority order (higher priority first)
        let insert_pos = new_queue
            .entries
            .iter()
            .position(|e| e.priority < entry.priority)
            .unwrap_or(new_queue.entries.len());

        new_queue.entries.insert(insert_pos, entry);

        Ok(new_queue)
    }

    /// Remove and return the next entry from the supply queue.
    pub fn dequeue(&self) -> Result<(Self, SupplyQueueEntry), SupplyQueueError> {
        if self.is_empty() {
            return Err(SupplyQueueError::QueueEmpty);
        }

        let mut new_queue = self.clone();
        let entry = new_queue
            .entries
            .pop_front()
            .ok_or(SupplyQueueError::QueueEmpty)?;

        Ok((new_queue, entry))
    }

    /// Peek at the next entry without removing it.
    #[must_use]
    pub fn peek(&self) -> Option<&SupplyQueueEntry> {
        self.entries.front()
    }

    /// Compute the total amount in the supply queue.
    #[must_use]
    pub fn total(&self) -> u128 {
        self.entries
            .iter()
            .fold(0u128, |acc, e| acc.saturating_add(e.amount))
    }

    /// Compute totals per target in the supply queue.
    #[must_use]
    pub fn totals_by_target(&self) -> Vec<(TargetId, u128)> {
        let mut totals: Vec<(TargetId, u128)> = Vec::new();

        for entry in &self.entries {
            if let Some((_, amount)) = totals.iter_mut().find(|(id, _)| *id == entry.target_id) {
                *amount = amount.saturating_add(entry.amount);
            } else {
                totals.push((entry.target_id, entry.amount));
            }
        }

        totals
    }

    /// Remove all entries for a specific target from the queue.
    #[must_use]
    pub fn remove_target(&self, target_id: TargetId) -> Self {
        let mut new_queue = self.clone();
        new_queue.entries.retain(|e| e.target_id != target_id);
        new_queue
    }

    /// Drain the queue into a list of entries.
    #[must_use]
    pub fn drain(&self) -> (Self, Vec<SupplyQueueEntry>) {
        let entries: Vec<SupplyQueueEntry> = self.entries.iter().cloned().collect();
        let empty_queue = Self {
            entries: VecDeque::new(),
            max_length: self.max_length,
        };
        (empty_queue, entries)
    }

    /// Convert the queue to an allocation plan.
    ///
    /// Aggregates entries by target and returns a plan suitable for the
    /// allocation state machine.
    #[must_use]
    pub fn to_allocation_plan(&self) -> Vec<(TargetId, u128)> {
        self.totals_by_target()
    }

    /// Get total amount for a specific target.
    #[must_use]
    pub fn total_for_target(&self, target_id: TargetId) -> u128 {
        self.entries
            .iter()
            .filter(|e| e.target_id == target_id)
            .fold(0u128, |acc, e| acc.saturating_add(e.amount))
    }

    /// Check if a target has any pending entries.
    #[must_use]
    pub fn has_target(&self, target_id: TargetId) -> bool {
        self.entries.iter().any(|e| e.target_id == target_id)
    }
}


impl From<Vec<SupplyQueueEntry>> for SupplyQueue {
    fn from(entries: Vec<SupplyQueueEntry>) -> Self {
        Self {
            entries: VecDeque::from(entries),
            max_length: 0,
        }
    }
}

/// Errors that can occur during supply queue operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupplyQueueError {
    /// Queue is at maximum capacity.
    QueueFull { max_length: usize },
    /// Amount must be greater than zero.
    ZeroAmount,
    /// Queue is empty.
    QueueEmpty,
    /// Target not found in queue.
    TargetNotFound { target_id: TargetId },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_queue_is_empty() {
        let queue = SupplyQueue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert!(!queue.is_full());
    }

    #[test]
    fn test_enqueue_supply() {
        let queue = SupplyQueue::new();
        let entry = SupplyQueueEntry::new(1, 100);

        let result = queue.enqueue(entry.clone()).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.entries[0], entry);
    }

    #[test]
    fn test_enqueue_zero_amount_error() {
        let queue = SupplyQueue::new();
        let entry = SupplyQueueEntry::new(1, 0);

        let result = queue.enqueue(entry);

        assert!(matches!(result, Err(SupplyQueueError::ZeroAmount)));
    }

    #[test]
    fn test_enqueue_full_queue_error() {
        let queue = SupplyQueue::with_max_length(2);
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(3, 300);

        let queue = queue.enqueue(entry1).unwrap();
        let queue = queue.enqueue(entry2).unwrap();
        let result = queue.enqueue(entry3);

        assert!(matches!(
            result,
            Err(SupplyQueueError::QueueFull { max_length: 2 })
        ));
    }

    #[test]
    fn test_enqueue_with_priority() {
        let queue = SupplyQueue::new();
        let low = SupplyQueueEntry::new(1, 100).with_priority(0);
        let high = SupplyQueueEntry::new(2, 200).with_priority(10);
        let medium = SupplyQueueEntry::new(3, 300).with_priority(5);

        let queue = queue.enqueue(low).unwrap();
        let queue = queue.enqueue(high).unwrap();
        let queue = queue.enqueue(medium).unwrap();

        // High priority should be first
        assert_eq!(queue.entries[0].target_id, 2);
        assert_eq!(queue.entries[1].target_id, 3);
        assert_eq!(queue.entries[2].target_id, 1);
    }

    #[test]
    fn test_dequeue_supply() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);

        let queue = queue.enqueue(entry1.clone()).unwrap();
        let queue = queue.enqueue(entry2).unwrap();

        let (queue, dequeued) = queue.dequeue().unwrap();

        assert_eq!(dequeued, entry1);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_dequeue_empty_error() {
        let queue = SupplyQueue::new();
        let result = queue.dequeue();

        assert!(matches!(result, Err(SupplyQueueError::QueueEmpty)));
    }

    #[test]
    fn test_peek() {
        let queue = SupplyQueue::new();
        assert!(queue.peek().is_none());

        let entry = SupplyQueueEntry::new(1, 100);
        let queue = queue.enqueue(entry.clone()).unwrap();

        assert_eq!(queue.peek(), Some(&entry));
        assert_eq!(queue.len(), 1); // Still in queue
    }

    #[test]
    fn test_compute_queue_total() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(1, 50);

        let queue = queue.enqueue(entry1).unwrap();
        let queue = queue.enqueue(entry2).unwrap();
        let queue = queue.enqueue(entry3).unwrap();

        assert_eq!(queue.total(), 350);
    }

    #[test]
    fn test_compute_queue_totals_by_target() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(1, 50);

        let queue = queue.enqueue(entry1).unwrap();
        let queue = queue.enqueue(entry2).unwrap();
        let queue = queue.enqueue(entry3).unwrap();

        let totals = queue.totals_by_target();

        assert_eq!(totals.len(), 2);
        assert!(totals.contains(&(1, 150)));
        assert!(totals.contains(&(2, 200)));
    }

    #[test]
    fn test_remove_target_entries() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(1, 50);

        let queue = queue.enqueue(entry1).unwrap();
        let queue = queue.enqueue(entry2).unwrap();
        let queue = queue.enqueue(entry3).unwrap();

        let filtered = queue.remove_target(1);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered.entries[0].target_id, 2);
    }

    #[test]
    fn test_drain_queue() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);

        let queue = queue.enqueue(entry1).unwrap();
        let queue = queue.enqueue(entry2).unwrap();

        let (empty, entries) = queue.drain();

        assert!(empty.is_empty());
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_to_allocation_plan() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(1, 50);

        let queue = queue.enqueue(entry1).unwrap();
        let queue = queue.enqueue(entry2).unwrap();
        let queue = queue.enqueue(entry3).unwrap();

        let plan = queue.to_allocation_plan();

        // Should be aggregated by target
        assert_eq!(plan.len(), 2);
        assert!(plan.contains(&(1, 150)));
        assert!(plan.contains(&(2, 200)));
    }

    #[test]
    fn test_total_for_target() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(1, 50);

        let queue = queue.enqueue(entry1).unwrap();
        let queue = queue.enqueue(entry2).unwrap();
        let queue = queue.enqueue(entry3).unwrap();

        assert_eq!(queue.total_for_target(1), 150);
        assert_eq!(queue.total_for_target(2), 200);
        assert_eq!(queue.total_for_target(3), 0);
    }

    #[test]
    fn test_has_target() {
        let queue = SupplyQueue::new();
        let entry = SupplyQueueEntry::new(1, 100);
        let queue = queue.enqueue(entry).unwrap();

        assert!(queue.has_target(1));
        assert!(!queue.has_target(2));
    }

    #[test]
    fn test_builder_pattern() {
        let entry = SupplyQueueEntry::new(1, 100)
            .with_priority(5)
            .with_timestamp(1000);

        assert_eq!(entry.target_id, 1);
        assert_eq!(entry.amount, 100);
        assert_eq!(entry.priority, 5);
        assert_eq!(entry.queued_at_ns, 1000);
    }
}
