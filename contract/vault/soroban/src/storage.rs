//! Storage versioning and persistence for Soroban ledger.
//!
//! This module provides versioned storage wrappers for persisting vault state
//! to the Soroban ledger. It handles schema migrations and forward compatibility.

use derive_more::{From, Into};
use soroban_sdk::{contracttype, Env};
use templar_vault_kernel::VaultState;

use crate::error::RuntimeError;

// ---------------------------------------------------------------------------
// Soroban Storage Keys
// ---------------------------------------------------------------------------

/// Storage keys for Soroban ledger storage.
///
/// Using `#[contracttype]` allows the key enum to be used with Soroban's
/// native storage API.
#[contracttype]
#[derive(Clone, Debug)]
pub enum SorobanStorageKey {
    /// Main vault state stored as serialized fields.
    VaultState,
    /// Storage version number.
    Version,
    /// Contract configuration.
    Config,
    /// Pause flag.
    Paused,
}

// ---------------------------------------------------------------------------
// Serializable Vault State (Soroban-compatible)
// ---------------------------------------------------------------------------

/// Soroban-compatible vault state representation.
///
/// This mirrors `VaultState` from the kernel but uses types compatible
/// with Soroban's storage (i128 instead of u128, etc.).
#[contracttype]
#[derive(Clone, Debug, Default)]
pub struct SorobanVaultState {
    /// Total assets under management.
    pub total_assets: i128,
    /// Total vault shares in circulation.
    pub total_shares: i128,
    /// Assets held idle in the vault.
    pub idle_assets: i128,
    /// Assets deployed externally.
    pub external_assets: i128,
    /// Fee anchor timestamp (nanoseconds).
    pub fee_anchor_ns: u64,
    /// Fee anchor total assets at anchor time.
    pub fee_anchor_assets: i128,
    /// Operation state: 0=Idle, 1=Allocating, 2=Refreshing
    pub op_state_kind: u32,
    /// Current operation ID (if not idle).
    pub op_state_id: u64,
    /// Number of pending withdrawals.
    pub withdraw_queue_len: u32,
    /// Next operation ID.
    pub next_op_id: u64,
}

impl SorobanVaultState {
    /// Convert from kernel VaultState.
    #[must_use]
    pub fn from_kernel(state: &VaultState) -> Self {
        use templar_vault_kernel::OpState;

        let (op_state_kind, op_state_id) = match &state.op_state {
            OpState::Idle => (0, 0),
            OpState::Allocating(s) => (1, s.op_id),
            OpState::Withdrawing(s) => (2, s.op_id),
            OpState::Refreshing(s) => (3, s.op_id),
            OpState::Payout(s) => (4, s.op_id),
        };

        Self {
            total_assets: state.total_assets as i128,
            total_shares: state.total_shares as i128,
            idle_assets: state.idle_assets as i128,
            external_assets: state.external_assets as i128,
            fee_anchor_ns: state.fee_anchor.timestamp_ns,
            fee_anchor_assets: state.fee_anchor.total_assets as i128,
            op_state_kind,
            op_state_id,
            withdraw_queue_len: state.withdraw_queue.len() as u32,
            next_op_id: state.next_op_id,
        }
    }

