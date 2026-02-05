use alloc::collections::BTreeMap;

use crate::types::Address;

/// Simple address map for resolving kernel addresses to chain-specific values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressBook<T> {
    addresses: BTreeMap<Address, T>,
}

impl<T> Default for AddressBook<T> {
    fn default() -> Self {
        Self {
            addresses: BTreeMap::new(),
        }
    }
}

impl<T> AddressBook<T> {
    /// Create an empty address book.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            addresses: BTreeMap::new(),
        }
    }

    /// Insert or update an address mapping.
    #[inline]
    pub fn insert(&mut self, address: Address, value: T) {
        self.addresses.insert(address, value);
    }

    /// Resolve a kernel address to a stored value.
    #[inline]
    #[must_use]
    pub fn resolve(&self, address: &Address) -> Option<&T> {
        self.addresses.get(address)
    }

    /// Returns true if the address exists in the map.
    #[inline]
    #[must_use]
    pub fn contains(&self, address: &Address) -> bool {
        self.addresses.contains_key(address)
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
}

impl<T> From<BTreeMap<Address, T>> for AddressBook<T> {
    fn from(addresses: BTreeMap<Address, T>) -> Self {
        Self { addresses }
    }
}
