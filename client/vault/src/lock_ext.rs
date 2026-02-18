//! Extension traits for handling poisoned locks with less boilerplate.
//!
//! Provides two patterns:
//! 1. `lock_or_poison()` / `read_or_poison()` / `write_or_poison()` - Returns `Result<Guard, ErrorWrapper>`
//! 2. `lock_recover()` / `read_recover()` / `write_recover()` - Recovers from poison, returns guard directly

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::ErrorWrapper;

/// Extension trait for `Mutex` to handle poisoned locks.
#[allow(dead_code)]
pub trait MutexExt<T> {
    /// Lock the mutex, returning an error if poisoned.
    fn lock_or_poison(&self) -> Result<MutexGuard<'_, T>, ErrorWrapper>;

    /// Lock the mutex, recovering from poison by extracting the inner value.
    fn lock_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn lock_or_poison(&self) -> Result<MutexGuard<'_, T>, ErrorWrapper> {
        self.lock()
            .map_err(|_| ErrorWrapper::Wrapped("poisoned lock".to_string()))
    }

    fn lock_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Extension trait for `RwLock` to handle poisoned locks.
#[allow(dead_code)]
pub trait RwLockExt<T> {
    /// Acquire a read lock, returning an error if poisoned.
    fn read_or_poison(&self) -> Result<RwLockReadGuard<'_, T>, ErrorWrapper>;

    /// Acquire a write lock, returning an error if poisoned.
    fn write_or_poison(&self) -> Result<RwLockWriteGuard<'_, T>, ErrorWrapper>;

    /// Acquire a read lock, recovering from poison by extracting the inner value.
    fn read_recover(&self) -> RwLockReadGuard<'_, T>;

    /// Acquire a write lock, recovering from poison by extracting the inner value.
    fn write_recover(&self) -> RwLockWriteGuard<'_, T>;
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

    fn read_recover(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write_recover(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(|e| e.into_inner())
    }
}
