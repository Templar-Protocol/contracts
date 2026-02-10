//! Shared helpers for target-set validation.

use alloc::collections::BTreeSet;

/// Returns the first duplicate item found in insertion order.
#[must_use]
pub fn find_first_duplicate<T: Ord + Copy>(items: &[T]) -> Option<T> {
    let mut seen = BTreeSet::new();
    for item in items {
        if !seen.insert(*item) {
            return Some(*item);
        }
    }
    None
}

/// Returns true when all items are unique.
#[must_use]
pub fn has_unique_items<T: Ord + Copy>(items: &[T]) -> bool {
    find_first_duplicate(items).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_first_duplicate() {
        assert_eq!(find_first_duplicate(&[1u32, 2, 3]), None);
        assert_eq!(find_first_duplicate(&[1u32, 2, 1]), Some(1));
        assert_eq!(find_first_duplicate(&[1u32, 2, 2, 3]), Some(2));
    }

    #[test]
    fn validates_uniqueness() {
        assert!(has_unique_items(&[1u32, 2, 3]));
        assert!(!has_unique_items(&[1u32, 2, 1]));
    }
}
