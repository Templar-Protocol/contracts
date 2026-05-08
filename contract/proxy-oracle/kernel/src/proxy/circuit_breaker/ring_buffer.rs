use alloc::vec::Vec;

#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::{format, string::ToString};

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RingBuffer<T> {
        capacity: u32,
        entries: Vec<T>,
    }
}

impl<T> RingBuffer<T> {
    #[must_use]
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity,
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, item: T) {
        let capacity = self.capacity as usize;
        if capacity == 0 {
            return;
        }

        if self.entries.len() == capacity {
            self.entries.remove(0);
        }

        self.entries.push(item);
    }

    pub fn set_capacity(&mut self, capacity: u32) {
        self.capacity = capacity;
        let capacity = capacity as usize;
        let excess = self.entries.len().saturating_sub(capacity);
        if excess > 0 {
            self.entries.drain(0..excess);
        }
    }

    #[must_use]
    pub fn last(&self) -> Option<&T> {
        self.entries.last()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_noops_when_capacity_is_zero() {
        let mut buffer = RingBuffer::new(0);

        buffer.push(1);
        buffer.push(2);

        assert!(buffer.is_empty());
        assert_eq!(buffer.last(), None);
    }

    #[test]
    fn push_preserves_insertion_order_before_capacity() {
        let mut buffer = RingBuffer::new(3);

        buffer.push(1);
        buffer.push(2);

        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.as_slice(), &[1, 2]);
        assert_eq!(buffer.last(), Some(&2));
    }

    #[test]
    fn push_drops_oldest_entry_at_capacity() {
        let mut buffer = RingBuffer::new(3);

        buffer.push(1);
        buffer.push(2);
        buffer.push(3);
        buffer.push(4);
        buffer.push(5);

        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.as_slice(), &[3, 4, 5]);
        assert_eq!(buffer.last(), Some(&5));
    }

    #[test]
    fn set_capacity_grow_preserves_existing_entries() {
        let mut buffer = RingBuffer::new(2);
        buffer.push(1);
        buffer.push(2);

        buffer.set_capacity(4);
        buffer.push(3);
        buffer.push(4);

        assert_eq!(buffer.as_slice(), &[1, 2, 3, 4]);
    }

    #[test]
    fn set_capacity_shrink_keeps_newest_entries() {
        let mut buffer = RingBuffer::new(4);
        for item in 1..=4 {
            buffer.push(item);
        }

        buffer.set_capacity(2);

        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.as_slice(), &[3, 4]);
        assert_eq!(buffer.last(), Some(&4));
    }

    #[test]
    fn set_capacity_zero_clears_entries_and_blocks_future_pushes() {
        let mut buffer = RingBuffer::new(2);
        buffer.push(1);
        buffer.push(2);

        buffer.set_capacity(0);
        buffer.push(3);

        assert!(buffer.is_empty());
    }

    #[test]
    fn set_capacity_after_zero_allows_future_entries() {
        let mut buffer = RingBuffer::new(0);
        buffer.push(1);

        buffer.set_capacity(2);
        buffer.push(2);
        buffer.push(3);

        assert_eq!(buffer.as_slice(), &[2, 3]);
    }

    #[test]
    fn set_capacity_to_same_value_is_stable() {
        let mut buffer = RingBuffer::new(3);
        for item in 1..=3 {
            buffer.push(item);
        }
        buffer.set_capacity(3);

        assert_eq!(buffer.as_slice(), &[1, 2, 3]);
    }
}
