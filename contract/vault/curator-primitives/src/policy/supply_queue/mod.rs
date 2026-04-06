//! Supply queue for managing pending allocation requests.

use alloc::vec::Vec;
use core::num::NonZeroU32;
use templar_vault_kernel::{TargetId, TimestampNs};

use super::market_lock::MarketLeaseRegistry;

#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct SupplyQueueEntry {
    pub target_id: TargetId,
    pub amount: u128,
    pub priority: u8,
}

impl SupplyQueueEntry {
    pub fn new(target_id: TargetId, amount: u128) -> Result<Self, SupplyQueueError> {
        Self::new_with_priority(target_id, amount, 0)
    }

    pub fn new_with_priority(
        target_id: TargetId,
        amount: u128,
        priority: u8,
    ) -> Result<Self, SupplyQueueError> {
        if amount == 0 {
            return Err(SupplyQueueError::ZeroAmount);
        }

        Ok(Self {
            target_id,
            amount,
            priority,
        })
    }

    fn validate(&self) -> Result<(), SupplyQueueError> {
        if self.amount == 0 {
            return Err(SupplyQueueError::ZeroAmount);
        }

        Ok(())
    }
}

impl TryFrom<(TargetId, u128)> for SupplyQueueEntry {
    type Error = SupplyQueueError;

    fn try_from(value: (TargetId, u128)) -> Result<Self, Self::Error> {
        Self::new(value.0, value.1)
    }
}

#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct SupplyQueue {
    buckets: Vec<Vec<SupplyQueueEntry>>,
    len: u32,
    max_length: Option<u32>,
}

impl Default for SupplyQueue {
    fn default() -> Self {
        Self::unbounded()
    }
}

impl SupplyQueue {
    #[must_use]
    pub fn new(max_length: Option<NonZeroU32>) -> Self {
        Self {
            buckets: alloc::vec![Vec::new(); usize::from(u8::MAX) + 1],
            len: 0,
            max_length: max_length.map(NonZeroU32::get),
        }
    }

    #[must_use]
    pub fn unbounded() -> Self {
        Self::new(None)
    }

    #[must_use]
    pub fn bounded(max_length: NonZeroU32) -> Self {
        Self::new(Some(max_length))
    }

    pub fn try_from_entries(
        entries: Vec<SupplyQueueEntry>,
        max_length: Option<NonZeroU32>,
    ) -> Result<Self, SupplyQueueError> {
        let mut queue = Self::new(max_length);
        for entry in entries {
            queue.enqueue(entry)?;
        }
        Ok(queue)
    }

