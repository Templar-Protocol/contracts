//! Supply queue for managing pending allocation requests.
//!
//! The supply queue holds pending supply requests that will be processed
//! during the next allocation cycle. This allows batching of deposits
//! and efficient allocation planning.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use templar_vault_kernel::TargetId;

/// An entry in the supply queue representing a pending allocation.
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupplyQueueEntry {
    /// Target market/strategy ID to allocate to.
    pub target_id: TargetId,
    /// Amount to allocate in underlying asset units.
    pub amount: u128,
    /// Priority (higher = process first). Default is 0.
    pub priority: u8,
    /// Timestamp when this entry was queued (nanoseconds).
    pub queued_at_ns: u64,
}

impl SupplyQueueEntry {
    /// Create a new supply queue entry.
    pub fn new(target_id: TargetId, amount: u128) -> Self {
        Self {
            target_id,
            amount,
            priority: 0,
            queued_at_ns: 0,
        }
    }

    /// Create a new entry with priority.
    pub fn with_priority(target_id: TargetId, amount: u128, priority: u8) -> Self {
        Self {
            target_id,
            amount,
            priority,
            queued_at_ns: 0,
        }
    }

    /// Create a new entry with timestamp.
    pub fn with_timestamp(target_id: TargetId, amount: u128, queued_at_ns: u64) -> Self {
        Self {
            target_id,
            amount,
            priority: 0,
            queued_at_ns,
        }
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
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            max_length: 0,
        }
    }

    /// Create a new supply queue with a maximum length.
    pub fn with_max_length(max_length: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_length,
        }
    }

    /// Returns true if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the number of entries in the queue.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the queue is at maximum capacity.
    pub fn is_full(&self) -> bool {
        self.max_length > 0 && self.entries.len() >= self.max_length
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

/// Add an entry to the supply queue.
///
/// Entries are added to the back of the queue (FIFO order).
/// If the queue has a maximum length and is full, returns an error.
///
/// # Arguments
/// * `queue` - The supply queue
/// * `entry` - The entry to add
///
/// # Returns
/// Updated queue with the new entry, or an error.
pub fn enqueue_supply(
    queue: &SupplyQueue,
    entry: SupplyQueueEntry,
) -> Result<SupplyQueue, SupplyQueueError> {
    if entry.amount == 0 {
        return Err(SupplyQueueError::ZeroAmount);
    }

    if queue.is_full() {
        return Err(SupplyQueueError::QueueFull {
            max_length: queue.max_length,
        });
    }

    let mut new_queue = queue.clone();

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
///
/// # Arguments
/// * `queue` - The supply queue
///
/// # Returns
/// Tuple of (updated queue, dequeued entry), or an error if empty.
pub fn dequeue_supply(
    queue: &SupplyQueue,
) -> Result<(SupplyQueue, SupplyQueueEntry), SupplyQueueError> {
    if queue.is_empty() {
        return Err(SupplyQueueError::QueueEmpty);
    }

    let mut new_queue = queue.clone();
    let entry = new_queue
        .entries
        .pop_front()
        .ok_or(SupplyQueueError::QueueEmpty)?;

    Ok((new_queue, entry))
}

/// Compute the total amount in the supply queue.
///
/// # Arguments
/// * `queue` - The supply queue
///
/// # Returns
/// Total amount across all entries, using saturating addition.
pub fn compute_queue_total(queue: &SupplyQueue) -> u128 {
    queue
        .entries
        .iter()
        .fold(0u128, |acc, e| acc.saturating_add(e.amount))
}

/// Compute totals per target in the supply queue.
///
/// # Arguments
/// * `queue` - The supply queue
///
/// # Returns
/// Vec of (target_id, total_amount) pairs.
pub fn compute_queue_totals_by_target(queue: &SupplyQueue) -> Vec<(TargetId, u128)> {
    let mut totals: Vec<(TargetId, u128)> = Vec::new();

    for entry in &queue.entries {
        if let Some((_, amount)) = totals.iter_mut().find(|(id, _)| *id == entry.target_id) {
            *amount = amount.saturating_add(entry.amount);
        } else {
            totals.push((entry.target_id, entry.amount));
        }
    }

    totals
}

/// Remove all entries for a specific target from the queue.
///
/// # Arguments
/// * `queue` - The supply queue
/// * `target_id` - The target to remove
///
/// # Returns
/// Updated queue with entries for the target removed.
pub fn remove_target_entries(queue: &SupplyQueue, target_id: TargetId) -> SupplyQueue {
    let mut new_queue = queue.clone();
    new_queue.entries.retain(|e| e.target_id != target_id);
    new_queue
}

/// Drain the queue into a list of entries.
///
/// # Arguments
/// * `queue` - The supply queue
///
/// # Returns
/// Tuple of (empty queue, list of all entries).
pub fn drain_queue(queue: &SupplyQueue) -> (SupplyQueue, Vec<SupplyQueueEntry>) {
    let entries: Vec<SupplyQueueEntry> = queue.entries.iter().cloned().collect();
    let empty_queue = SupplyQueue {
        entries: VecDeque::new(),
        max_length: queue.max_length,
    };
    (empty_queue, entries)
}

/// Convert the queue to an allocation plan.
///
/// Aggregates entries by target and returns a plan suitable for the
/// allocation state machine.
///
/// # Arguments
/// * `queue` - The supply queue
///
/// # Returns
/// Vec of (target_id, amount) pairs for allocation.
pub fn to_allocation_plan(queue: &SupplyQueue) -> Vec<(TargetId, u128)> {
    compute_queue_totals_by_target(queue)
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

        let result = enqueue_supply(&queue, entry.clone()).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.entries[0], entry);
    }

    #[test]
    fn test_enqueue_zero_amount_error() {
        let queue = SupplyQueue::new();
        let entry = SupplyQueueEntry::new(1, 0);

        let result = enqueue_supply(&queue, entry);

        assert!(matches!(result, Err(SupplyQueueError::ZeroAmount)));
    }

    #[test]
    fn test_enqueue_full_queue_error() {
        let queue = SupplyQueue::with_max_length(2);
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(3, 300);

        let queue = enqueue_supply(&queue, entry1).unwrap();
        let queue = enqueue_supply(&queue, entry2).unwrap();
        let result = enqueue_supply(&queue, entry3);

        assert!(matches!(
            result,
            Err(SupplyQueueError::QueueFull { max_length: 2 })
        ));
    }

    #[test]
    fn test_enqueue_with_priority() {
        let queue = SupplyQueue::new();
        let low = SupplyQueueEntry::with_priority(1, 100, 0);
        let high = SupplyQueueEntry::with_priority(2, 200, 10);
        let medium = SupplyQueueEntry::with_priority(3, 300, 5);

        let queue = enqueue_supply(&queue, low).unwrap();
        let queue = enqueue_supply(&queue, high).unwrap();
        let queue = enqueue_supply(&queue, medium).unwrap();

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

        let queue = enqueue_supply(&queue, entry1.clone()).unwrap();
        let queue = enqueue_supply(&queue, entry2).unwrap();

        let (queue, dequeued) = dequeue_supply(&queue).unwrap();

        assert_eq!(dequeued, entry1);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_dequeue_empty_error() {
        let queue = SupplyQueue::new();
        let result = dequeue_supply(&queue);

        assert!(matches!(result, Err(SupplyQueueError::QueueEmpty)));
    }

    #[test]
    fn test_compute_queue_total() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(1, 50);

        let queue = enqueue_supply(&queue, entry1).unwrap();
        let queue = enqueue_supply(&queue, entry2).unwrap();
        let queue = enqueue_supply(&queue, entry3).unwrap();

        assert_eq!(compute_queue_total(&queue), 350);
    }

    #[test]
    fn test_compute_queue_totals_by_target() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(1, 50);

        let queue = enqueue_supply(&queue, entry1).unwrap();
        let queue = enqueue_supply(&queue, entry2).unwrap();
        let queue = enqueue_supply(&queue, entry3).unwrap();

        let totals = compute_queue_totals_by_target(&queue);

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

        let queue = enqueue_supply(&queue, entry1).unwrap();
        let queue = enqueue_supply(&queue, entry2).unwrap();
        let queue = enqueue_supply(&queue, entry3).unwrap();

        let filtered = remove_target_entries(&queue, 1);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered.entries[0].target_id, 2);
    }

    #[test]
    fn test_drain_queue() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);

        let queue = enqueue_supply(&queue, entry1).unwrap();
        let queue = enqueue_supply(&queue, entry2).unwrap();

        let (empty, entries) = drain_queue(&queue);

        assert!(empty.is_empty());
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_to_allocation_plan() {
        let queue = SupplyQueue::new();
        let entry1 = SupplyQueueEntry::new(1, 100);
        let entry2 = SupplyQueueEntry::new(2, 200);
        let entry3 = SupplyQueueEntry::new(1, 50);

        let queue = enqueue_supply(&queue, entry1).unwrap();
        let queue = enqueue_supply(&queue, entry2).unwrap();
        let queue = enqueue_supply(&queue, entry3).unwrap();

        let plan = to_allocation_plan(&queue);

        // Should be aggregated by target
        assert_eq!(plan.len(), 2);
        assert!(plan.contains(&(1, 150)));
        assert!(plan.contains(&(2, 200)));
    }
}
