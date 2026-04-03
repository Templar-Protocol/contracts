use alloc::collections::BTreeSet;

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

#[must_use]
pub fn has_unique_items<T: Ord + Copy>(items: &[T]) -> bool {
    find_first_duplicate(items).is_none()
}
