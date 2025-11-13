use std::num::NonZeroU32;

use near_sdk::{collections::LookupMap, near, AccountId, BorshStorageKey, IntoStorageKey};

use crate::asset::BorrowAssetAmount;

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct QueueNode {
    account_id: AccountId,
    amount: BorrowAssetAmount,
    prev: Option<NonZeroU32>,
    next: Option<NonZeroU32>,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct WithdrawalQueue {
    prefix: Vec<u8>,
    length: u32,
    next_queue_node_id: NonZeroU32,
    queue: LookupMap<NonZeroU32, QueueNode>,
    queue_head: Option<NonZeroU32>,
    queue_tail: Option<NonZeroU32>,
    entries: LookupMap<AccountId, NonZeroU32>,
}

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
enum StorageKey {
    Queue,
    Entries,
}

fn inconsistent_state<T>() -> T {
    crate::panic_with_message("Inconsistent state")
}

impl WithdrawalQueue {
    pub fn new(prefix: impl IntoStorageKey) -> Self {
        let prefix = prefix.into_storage_key();
        macro_rules! key {
            ($k:ident) => {
                [prefix.clone(), StorageKey::$k.into_storage_key()].concat()
            };
        }
        Self {
            prefix: prefix.clone(),
            length: 0,
            next_queue_node_id: NonZeroU32::MIN,
            queue: LookupMap::new(key!(Queue)),
            queue_head: None,
            queue_tail: None,
            entries: LookupMap::new(key!(Entries)),
        }
    }

    #[inline]
    pub fn len(&self) -> u32 {
        self.length
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub fn get(&self, account_id: &AccountId) -> Option<BorrowAssetAmount> {
        self.entries
            .get(account_id)
            .and_then(|node_id| self.queue.get(&node_id))
            .map(|queue_node| queue_node.amount)
    }

    pub fn contains(&self, account_id: &AccountId) -> bool {
        self.entries.contains_key(account_id)
    }

    fn mut_existing_node<T>(
        &mut self,
        node_id: NonZeroU32,
        f: impl FnOnce(&mut QueueNode) -> T,
    ) -> T {
        let mut node = self.queue.get(&node_id).unwrap_or_else(inconsistent_state);
        let r = f(&mut node);
        self.queue.insert(&node_id, &node);
        r
    }

    fn set_existing_node_next(&mut self, node_id: NonZeroU32, next: Option<NonZeroU32>) {
        let mut node = self.queue.get(&node_id).unwrap_or_else(inconsistent_state);
        node.next = next;
        self.queue.insert(&node_id, &node);
    }

    fn set_existing_node_prev(&mut self, node_id: NonZeroU32, prev: Option<NonZeroU32>) {
        let mut node = self.queue.get(&node_id).unwrap_or_else(inconsistent_state);
        node.prev = prev;
        self.queue.insert(&node_id, &node);
    }

    pub fn peek(&self) -> Option<(AccountId, BorrowAssetAmount)> {
        if let Some(node_id) = self.queue_head {
            let QueueNode {
                account_id, amount, ..
            } = self.queue.get(&node_id).unwrap_or_else(inconsistent_state);
            Some((account_id, amount))
        } else {
            None
        }
    }

    pub fn mut_head<T>(&mut self, f: impl FnOnce(&mut BorrowAssetAmount) -> T) -> Option<T> {
        self.queue_head
            .map(|node_id| self.mut_existing_node(node_id, |n| f(&mut n.amount)))
    }

    /// Only pops if queue is non-empty.
    pub fn pop(&mut self) -> Option<(AccountId, BorrowAssetAmount)> {
        if let Some(node_id) = self.queue_head {
            let QueueNode {
                account_id,
                amount,
                next,
                ..
            } = self
                .queue
                .remove(&node_id)
                .unwrap_or_else(inconsistent_state);
            self.queue_head = next;
            if let Some(next_id) = next {
                self.set_existing_node_prev(next_id, None);
            } else {
                self.queue_tail = None;
            }
            self.entries.remove(&account_id);
            self.length -= 1;
            Some((account_id, amount))
        } else {
            None
        }
    }

    /// If the queue is locked, accounts can only be removed if they are not
    /// at the head of the queue.
    pub fn remove(&mut self, account_id: &AccountId) -> Option<BorrowAssetAmount> {
        if let Some(node_id) = self.entries.remove(account_id) {
            let node = self
                .queue
                .remove(&node_id)
                .unwrap_or_else(inconsistent_state);

            if let Some(next_id) = node.next {
                self.set_existing_node_prev(next_id, node.prev);
            } else {
                self.queue_tail = node.prev;
            }

            if let Some(prev_id) = node.prev {
                self.set_existing_node_next(prev_id, node.next);
            } else {
                self.queue_head = node.next;
            }

            self.length -= 1;

            Some(node.amount)
        } else {
            None
        }
    }

    pub fn insert_or_update(&mut self, account_id: &AccountId, amount: BorrowAssetAmount) {
        if let Some(node_id) = self.entries.get(account_id) {
            // update existing
            self.mut_existing_node(node_id, |node| node.amount = amount);
        } else {
            // add new
            let node_id = self.next_queue_node_id;
            {
                #![allow(clippy::unwrap_used)]
                // assume the collection never processes more than u32::MAX items
                self.next_queue_node_id = self.next_queue_node_id.checked_add(1).unwrap();
            }

            if let Some(tail_id) = self.queue_tail {
                self.set_existing_node_next(tail_id, Some(node_id));
            }
            let node = QueueNode {
                account_id: account_id.clone(),
                amount,
                prev: self.queue_tail,
                next: None,
            };
            if self.queue_head.is_none() {
                self.queue_head = Some(node_id);
            }
            self.queue_tail = Some(node_id);
            self.queue.insert(&node_id, &node);
            self.entries.insert(account_id, &node_id);
            self.length += 1;
        }
    }

    pub fn iter(&self) -> WithdrawalQueueIter {
        WithdrawalQueueIter {
            withdrawal_queue: self,
            next_node_id: self.queue_head,
        }
    }

    pub fn get_status(&self) -> WithdrawalQueueStatus {
        WithdrawalQueueStatus {
            depth: self
                .iter()
                .map(|(_, amount)| u128::from(amount))
                .sum::<u128>()
                .into(),
            length: self.len(),
        }
    }

    pub fn get_request_status(&self, account_id: &AccountId) -> Option<WithdrawalRequestStatus> {
        if !self.contains(account_id) {
            return None;
        }

        let mut depth = 0.into();
        for (index, (current_account, amount)) in self.iter().enumerate() {
            if &current_account == account_id {
                return Some(WithdrawalRequestStatus {
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "Queue length is u32, so this will never truncate"
                    )]
                    index: index as u32,
                    depth,
                    amount,
                });
            }

            depth += amount;
        }

        unreachable!()
    }
}

