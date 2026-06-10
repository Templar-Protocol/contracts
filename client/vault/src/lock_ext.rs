//! Extension traits for handling poisoned locks with less boilerplate.
//!
//! All helpers now propagate poisoned-lock failures to avoid silently
//! operating on potentially inconsistent state.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::ErrorWrapper;

/// Extension trait for `Mutex` to handle poisoned locks.
#[allow(dead_code)]
pub trait MutexExt<T> {
    /// Lock the mutex, returning an error if poisoned.
    fn lock_or_poison(&self) -> Result<MutexGuard<'_, T>, ErrorWrapper>;

    /// Compatibility alias for `lock_or_poison`.
    fn lock_recover(&self) -> Result<MutexGuard<'_, T>, ErrorWrapper>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn lock_or_poison(&self) -> Result<MutexGuard<'_, T>, ErrorWrapper> {
        self.lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))
    }

    fn lock_recover(&self) -> Result<MutexGuard<'_, T>, ErrorWrapper> {
        self.lock_or_poison()
    }
}

/// Extension trait for `RwLock` to handle poisoned locks.
#[allow(dead_code)]
pub trait RwLockExt<T> {
    /// Acquire a read lock, returning an error if poisoned.
    fn read_or_poison(&self) -> Result<RwLockReadGuard<'_, T>, ErrorWrapper>;

    /// Acquire a write lock, returning an error if poisoned.
    fn write_or_poison(&self) -> Result<RwLockWriteGuard<'_, T>, ErrorWrapper>;

    /// Compatibility alias for `read_or_poison`.
    fn read_recover(&self) -> Result<RwLockReadGuard<'_, T>, ErrorWrapper>;

    /// Compatibility alias for `write_or_poison`.
    fn write_recover(&self) -> Result<RwLockWriteGuard<'_, T>, ErrorWrapper>;
}

impl<T> RwLockExt<T> for RwLock<T> {
    fn read_or_poison(&self) -> Result<RwLockReadGuard<'_, T>, ErrorWrapper> {
        self.read()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))
    }

    fn write_or_poison(&self) -> Result<RwLockWriteGuard<'_, T>, ErrorWrapper> {
        self.write()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))
    }

    fn read_recover(&self) -> Result<RwLockReadGuard<'_, T>, ErrorWrapper> {
        self.read_or_poison()
    }

    fn write_recover(&self) -> Result<RwLockWriteGuard<'_, T>, ErrorWrapper> {
        self.write_or_poison()
    }
}

#[cfg(test)]
mod tests {
    use super::{MutexExt, RwLockExt};
    use std::sync::{Arc, Mutex, RwLock};

    #[test]
    fn mutex_helpers_read_and_write_values() {
        let lock = Mutex::new(7_u32);
        assert_eq!(*lock.lock_or_poison().expect("lock must succeed"), 7);
        *lock.lock_recover().expect("alias must succeed") = 9;
        assert_eq!(*lock.lock_or_poison().expect("lock must succeed"), 9);
    }

    #[test]
    fn rwlock_helpers_read_and_write_values() {
        let lock = RwLock::new(11_u32);
        assert_eq!(*lock.read_or_poison().expect("read must succeed"), 11);
        *lock.write_recover().expect("write alias must succeed") = 13;
        assert_eq!(*lock.read_recover().expect("read alias must succeed"), 13);
    }

    #[test]
    fn poisoned_mutex_returns_error() {
        let lock = Arc::new(Mutex::new(1_u32));
        let cloned = Arc::clone(&lock);
        let _ = std::thread::spawn(move || {
            let _guard = cloned.lock().expect("lock must succeed before panic");
            panic!("poison mutex");
        })
        .join();

        assert!(lock.lock_or_poison().is_err());
    }
}
