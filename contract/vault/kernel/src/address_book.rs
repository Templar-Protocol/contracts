use alloc::vec::Vec;

use crate::types::Address;

/// Simple address map for resolving kernel addresses to chain-specific values.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct AddressBook<T> {
    addresses: Vec<(Address, T)>,
}

impl<T> Default for AddressBook<T> {
    fn default() -> Self {
        Self {
            addresses: Vec::new(),
        }
    }
}

impl<T> AddressBook<T> {
    /// Create an empty address book.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            addresses: Vec::new(),
        }
    }

    /// Insert or update an address mapping.
    #[inline]
    pub fn insert(&mut self, address: Address, value: T) {
        if let Some((_, existing)) = self
            .addresses
            .iter_mut()
            .find(|(candidate, _)| *candidate == address)
        {
            *existing = value;
            return;
        }
        self.addresses.push((address, value));
    }

    /// Resolve a kernel address to a stored value.
    #[inline]
    #[must_use]
    pub fn resolve(&self, address: &Address) -> Option<&T> {
        self.addresses
            .iter()
            .find(|(candidate, _)| candidate == address)
            .map(|(_, value)| value)
    }

    /// Returns true if the address exists in the map.
    #[inline]
    #[must_use]
    pub fn contains(&self, address: &Address) -> bool {
        self.addresses
            .iter()
            .any(|(candidate, _)| candidate == address)
    }

    /// Returns the number of entries in the address book.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.addresses.len()
    }

    /// Returns true if the address book is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.addresses.is_empty()
    }

    #[inline]
    pub fn clear(&mut self) {
        self.addresses.clear();
    }
}

impl<T> From<Vec<(Address, T)>> for AddressBook<T> {
    fn from(addresses: Vec<(Address, T)>) -> Self {
        let mut book = Self::new();
        for (address, value) in addresses {
            book.insert(address, value);
        }
        book
    }
}

#[cfg(test)]
mod tests {
    use super::AddressBook;
    use alloc::vec;

    fn address(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    #[test]
    fn from_vec_overwrites_duplicate_addresses() {
        let book = AddressBook::from(vec![(address(1), 10u32), (address(1), 20u32)]);

        assert_eq!(book.len(), 1);
        assert_eq!(book.resolve(&address(1)), Some(&20u32));
    }
}
