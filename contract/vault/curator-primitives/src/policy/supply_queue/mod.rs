//! Supply queue for managing pending allocation requests.

use alloc::vec::Vec;
use templar_vault_kernel::TargetId;
use typed_builder::TypedBuilder;

/// An entry in the supply queue representing a pending allocation.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq, TypedBuilder)]
#[builder(field_defaults(setter(into)))]
pub struct SupplyQueueEntry {
    pub target_id: TargetId,
    pub amount: u128,
    #[builder(default)]
    pub priority: u8,
    #[builder(default)]
    pub queued_at_ns: u64,
}

impl SupplyQueueEntry {
    #[must_use]
    pub fn new(target_id: TargetId, amount: u128) -> Self {
        Self {
            target_id,
            amount,
            priority: 0,
            queued_at_ns: 0,
        }
    }
}

impl From<(TargetId, u128)> for SupplyQueueEntry {
    fn from(value: (TargetId, u128)) -> Self {
        Self::new(value.0, value.1)
    }
}

/// A queue of pending supply requests.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, Default)]
pub struct SupplyQueue {
    pub entries: Vec<SupplyQueueEntry>,
    pub max_length: usize,
}

impl SupplyQueue {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

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

    pub fn dequeue(&self) -> Result<(Self, SupplyQueueEntry), SupplyQueueError> {
        if self.is_empty() {
            return Err(SupplyQueueError::QueueEmpty);
        }

        let mut new_queue = self.clone();
        let entry = new_queue.entries.remove(0);

        Ok((new_queue, entry))
    }

    #[must_use]
    pub fn peek(&self) -> Option<&SupplyQueueEntry> {
        self.entries.first()
    }

    #[must_use]
    pub fn total(&self) -> u128 {
        self.entries
            .iter()
            .fold(0u128, |acc, e| acc.saturating_add(e.amount))
    }

    /// Returns totals grouped by target ID.
    #[must_use]
    pub fn totals_by_target(&self) -> Vec<(TargetId, u128)> {
        let mut totals: Vec<(TargetId, u128)> = Vec::new();
        for entry in &self.entries {
            if let Some((_, sum)) = totals
                .iter_mut()
                .find(|(target_id, _)| *target_id == entry.target_id)
            {
                *sum = sum.saturating_add(entry.amount);
            } else {
                totals.push((entry.target_id, entry.amount));
            }
        }
        totals.sort_unstable_by_key(|(target_id, _)| *target_id);
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
        let entries: Vec<SupplyQueueEntry> = self.entries.to_vec();
        let empty_queue = Self {
            entries: Vec::new(),
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
            entries,
            max_length: 0,
        }
    }
}

/// Errors that can occur during supply queue operations.
#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum SupplyQueueError {
    /// Queue is at maximum capacity.
    QueueFull { max_length: usize },
    /// Amount must be greater than zero.
    ZeroAmount,
    /// Queue is empty.
    QueueEmpty,
}
