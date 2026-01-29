//! Storage versioning and persistence for Soroban ledger.
//!
//! This module provides versioned storage wrappers for persisting vault state
//! to the Soroban ledger. It handles schema migrations and forward compatibility.

use derive_more::{From, Into};
use templar_vault_kernel::VaultState;

use crate::error::RuntimeError;

/// Storage version identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, From, Into)]
pub struct StorageVersion(pub u32);

impl StorageVersion {
    /// Initial storage version.
    pub const V1: Self = Self(1);

    /// Current storage version.
    pub const CURRENT: Self = Self::V1;

    /// Create a new storage version.
    #[inline]
    #[must_use]
    pub const fn new(version: u32) -> Self {
        Self(version)
    }

    /// Get the version number.
    #[inline]
    #[must_use]
    pub const fn number(&self) -> u32 {
        self.0
    }

    /// Check if this version is compatible with the current version.
    #[inline]
    #[must_use]
    pub const fn is_compatible(&self) -> bool {
        self.0 <= Self::CURRENT.0
    }
}

impl Default for StorageVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

/// Versioned state wrapper.
///
/// Wraps vault state with version information for storage migration support.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionedState {
    /// Storage schema version.
    pub version: StorageVersion,
    /// The vault state.
    pub state: VaultState,
}

impl VersionedState {
    /// Create a new versioned state at the current version.
    #[inline]
    #[must_use]
    pub fn new(state: VaultState) -> Self {
        Self {
            version: StorageVersion::CURRENT,
            state,
        }
    }

    /// Create a versioned state with a specific version (for testing/migration).
    #[inline]
    #[must_use]
    pub fn with_version(version: StorageVersion, state: VaultState) -> Self {
        Self { version, state }
    }

    /// Check if this state needs migration to the current version.
    #[inline]
    #[must_use]
    pub fn needs_migration(&self) -> bool {
        self.version < StorageVersion::CURRENT
    }

    /// Get the version number.
    #[inline]
    #[must_use]
    pub const fn version_number(&self) -> u32 {
        self.version.0
    }
}

impl Default for VersionedState {
    fn default() -> Self {
        Self::new(VaultState::default())
    }
}

/// Storage key types for different data categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageKey {
    /// Main vault state.
    VaultState,
    /// Storage version.
    Version,
    /// Pending withdrawal by ID.
    PendingWithdrawal(u64),
    /// Share balance for an account.
    ShareBalance([u8; 32]),
    /// Total share supply.
    TotalSupply,
}

/// Trait for storage operations.
///
/// Implementations of this trait handle the actual persistence to the
/// Soroban ledger.
pub trait Storage {
    /// Load the versioned state from storage.
    ///
    /// Returns `None` if no state exists (fresh deployment).
    fn load_state(&self) -> Result<Option<VersionedState>, RuntimeError>;

    /// Save the versioned state to storage.
    fn save_state(&mut self, state: &VersionedState) -> Result<(), RuntimeError>;

    /// Check if storage has been initialized.
    fn is_initialized(&self) -> bool;

    /// Get the storage version.
    fn get_version(&self) -> Result<StorageVersion, RuntimeError>;
}

/// In-memory storage implementation for testing.
#[derive(Clone, Debug, Default)]
pub struct MemoryStorage {
    state: Option<VersionedState>,
    initialized: bool,
}

impl MemoryStorage {
    /// Create a new empty memory storage.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a memory storage with initial state.
    #[inline]
    #[must_use]
    pub fn with_state(state: VersionedState) -> Self {
        Self {
            state: Some(state),
            initialized: true,
        }
    }

    /// Get a reference to the stored state.
    #[inline]
    #[must_use]
    pub fn get_state(&self) -> Option<&VersionedState> {
        self.state.as_ref()
    }

    /// Clear the storage.
    #[inline]
    pub fn clear(&mut self) {
        self.state = None;
        self.initialized = false;
    }
}

impl Storage for MemoryStorage {
    fn load_state(&self) -> Result<Option<VersionedState>, RuntimeError> {
        Ok(self.state.clone())
    }

