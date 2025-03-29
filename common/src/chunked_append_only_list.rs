use borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::{env, near, store::Vector, BorshStorageKey, IntoStorageKey};

#[derive(Debug, Clone, Copy, BorshSerialize, BorshStorageKey, PartialEq, Eq, PartialOrd, Ord)]
enum StorageKey {
    Inner,
}

/// Represents an append-only iterable list that stores multiple items per
/// storage slot to reduce gas cost when reading.
#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct ChunkedAppendOnlyList<T: BorshSerialize + BorshDeserialize, const CHUNK_SIZE: u32> {
    inner: Vector<Vec<T>>,
    last_chunk_next_index: u32,
}

impl<T: BorshSerialize + BorshDeserialize, const CHUNK_SIZE: u32>
    ChunkedAppendOnlyList<T, CHUNK_SIZE>
{
    pub fn new(prefix: impl IntoStorageKey) -> Self {
        Self {
            inner: Vector::new(
                [
                    prefix.into_storage_key(),
                    StorageKey::Inner.into_storage_key(),
                ]
                .concat(),
            ),
            last_chunk_next_index: 0,
        }
    }

    pub fn len(&self) -> u32 {
        if let Some(last_index) = self.inner.len().checked_sub(1) {
            let mut full_count = last_index * CHUNK_SIZE;
            if self.last_chunk_next_index == 0 {
                full_count += CHUNK_SIZE;
            }
            full_count + self.last_chunk_next_index
        } else {
            0
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn push(&mut self, item: T) {
        if self.last_chunk_next_index == 0 {
            let v = vec![item];
            self.inner.push(v);
        } else {
            let v = self
                .inner
                .get_mut(
                    self.inner
                        .len()
                        .checked_sub(1)
                        .unwrap_or_else(|| env::panic_str("Inconsistent state: len == 0")),
                )
                .unwrap_or_else(|| env::panic_str("Inconsistent state: tail dne"));
            v.push(item);
        }
        self.last_chunk_next_index = (self.last_chunk_next_index + 1) % CHUNK_SIZE;
    }

    pub fn get(&self, index: u32) -> Option<&T> {
        self.inner
            .get(index / CHUNK_SIZE)
            .and_then(|v| v.get((index % CHUNK_SIZE) as usize))
    }

    pub fn replace_last(&mut self, item: T) {
        if let Some(entry) = self
            .inner
            .len()
            .checked_sub(1)
            .and_then(|last_index| self.inner.get_mut(last_index))
            .and_then(|v| v.last_mut())
        {
            *entry = item;
        } else {
            env::panic_str("Cannot replace_last in empty list");
        }
    }

    pub fn iter(&self) -> Iter<'_, T, CHUNK_SIZE> {
        Iter {
            list: self,
            next_index: 0,
            until_index: self.len(),
        }
    }

    pub fn flush(&mut self) {
        self.inner.flush();
    }
}

impl<'a, T: BorshSerialize + BorshDeserialize, const CHUNK_SIZE: u32> IntoIterator
    for &'a ChunkedAppendOnlyList<T, CHUNK_SIZE>
{
    type Item = &'a T;

    type IntoIter = Iter<'a, T, CHUNK_SIZE>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct Iter<'a, T: BorshSerialize + BorshDeserialize, const CHUNK_SIZE: u32> {
    list: &'a ChunkedAppendOnlyList<T, CHUNK_SIZE>,
    next_index: u32,
    until_index: u32,
}

impl<'a, T: BorshSerialize + BorshDeserialize, const CHUNK_SIZE: u32> Iterator
    for Iter<'a, T, CHUNK_SIZE>
{
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.until_index <= self.next_index {
            return None;
        }

        if let Some(value) = self.list.get(self.next_index) {
            self.next_index += 1;
            Some(value)
        } else {
            None
        }
    }
}

impl<T: BorshSerialize + BorshDeserialize, const CHUNK_SIZE: u32> DoubleEndedIterator
    for Iter<'_, T, CHUNK_SIZE>
{
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.until_index <= self.next_index {
            return None;
        }

        if let Some((index, value)) = self
            .until_index
            .checked_sub(1)
            .and_then(|index| self.list.get(index).map(|x| (index, x)))
        {
            self.until_index = index;
            Some(value)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let mut list = ChunkedAppendOnlyList::<_, 47>::new(b"l");
        assert_eq!(list.len(), 0);
        assert!(list.is_empty());

        for i in 0..10_000usize {
            list.push(i);
            assert_eq!(list.len() as usize, i + 1);
            assert!(!list.is_empty());
        }

        let mut count = 0;
        for (i, v) in list.iter().enumerate() {
            assert_eq!(i, *v);
            count += 1;
        }

        assert_eq!(count, 10_000);
    }

    #[test]
    fn replace_last() {
        let mut list = ChunkedAppendOnlyList::<_, 47>::new(b"l");
        for i in 0..10_000u32 {
            list.push(i);
            list.replace_last(i * 2);
            assert_eq!(list.len(), i + 1);
            assert!(!list.is_empty());
        }

        for i in 0..10_000u32 {
            let x = list.get(i).unwrap();
            assert_eq!(*x, i * 2);
        }

        assert_eq!(list.len(), 10_000);
    }

    #[test]
    fn next_back() {
        let mut list = ChunkedAppendOnlyList::<_, 47>::new(b"l");
        for i in 0..10_000u32 {
            list.push(i);
        }

        let mut it = list.iter();

        let mut i = 10_000;
        while let Some(x) = it.next_back() {
            i -= 1;
            assert_eq!(*x, i);
        }

        assert_eq!(i, 0);
    }
}