impl<'a> IntoIterator for &'a WithdrawalQueue {
    type IntoIter = WithdrawalQueueIter<'a>;
    type Item = (AccountId, BorrowAssetAmount);

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct WithdrawalQueueIter<'a> {
    withdrawal_queue: &'a WithdrawalQueue,
    next_node_id: Option<NonZeroU32>,
}

impl Iterator for WithdrawalQueueIter<'_> {
    type Item = (AccountId, BorrowAssetAmount);

    fn next(&mut self) -> Option<Self::Item> {
        let next_node_id = self.next_node_id?;
        let r = self
            .withdrawal_queue
            .queue
            .get(&next_node_id)
            .unwrap_or_else(inconsistent_state);
        self.next_node_id = r.next;
        Some((r.account_id, r.amount))
    }
}

/// Status of a single account in the withdrawal queue.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct WithdrawalRequestStatus {
    /// What index is this account in the queue?
    /// That is, how many other withdrawal requests are ahead of this account
    /// in the queue?
    pub index: u32,
    /// Sum of requested amounts of the requests ahead of this account in the
    /// queue.
    pub depth: BorrowAssetAmount,
    /// The amount that this account has requested to withdraw from the
    /// contract.
    pub amount: BorrowAssetAmount,
}

/// Status of the withdrawal queue.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct WithdrawalQueueStatus {
    /// Sum of all amounts of requests in the queue.
    pub depth: BorrowAssetAmount,
    /// Number of requests in the queue.
    pub length: u32,
}

