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