    pub fn validate(&self) -> Result<(), SupplyQueueError> {
        let actual_len = self.buckets.iter().try_fold(0u32, |acc, bucket| {
            let bucket_len =
                u32::try_from(bucket.len()).map_err(|_| SupplyQueueError::LengthOverflow)?;
            acc.checked_add(bucket_len)
                .ok_or(SupplyQueueError::LengthOverflow)
        })?;

        if self.len != actual_len {
            return Err(SupplyQueueError::LengthMismatch {
                recorded_len: self.len,
                actual_len,
            });
        }

        if let Some(max_length) = self.max_length {
            if self.len > max_length {
                return Err(SupplyQueueError::QueueTooLong {
                    len: self.len,
                    max_length,
                });
            }
        }

        for (priority, bucket) in self.buckets.iter().enumerate() {
            let expected_priority =
                u8::try_from(priority).map_err(|_| SupplyQueueError::LengthOverflow)?;
            for entry in bucket {
                entry.validate()?;
                if entry.priority != expected_priority {
                    return Err(SupplyQueueError::PriorityBucketMismatch {
                        expected_priority,
                        actual_priority: entry.priority,
                    });
                }
            }
        }

        Ok(())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn len(&self) -> usize {
        usize::try_from(self.len).unwrap()
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        self.max_length
            .is_some_and(|max_length| self.len >= max_length)
    }

    #[must_use]
    pub fn entries(&self) -> Vec<&SupplyQueueEntry> {
        self.buckets
            .iter()
            .rev()
            .flat_map(|bucket| bucket.iter())
            .collect()
    }

    #[must_use]
    pub fn max_length(&self) -> Option<NonZeroU32> {
        self.max_length.and_then(NonZeroU32::new)
    }

    pub fn enqueue(&mut self, entry: SupplyQueueEntry) -> Result<(), SupplyQueueError> {
        entry.validate()?;

        if self.is_full() {
            return Err(SupplyQueueError::QueueFull {
                max_length: self.max_length.unwrap(),
            });
        }

        self.push_validated_entry(entry)
            .ok_or(SupplyQueueError::LengthOverflow)?;
        Ok(())
    }

    fn push_validated_entry(&mut self, entry: SupplyQueueEntry) -> Option<()> {
        self.buckets[usize::from(entry.priority)].push(entry);
        self.len = self.len.checked_add(1)?;
        Some(())
    }

    pub fn dequeue(&mut self) -> Result<SupplyQueueEntry, SupplyQueueError> {
        for bucket in self.buckets.iter_mut().rev() {
            if !bucket.is_empty() {
                let entry = bucket.remove(0);
                self.len -= 1;
                return Ok(entry);
            }
        }

        Err(SupplyQueueError::QueueEmpty)
    }

    #[must_use]
    pub fn peek(&self) -> Option<&SupplyQueueEntry> {
        self.buckets.iter().rev().find_map(|bucket| bucket.first())
    }

    pub fn total(&self) -> Result<u128, SupplyQueueError> {
        checked_total_amount(self.entries().into_iter().map(|entry| entry.amount))
    }

    pub fn totals_by_target(&self) -> Result<Vec<(TargetId, u128)>, SupplyQueueError> {
        let mut totals: Vec<(TargetId, u128)> = Vec::new();
        for entry in self.entries() {
            let sum = match totals
                .iter_mut()
                .find(|(target_id, _)| *target_id == entry.target_id)
            {
                Some((_, total)) => total,
                None => {
                    totals.push((entry.target_id, 0));
                    &mut totals.last_mut().unwrap().1
                }
            };
            *sum = (*sum)
                .checked_add(entry.amount)
                .ok_or(SupplyQueueError::AmountOverflow)?;
        }
        Ok(totals)
    }

    pub fn remove_target(&mut self, target_id: TargetId) {
        let mut removed = 0u32;
        for bucket in &mut self.buckets {
            let before = bucket.len();
            bucket.retain(|entry| entry.target_id != target_id);
            let after = bucket.len();
            let diff = before.saturating_sub(after);
            removed = removed.saturating_add(u32::try_from(diff).unwrap_or(u32::MAX));
        }
        self.len = self.len.saturating_sub(removed);
    }

    #[must_use]
    pub fn excluding_leased(&self, leases: &MarketLeaseRegistry, now_ns: TimestampNs) -> Self {
        let mut filtered = Self::new(self.max_length());
        for entry in self.entries() {
            if leases.is_unleased(entry.target_id, now_ns) {
                filtered.push_validated_entry(entry.clone()).unwrap();
            }
        }
        filtered
    }

    pub fn drain(&mut self) -> Vec<SupplyQueueEntry> {
        let mut drained = Vec::with_capacity(self.len());
        for bucket in self.buckets.iter_mut().rev() {
            drained.append(bucket);
        }
        self.len = 0;
        drained
    }

    pub fn to_allocation_plan(&self) -> Result<Vec<(TargetId, u128)>, SupplyQueueError> {
        let mut totals = self.totals_by_target()?;
        let mut plan = Vec::with_capacity(totals.len());

        for entry in self.entries() {
            if let Some(index) = totals
                .iter()
                .position(|(target_id, _)| *target_id == entry.target_id)
            {
                let (_, amount) = totals.remove(index);
                plan.push((entry.target_id, amount));
            }
        }

        Ok(plan)
    }

    pub fn to_allocation_plan_excluding_leased(
        &self,
        leases: &MarketLeaseRegistry,
        now_ns: TimestampNs,
    ) -> Result<Vec<(TargetId, u128)>, SupplyQueueError> {
        self.excluding_leased(leases, now_ns).to_allocation_plan()
    }

    pub fn total_for_target(&self, target_id: TargetId) -> Result<u128, SupplyQueueError> {
        self.entries()
            .into_iter()
            .filter(|entry| entry.target_id == target_id)
            .map(|entry| entry.amount)
            .try_fold(0u128, |acc, amount| {
                acc.checked_add(amount)
                    .ok_or(SupplyQueueError::AmountOverflow)
            })
    }

    #[must_use]
    pub fn has_target(&self, target_id: TargetId) -> bool {
        self.entries()
            .into_iter()
            .any(|entry| entry.target_id == target_id)
    }
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum SupplyQueueError {
    QueueFull {
        max_length: u32,
    },
    QueueTooLong {
        len: u32,
        max_length: u32,
    },
    ZeroAmount,
    PriorityBucketMismatch {
        expected_priority: u8,
        actual_priority: u8,
    },
    LengthMismatch {
        recorded_len: u32,
        actual_len: u32,
    },
    LengthOverflow,
    AmountOverflow,
    QueueEmpty,
}

fn checked_total_amount<I>(amounts: I) -> Result<u128, SupplyQueueError>
where
    I: IntoIterator<Item = u128>,
{
    amounts.into_iter().try_fold(0u128, |acc, amount| {
        acc.checked_add(amount)
            .ok_or(SupplyQueueError::AmountOverflow)
    })
}