/// Return value after executing requests from the withdrawal queue.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct WithdrawalQueueExecutionResult {
    /// What is the total value of the requests that were cleared from the queue?
    pub depth: BorrowAssetAmount,
    /// How many requests were cleared from the queue?
    pub length: u32,
}

pub mod error {
    use thiserror::Error;

    #[derive(Error, Debug)]
    #[error("The withdrawal queue is empty")]
    pub struct EmptyError;
}

#[cfg(test)]
mod tests {
    use near_sdk::AccountId;

    use super::WithdrawalQueue;

    #[test]
    fn mut_head() {
        let mut wq = WithdrawalQueue::new(b"w");

        let alice: AccountId = "alice".parse().unwrap();
        let bob: AccountId = "bob".parse().unwrap();
        let charlie: AccountId = "charlie".parse().unwrap();

        wq.insert_or_update(&alice, 1.into());
        wq.insert_or_update(&bob, 2.into());
        wq.insert_or_update(&charlie, 3.into());

        wq.mut_head(|a| *a += 10).unwrap();
        assert_eq!(wq.len(), 3);

        assert_eq!(wq.get(&alice).unwrap(), 11.into());
        assert_eq!(wq.get(&bob).unwrap(), 2.into());
        assert_eq!(wq.get(&charlie).unwrap(), 3.into());
        assert_eq!(wq.remove(&alice).unwrap(), 11.into());
        assert_eq!(wq.len(), 2);

        wq.mut_head(|a| *a += 20).unwrap();
        assert_eq!(wq.get(&alice), None);
        assert_eq!(wq.get(&bob).unwrap(), 22.into());
        assert_eq!(wq.get(&charlie).unwrap(), 3.into());
        assert_eq!(wq.remove(&bob).unwrap(), 22.into());
        assert_eq!(wq.len(), 1);

        wq.mut_head(|a| *a += 30).unwrap();
        assert_eq!(wq.get(&alice), None);
        assert_eq!(wq.get(&bob), None);
        assert_eq!(wq.get(&charlie).unwrap(), 33.into());
        assert_eq!(wq.remove(&charlie).unwrap(), 33.into());
        assert_eq!(wq.len(), 0);
    }

    #[test]
    fn withdrawal_remove() {
        let mut wq = WithdrawalQueue::new(b"w");

        let alice: AccountId = "alice".parse().unwrap();
        let bob: AccountId = "bob".parse().unwrap();
        let charlie: AccountId = "charlie".parse().unwrap();

        wq.insert_or_update(&alice, 1.into());
        wq.insert_or_update(&bob, 2.into());
        wq.insert_or_update(&charlie, 3.into());
        assert_eq!(wq.len(), 3);
        assert_eq!(wq.remove(&bob), Some(2.into()));
        assert_eq!(wq.len(), 2);
        assert_eq!(wq.remove(&charlie), Some(3.into()));
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.remove(&alice), Some(1.into()));
        assert_eq!(wq.len(), 0);
    }

    #[test]
    fn withdrawal_queueing() {
        let mut wq = WithdrawalQueue::new(b"w");

        let alice: AccountId = "alice".parse().unwrap();
        let bob: AccountId = "bob".parse().unwrap();
        let charlie: AccountId = "charlie".parse().unwrap();

        assert_eq!(wq.len(), 0);
        assert_eq!(wq.peek(), None);
        wq.insert_or_update(&alice, 1.into());
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.peek(), Some((alice.clone(), 1.into())));
        wq.insert_or_update(&alice, 99.into());
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.peek(), Some((alice.clone(), 99.into())));
        wq.insert_or_update(&bob, 123.into());
        assert_eq!(wq.len(), 2);

        assert_eq!(wq.pop(), Some((alice.clone(), 99.into())));
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.peek(), Some((bob.clone(), 123.into())));

        wq.insert_or_update(&charlie, 8080.into());
        assert_eq!(wq.len(), 2);
        assert_eq!(wq.peek(), Some((bob.clone(), 123.into())));

        assert_eq!(wq.pop(), Some((bob.clone(), 123.into())));
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.peek(), Some((charlie.clone(), 8080.into())));

        assert_eq!(wq.pop(), Some((charlie.clone(), 8080.into())));
        assert_eq!(wq.len(), 0);
        assert_eq!(wq.peek(), None);
    }
}