    /// Convert to kernel VaultState.
    ///
    /// Note: This creates a minimal VaultState for Idle state only.
    /// Non-idle states should be reconstructed from full persistent storage.
    #[must_use]
    pub fn to_kernel(&self) -> VaultState {
        use alloc::vec::Vec;
        use templar_vault_kernel::{
            AllocatingState, FeeAccrualAnchor, OpState, PayoutState, RefreshingState,
            WithdrawQueue, WithdrawingState,
        };

        let op_state = match self.op_state_kind {
            1 => OpState::Allocating(AllocatingState {
                op_id: self.op_state_id,
                index: 0,
                remaining: 0,
                plan: Vec::new(),
            }),
            2 => OpState::Withdrawing(WithdrawingState {
                op_id: self.op_state_id,
                index: 0,
                remaining: 0,
                collected: 0,
                receiver: [0u8; 32],
                owner: [0u8; 32],
                escrow_shares: 0,
            }),
            3 => OpState::Refreshing(RefreshingState {
                op_id: self.op_state_id,
                index: 0,
                plan: Vec::new(),
            }),
            4 => OpState::Payout(PayoutState {
                op_id: self.op_state_id,
                receiver: [0u8; 32],
                amount: 0,
                owner: [0u8; 32],
                escrow_shares: 0,
                burn_shares: 0,
            }),
            _ => OpState::Idle,
        };

        VaultState {
            total_assets: self.total_assets as u128,
            total_shares: self.total_shares as u128,
            idle_assets: self.idle_assets as u128,
            external_assets: self.external_assets as u128,
            fee_anchor: FeeAccrualAnchor::new(
                self.fee_anchor_assets as u128,
                self.fee_anchor_ns,
            ),
            op_state,
            withdraw_queue: WithdrawQueue::new(),
            next_op_id: self.next_op_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Soroban Storage Implementation
// ---------------------------------------------------------------------------

/// Soroban ledger storage implementation.
///
/// Uses the Soroban SDK's persistent storage to store vault state
/// directly on the blockchain ledger.
pub struct SorobanStorage<'a> {
    env: &'a Env,
}

impl<'a> SorobanStorage<'a> {
    /// Create a new Soroban storage instance.
    #[inline]
    #[must_use]
    pub fn new(env: &'a Env) -> Self {
        Self { env }
    }

    /// Load the vault state from persistent storage.
    pub fn load_vault_state(&self) -> Option<SorobanVaultState> {
        self.env
            .storage()
            .persistent()
            .get(&SorobanStorageKey::VaultState)
    }

    /// Save the vault state to persistent storage.
    pub fn save_vault_state(&self, state: &SorobanVaultState) {
        self.env
            .storage()
            .persistent()
            .set(&SorobanStorageKey::VaultState, state);
    }

    /// Get the storage version.
    pub fn get_version(&self) -> Option<u32> {
        self.env
            .storage()
            .persistent()
            .get(&SorobanStorageKey::Version)
    }

    /// Set the storage version.
    pub fn set_version(&self, version: u32) {
        self.env
            .storage()
            .persistent()
            .set(&SorobanStorageKey::Version, &version);
    }

    /// Check if the contract is paused.
    pub fn is_paused(&self) -> bool {
        self.env
            .storage()
            .instance()
            .get(&SorobanStorageKey::Paused)
            .unwrap_or(false)
    }

    /// Set the pause state.
    pub fn set_paused(&self, paused: bool) {
        self.env
            .storage()
            .instance()
            .set(&SorobanStorageKey::Paused, &paused);
    }

    /// Check if storage has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::VaultState)
    }

    /// Extend the TTL of persistent storage entries.
    ///
    /// Call this periodically to prevent state from expiring.
    pub fn extend_ttl(&self, threshold: u32, extend_to: u32) {
        self.env.storage().persistent().extend_ttl(
            &SorobanStorageKey::VaultState,
            threshold,
            extend_to,
        );
        self.env.storage().persistent().extend_ttl(
            &SorobanStorageKey::Version,
            threshold,
            extend_to,
        );
    }
}

impl Storage for SorobanStorage<'_> {
    fn load_state(&self) -> Result<Option<VersionedState>, RuntimeError> {
        match self.load_vault_state() {
            Some(soroban_state) => {
                let version = self.get_version().unwrap_or(1);
                Ok(Some(VersionedState {
                    version: StorageVersion::new(version),
                    state: soroban_state.to_kernel(),
                }))
            }
            None => Ok(None),
        }
    }

    fn save_state(&mut self, state: &VersionedState) -> Result<(), RuntimeError> {
        let soroban_state = SorobanVaultState::from_kernel(&state.state);
        self.save_vault_state(&soroban_state);
        self.set_version(state.version.number());
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        SorobanStorage::is_initialized(self)
    }

