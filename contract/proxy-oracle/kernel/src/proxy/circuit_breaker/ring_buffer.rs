use alloc::vec::Vec;

#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::{format, string::ToString};

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct UncheckedRingBuffer<T> {
        pub capacity: u32,
        pub entries: Vec<T>,
    }
}

#[cfg_attr(
    feature = "serde",
    derive(::serde::Deserialize, ::serde::Serialize),
    serde(
        try_from = "UncheckedRingBuffer<T>",
        into = "UncheckedRingBuffer<T>",
        bound(
            serialize = "T: Clone + ::serde::Serialize",
            deserialize = "T: ::serde::Deserialize<'de>"
        )
    )
)]
#[cfg_attr(
    feature = "schemars",
    derive(::schemars::JsonSchema),
    schemars(transparent)
)]
#[cfg_attr(
    feature = "borsh",
    derive(::borsh::BorshSerialize, ::borsh::BorshSchema)
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingBuffer<T>(UncheckedRingBuffer<T>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingBufferParseError {
    EntriesExceedCapacity,
}

impl core::fmt::Display for RingBufferParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EntriesExceedCapacity => write!(f, "entries exceed ring buffer capacity"),
        }
    }
}

impl<T> TryFrom<UncheckedRingBuffer<T>> for RingBuffer<T> {
    type Error = RingBufferParseError;

    fn try_from(value: UncheckedRingBuffer<T>) -> Result<Self, Self::Error> {
        if value.entries.len() > value.capacity as usize {
            return Err(RingBufferParseError::EntriesExceedCapacity);
        }
        Ok(Self(value))
    }
}

impl<T> From<RingBuffer<T>> for UncheckedRingBuffer<T> {
    fn from(value: RingBuffer<T>) -> Self {
        value.0
    }
}

impl<T> RingBuffer<T> {
    #[must_use]
    pub fn new(capacity: u32) -> Self {
        Self(UncheckedRingBuffer {
            capacity,
            entries: Vec::new(),
        })
    }

    pub fn push(&mut self, item: T) {
        let capacity = self.0.capacity as usize;
        if capacity == 0 {
            return;
        }

        if self.0.entries.len() == capacity {
            self.0.entries.remove(0);
        }

        self.0.entries.push(item);
    }

    pub fn set_capacity(&mut self, capacity: u32) {
        self.0.capacity = capacity;
        let capacity = capacity as usize;
        let excess = self.0.entries.len().saturating_sub(capacity);
        if excess > 0 {
            self.0.entries.drain(0..excess);
        }
    }

    #[must_use]
    pub fn last(&self) -> Option<&T> {
        self.0.entries.last()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.entries.is_empty()
    }

    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.0.entries
    }
}

#[cfg(feature = "borsh")]
impl<T: ::borsh::BorshDeserialize> ::borsh::BorshDeserialize for RingBuffer<T> {
    fn deserialize_reader<Reader: ::borsh::io::Read>(
        reader: &mut Reader,
    ) -> ::borsh::io::Result<Self> {
        let unchecked =
            <UncheckedRingBuffer<T> as ::borsh::BorshDeserialize>::deserialize_reader(reader)?;
        unchecked.try_into().map_err(|_| {
            ::borsh::io::Error::new(
                ::borsh::io::ErrorKind::InvalidData,
                "could not parse ring buffer",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

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
    fn parse_rejects_entries_exceeding_capacity() {
        assert_eq!(
            RingBuffer::try_from(UncheckedRingBuffer {
                capacity: 1,
                entries: vec![1, 2],
            }),
            Err(RingBufferParseError::EntriesExceedCapacity)
        );
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn borsh_rejects_entries_exceeding_capacity() {
        let unchecked = UncheckedRingBuffer {
            capacity: 1,
            entries: vec![1_u32, 2],
        };
        let bytes = borsh::to_vec(&unchecked).unwrap();

        assert!(borsh::from_slice::<RingBuffer<u32>>(&bytes).is_err());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_rejects_entries_exceeding_capacity() {
        let unchecked = UncheckedRingBuffer {
            capacity: 1,
            entries: vec![1_u32, 2],
        };
        let bytes = serde_json::to_vec(&unchecked).unwrap();

        assert!(serde_json::from_slice::<RingBuffer<u32>>(&bytes).is_err());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_serializes_like_unchecked_representation() {
        let mut buffer = RingBuffer::new(2);
        buffer.push(1_u32);
        buffer.push(2);
        let unchecked = UncheckedRingBuffer::from(buffer.clone());

        assert_eq!(
            serde_json::to_value(&buffer).unwrap(),
            serde_json::to_value(&unchecked).unwrap()
        );
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