    fn save_state(&mut self, state: &VersionedState) -> Result<(), RuntimeError> {
        self.state = Some(state.clone());
        self.initialized = true;
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn get_version(&self) -> Result<StorageVersion, RuntimeError> {
        self.state
            .as_ref()
            .map(|s| s.version)
            .ok_or_else(|| RuntimeError::storage_error("state not initialized"))
    }
}

/// Migration helper for upgrading storage versions.
pub struct Migrator;

impl Migrator {
    /// Migrate state from one version to the current version.
    ///
    /// This function applies sequential migrations from the source version
    /// to the current version.
    ///
    /// # Arguments
    ///
    /// * `state` - The versioned state to migrate.
    ///
    /// # Returns
    ///
    /// The migrated state at the current version.
    pub fn migrate(state: VersionedState) -> Result<VersionedState, RuntimeError> {
        if !state.version.is_compatible() {
            return Err(RuntimeError::storage_error(
                "state version is newer than supported",
            ));
        }

        if !state.needs_migration() {
            return Ok(state);
        }

        let mut current = state;

        // Apply migrations sequentially
        // Currently only V1, so no migrations needed
        // Future migrations would be added here:
        // if current.version == StorageVersion::V1 {
        //     current = migrate_v1_to_v2(current)?;
        // }

        current.version = StorageVersion::CURRENT;
        Ok(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_version() {
        let v1 = StorageVersion::V1;
        assert_eq!(v1.number(), 1);
        assert!(v1.is_compatible());

        let current = StorageVersion::CURRENT;
        assert_eq!(current, v1);
    }

    #[test]
    fn test_storage_version_compatibility() {
        let old = StorageVersion::new(1);
        assert!(old.is_compatible());

        // Future version would not be compatible
        let future = StorageVersion::new(999);
        assert!(!future.is_compatible());
    }

    #[test]
    fn test_versioned_state_new() {
        let state = VaultState::default();
        let versioned = VersionedState::new(state);

        assert_eq!(versioned.version, StorageVersion::CURRENT);
        assert!(!versioned.needs_migration());
    }

    #[test]
    fn test_versioned_state_migration_check() {
        let state = VaultState::default();
        let versioned = VersionedState::with_version(StorageVersion::V1, state);

        // V1 is current, so no migration needed
        assert!(!versioned.needs_migration());
    }

    #[test]
    fn test_memory_storage_empty() {
        let storage = MemoryStorage::new();
        assert!(!storage.is_initialized());
        assert!(storage.load_state().unwrap().is_none());
    }

    #[test]
    fn test_memory_storage_save_load() {
        let mut storage = MemoryStorage::new();
        let state = VersionedState::default();

        storage.save_state(&state).unwrap();
        assert!(storage.is_initialized());

        let loaded = storage.load_state().unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap(), state);
    }

    #[test]
    fn test_memory_storage_with_state() {
        let state = VersionedState::default();
        let storage = MemoryStorage::with_state(state.clone());

        assert!(storage.is_initialized());
        assert_eq!(storage.get_state(), Some(&state));
    }

    #[test]
    fn test_memory_storage_clear() {
        let state = VersionedState::default();
        let mut storage = MemoryStorage::with_state(state);

        storage.clear();
        assert!(!storage.is_initialized());
        assert!(storage.get_state().is_none());
    }

    #[test]
    fn test_migrator_no_migration_needed() {
        let state = VersionedState::default();
        let migrated = Migrator::migrate(state.clone()).unwrap();

        assert_eq!(migrated.version, StorageVersion::CURRENT);
        assert_eq!(migrated.state, state.state);
    }

    #[test]
    fn test_migrator_rejects_future_version() {
        let state = VersionedState::with_version(StorageVersion::new(999), VaultState::default());
        let result = Migrator::migrate(state);

        assert!(result.is_err());
        assert!(matches!(result, Err(RuntimeError::StorageError(_))));
    }

    #[test]
    fn test_storage_key_variants() {
        let key1 = StorageKey::VaultState;
        let key2 = StorageKey::Version;
        let key3 = StorageKey::PendingWithdrawal(42);
        let key4 = StorageKey::ShareBalance([0u8; 32]);
        let key5 = StorageKey::TotalSupply;

        assert_ne!(key1, key2);
        assert_ne!(key3, key4);
        assert_ne!(key4, key5);
    }
}