    fn get_version(&self) -> Result<StorageVersion, RuntimeError> {
        SorobanStorage::get_version(self)
            .map(StorageVersion::new)
            .ok_or_else(|| RuntimeError::storage_error("version not initialized"))
    }
}

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

    #[test]
    fn test_soroban_vault_state_default() {
        let state = SorobanVaultState::default();
        assert_eq!(state.total_assets, 0);
        assert_eq!(state.total_shares, 0);
        assert_eq!(state.idle_assets, 0);
        assert_eq!(state.external_assets, 0);
        assert_eq!(state.op_state_kind, 0);
        assert_eq!(state.next_op_id, 0);
    }

    #[test]
    fn test_soroban_vault_state_from_kernel() {
        let mut kernel_state = VaultState::default();
        kernel_state.total_assets = 1000;
        kernel_state.total_shares = 500;
        kernel_state.idle_assets = 300;
        kernel_state.external_assets = 700;
        kernel_state.next_op_id = 42;

        let soroban_state = SorobanVaultState::from_kernel(&kernel_state);
        assert_eq!(soroban_state.total_assets, 1000);
        assert_eq!(soroban_state.total_shares, 500);
        assert_eq!(soroban_state.idle_assets, 300);
        assert_eq!(soroban_state.external_assets, 700);
        assert_eq!(soroban_state.next_op_id, 42);
        assert_eq!(soroban_state.op_state_kind, 0); // Idle
    }

    #[test]
    fn test_soroban_vault_state_roundtrip() {
        let mut kernel_state = VaultState::default();
        kernel_state.total_assets = 5000;
        kernel_state.total_shares = 2500;
        kernel_state.idle_assets = 1000;
        kernel_state.external_assets = 4000;
        kernel_state.next_op_id = 100;

        let soroban_state = SorobanVaultState::from_kernel(&kernel_state);
        let restored = soroban_state.to_kernel();

        assert_eq!(restored.total_assets, kernel_state.total_assets);
        assert_eq!(restored.total_shares, kernel_state.total_shares);
        assert_eq!(restored.idle_assets, kernel_state.idle_assets);
        assert_eq!(restored.external_assets, kernel_state.external_assets);
        assert_eq!(restored.next_op_id, kernel_state.next_op_id);
    }

    #[test]
    fn test_soroban_storage_key_variants() {
        let key1 = SorobanStorageKey::VaultState;
        let key2 = SorobanStorageKey::Version;
        let key3 = SorobanStorageKey::Config;
        let key4 = SorobanStorageKey::Paused;

        // Keys should be distinct
        assert!(matches!(key1, SorobanStorageKey::VaultState));
        assert!(matches!(key2, SorobanStorageKey::Version));
        assert!(matches!(key3, SorobanStorageKey::Config));
        assert!(matches!(key4, SorobanStorageKey::Paused));
    }

    // Helper to create a registered contract for storage tests
    mod test_contract {
        use soroban_sdk::{contract, contractimpl, Env};

        #[contract]
        pub struct TestContract;

        #[contractimpl]
        impl TestContract {
            pub fn noop(_env: Env) {}
        }
    }

    #[test]
    fn test_soroban_storage_with_sdk_env() {
        // Test using real Soroban SDK Env (testutils)
        let env = Env::default();
        let contract_id = env.register(test_contract::TestContract, ());

        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);

            // Fresh storage should not be initialized
            assert!(!storage.is_initialized());
            assert!(storage.get_version().is_none());
            assert!(storage.load_vault_state().is_none());

            // Save state
            let state = SorobanVaultState {
                total_assets: 10000,
                total_shares: 5000,
                idle_assets: 2000,
                external_assets: 8000,
                fee_anchor_ns: 12345,
                fee_anchor_assets: 9000,
                op_state_kind: 0,
                op_state_id: 0,
                withdraw_queue_len: 0,
                next_op_id: 1,
            };
            storage.save_vault_state(&state);
            storage.set_version(1);

            // Now storage should be initialized
            assert!(storage.is_initialized());
            assert_eq!(storage.get_version(), Some(1));

            // Load and verify
            let loaded = storage.load_vault_state().unwrap();
            assert_eq!(loaded.total_assets, 10000);
            assert_eq!(loaded.total_shares, 5000);
            assert_eq!(loaded.idle_assets, 2000);
            assert_eq!(loaded.external_assets, 8000);
            assert_eq!(loaded.next_op_id, 1);
        });
    }

    #[test]
    fn test_soroban_storage_pause_state() {
        let env = Env::default();
        let contract_id = env.register(test_contract::TestContract, ());

        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);

            // Default is not paused
            assert!(!storage.is_paused());

            // Set paused
            storage.set_paused(true);
            assert!(storage.is_paused());

            // Unset paused
            storage.set_paused(false);
            assert!(!storage.is_paused());
        });
    }

    #[test]
    fn test_soroban_storage_implements_storage_trait() {
        let env = Env::default();
        let contract_id = env.register(test_contract::TestContract, ());

        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);

            // Test Storage trait methods
            assert!(!Storage::is_initialized(&storage));
            assert!(storage.load_state().unwrap().is_none());

            // Save state via trait
            let versioned = VersionedState::default();
            storage.save_state(&versioned).unwrap();

            // Verify via trait
            assert!(Storage::is_initialized(&storage));
            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded.version, StorageVersion::CURRENT);
        });
    }
}
