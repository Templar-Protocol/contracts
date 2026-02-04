//! Storage versioning and persistence for Soroban ledger.
//!
//! This module provides versioned storage wrappers for persisting vault state
//! to the Soroban ledger. It handles schema migrations and forward compatibility.

use alloc::vec::Vec;
use derive_more::{From, Into};
use soroban_sdk::{contracttype, Env};
use templar_curator_primitives::PolicyState;
use templar_vault_kernel::{Restrictions, VaultState};

use crate::error::RuntimeError;

const DEFAULT_TTL_THRESHOLD: u32 = 50_000;
const DEFAULT_TTL_EXTEND_TO: u32 = 100_000;

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
    /// Persisted op_state payload.
    OpState,
    /// Persisted withdrawal queue.
    WithdrawQueue,
    /// Policy state (locks, caps, supply queue).
    PolicyState,
    /// Kernel restrictions (pause/allowlist/denylist).
    Restrictions,
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
    pub fn from_kernel(state: &VaultState) -> Result<Self, RuntimeError> {
        use templar_vault_kernel::OpState;

        let (op_state_kind, op_state_id) = match &state.op_state {
            OpState::Idle => (0, 0),
            OpState::Allocating(s) => (1, s.op_id),
            OpState::Withdrawing(s) => (2, s.op_id),
            OpState::Refreshing(s) => (3, s.op_id),
            OpState::Payout(s) => (4, s.op_id),
        };

        Ok(Self {
            total_assets: i128::try_from(state.total_assets)
                .map_err(|_| RuntimeError::storage_error("total_assets exceeds i128"))?,
            total_shares: i128::try_from(state.total_shares)
                .map_err(|_| RuntimeError::storage_error("total_shares exceeds i128"))?,
            idle_assets: i128::try_from(state.idle_assets)
                .map_err(|_| RuntimeError::storage_error("idle_assets exceeds i128"))?,
            external_assets: i128::try_from(state.external_assets)
                .map_err(|_| RuntimeError::storage_error("external_assets exceeds i128"))?,
            fee_anchor_ns: state.fee_anchor.timestamp_ns,
            fee_anchor_assets: i128::try_from(state.fee_anchor.total_assets)
                .map_err(|_| RuntimeError::storage_error("fee_anchor_assets exceeds i128"))?,
            op_state_kind,
            op_state_id,
            withdraw_queue_len: state.withdraw_queue.len() as u32,
            next_op_id: state.next_op_id,
        })
    }

    /// Convert to kernel VaultState.
    ///
    /// Note: This creates a base VaultState without op_state/queue details.
    /// Full op_state and withdraw queue must be loaded separately.
    pub fn to_kernel(&self) -> Result<VaultState, RuntimeError> {
        use templar_vault_kernel::{
            FeeAccrualAnchor, OpState, WithdrawQueue,
        };

        let op_state = OpState::Idle;
        let withdraw_queue = WithdrawQueue::new();

        Ok(VaultState {
            total_assets: u128::try_from(self.total_assets)
                .map_err(|_| RuntimeError::storage_error("total_assets is negative"))?,
            total_shares: u128::try_from(self.total_shares)
                .map_err(|_| RuntimeError::storage_error("total_shares is negative"))?,
            idle_assets: u128::try_from(self.idle_assets)
                .map_err(|_| RuntimeError::storage_error("idle_assets is negative"))?,
            external_assets: u128::try_from(self.external_assets)
                .map_err(|_| RuntimeError::storage_error("external_assets is negative"))?,
            fee_anchor: FeeAccrualAnchor::new(
                u128::try_from(self.fee_anchor_assets)
                    .map_err(|_| RuntimeError::storage_error("fee_anchor_assets is negative"))?,
                self.fee_anchor_ns,
            ),
            op_state,
            withdraw_queue,
            next_op_id: self.next_op_id,
        })
    }

}

// ---------------------------------------------------------------------------
// Borsh helpers for full op_state + queue persistence
// ---------------------------------------------------------------------------

fn borsh_serialize<T: borsh::BorshSerialize>(
    value: &T,
    msg: &'static str,
) -> Result<Vec<u8>, RuntimeError> {
    borsh::to_vec(value).map_err(|_| RuntimeError::storage_error(msg))
}

fn borsh_deserialize<T: borsh::BorshDeserialize>(
    bytes: &[u8],
    msg: &'static str,
) -> Result<T, RuntimeError> {
    T::try_from_slice(bytes).map_err(|_| RuntimeError::storage_error(msg))
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

    /// Load the op_state from persistent storage.
    pub fn load_op_state(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get(&SorobanStorageKey::OpState)
    }

    /// Save the op_state to persistent storage.
    pub fn save_op_state(&self, state: &Vec<u8>) {
        self.env
            .storage()
            .persistent()
            .set(&SorobanStorageKey::OpState, state);
    }

    /// Load the withdrawal queue from persistent storage.
    pub fn load_withdraw_queue(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get(&SorobanStorageKey::WithdrawQueue)
    }

    /// Save the withdrawal queue to persistent storage.
    pub fn save_withdraw_queue(&self, queue: &Vec<u8>) {
        self.env
            .storage()
            .persistent()
            .set(&SorobanStorageKey::WithdrawQueue, queue);
    }

    /// Load the policy state from persistent storage.
    pub fn load_policy_state(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get(&SorobanStorageKey::PolicyState)
    }

    /// Save the policy state to persistent storage.
    pub fn save_policy_state(&self, state: &Vec<u8>) {
        self.env
            .storage()
            .persistent()
            .set(&SorobanStorageKey::PolicyState, state);
    }

    /// Load restrictions from persistent storage.
    pub fn load_restrictions(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get(&SorobanStorageKey::Restrictions)
    }

    /// Save restrictions to persistent storage.
    pub fn save_restrictions(&self, restrictions: &Vec<u8>) {
        self.env
            .storage()
            .persistent()
            .set(&SorobanStorageKey::Restrictions, restrictions);
    }

    /// Clear restrictions from persistent storage.
    pub fn clear_restrictions(&self) {
        self.env
            .storage()
            .persistent()
            .remove(&SorobanStorageKey::Restrictions);
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

    /// Extend the TTL of storage entries.
    ///
    /// Call this periodically to prevent state from expiring.
    pub fn extend_ttl(&self, threshold: u32, extend_to: u32) {
        self.env.storage().instance().extend_ttl(threshold, extend_to);
        self.env.storage().persistent().extend_ttl(
            &SorobanStorageKey::VaultState,
            threshold,
            extend_to,
        );
        self.env.storage().persistent().extend_ttl(
            &SorobanStorageKey::OpState,
            threshold,
            extend_to,
        );
        self.env.storage().persistent().extend_ttl(
            &SorobanStorageKey::WithdrawQueue,
            threshold,
            extend_to,
        );
        if self
            .env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::PolicyState)
        {
            self.env.storage().persistent().extend_ttl(
                &SorobanStorageKey::PolicyState,
                threshold,
                extend_to,
            );
        }
        if self
            .env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::Restrictions)
        {
            self.env.storage().persistent().extend_ttl(
                &SorobanStorageKey::Restrictions,
                threshold,
                extend_to,
            );
        }
        self.env.storage().persistent().extend_ttl(
            &SorobanStorageKey::Version,
            threshold,
            extend_to,
        );
    }

    fn extend_default_ttl(&self) {
        self.extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
    }
}

impl Storage for SorobanStorage<'_> {
    fn load_state(&self) -> Result<Option<VersionedState>, RuntimeError> {
        match self.load_vault_state() {
            Some(soroban_state) => {
                use templar_vault_kernel::{OpState, WithdrawQueue};

                let op_state = match self.load_op_state() {
                    Some(stored) => Some(borsh_deserialize::<OpState>(
                        &stored,
                        "op_state deserialize failed",
                    )?),
                    None => None,
                };
                let withdraw_queue = match self.load_withdraw_queue() {
                    Some(stored) => Some(borsh_deserialize::<WithdrawQueue>(
                        &stored,
                        "withdraw queue deserialize failed",
                    )?),
                    None => None,
                };

                if op_state.is_none()
                    && (soroban_state.op_state_kind != 0 || soroban_state.op_state_id != 0)
                {
                    return Err(RuntimeError::storage_error(
                        "op_state missing for non-idle state",
                    ));
                }

                if withdraw_queue.is_none() && soroban_state.withdraw_queue_len != 0 {
                    return Err(RuntimeError::storage_error(
                        "withdraw queue missing for pending withdrawals",
                    ));
                }

                let version = self.get_version().unwrap_or(1);
                let mut state = soroban_state.to_kernel()?;
                state.op_state = op_state.unwrap_or(OpState::Idle);
                state.withdraw_queue = withdraw_queue.unwrap_or_else(WithdrawQueue::new);
                let mut versioned = VersionedState {
                    version: StorageVersion::new(version),
                    state,
                };

                if versioned.needs_migration() {
                    versioned = Migrator::migrate(versioned)?;
                    let mut storage = SorobanStorage::new(self.env);
                    Storage::save_state(&mut storage, &versioned)?;
                }

                Ok(Some(versioned))
            }
            None => Ok(None),
        }
    }

    fn save_state(&mut self, state: &VersionedState) -> Result<(), RuntimeError> {
        let soroban_state = SorobanVaultState::from_kernel(&state.state)?;
        self.save_vault_state(&soroban_state);
        let op_state =
            borsh_serialize(&state.state.op_state, "op_state serialize failed")?;
        let withdraw_queue = borsh_serialize(
            &state.state.withdraw_queue,
            "withdraw queue serialize failed",
        )?;
        self.save_op_state(&op_state);
        self.save_withdraw_queue(&withdraw_queue);
        self.set_version(state.version.number());
        self.extend_default_ttl();
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

    fn load_paused(&self) -> Result<bool, RuntimeError> {
        Ok(self.is_paused())
    }

    fn save_paused(&mut self, paused: bool) -> Result<(), RuntimeError> {
        self.set_paused(paused);
        self.extend_default_ttl();
        Ok(())
    }

    fn load_policy_state(&self) -> Result<Option<PolicyState>, RuntimeError> {
        match SorobanStorage::load_policy_state(self) {
            Some(stored) => Ok(Some(borsh_deserialize::<PolicyState>(
                &stored,
                "policy_state deserialize failed",
            )?)),
            None => Ok(None),
        }
    }

    fn save_policy_state(&mut self, state: &PolicyState) -> Result<(), RuntimeError> {
        let bytes = borsh_serialize(state, "policy_state serialize failed")?;
        SorobanStorage::save_policy_state(self, &bytes);
        self.extend_default_ttl();
        Ok(())
    }

    fn load_restrictions(&self) -> Result<Option<Restrictions>, RuntimeError> {
        match SorobanStorage::load_restrictions(self) {
            Some(stored) => Ok(Some(borsh_deserialize::<Restrictions>(
                &stored,
                "restrictions deserialize failed",
            )?)),
            None => Ok(None),
        }
    }

    fn save_restrictions(
        &mut self,
        restrictions: &Option<Restrictions>,
    ) -> Result<(), RuntimeError> {
        if let Some(restrictions) = restrictions {
            let bytes = borsh_serialize(restrictions, "restrictions serialize failed")?;
            SorobanStorage::save_restrictions(self, &bytes);
        } else {
            SorobanStorage::clear_restrictions(self);
        }
        self.extend_default_ttl();
        Ok(())
    }
}

/// Storage version identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, From, Into)]
pub struct StorageVersion(pub u32);

impl StorageVersion {
    /// Initial storage version.
    pub const V1: Self = Self(1);

    /// OpState + withdraw queue persistence.
    pub const V2: Self = Self(2);

    /// Current storage version.
    pub const CURRENT: Self = Self::V2;

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

    /// Load the paused flag for the vault.
    fn load_paused(&self) -> Result<bool, RuntimeError>;

    /// Persist the paused flag for the vault.
    fn save_paused(&mut self, paused: bool) -> Result<(), RuntimeError>;

    /// Load the policy state for the vault.
    fn load_policy_state(&self) -> Result<Option<PolicyState>, RuntimeError>;

    /// Persist the policy state for the vault.
    fn save_policy_state(&mut self, state: &PolicyState) -> Result<(), RuntimeError>;

    /// Load kernel restrictions for the vault.
    fn load_restrictions(&self) -> Result<Option<Restrictions>, RuntimeError>;

    /// Persist kernel restrictions for the vault.
    fn save_restrictions(&mut self, restrictions: &Option<Restrictions>)
        -> Result<(), RuntimeError>;
}

/// In-memory storage implementation for testing.
#[derive(Clone, Debug, Default)]
pub struct MemoryStorage {
    state: Option<VersionedState>,
    initialized: bool,
    paused: bool,
    policy_state: Option<PolicyState>,
    restrictions: Option<Restrictions>,
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
            paused: false,
            policy_state: None,
            restrictions: None,
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
        self.policy_state = None;
        self.restrictions = None;
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

    fn load_paused(&self) -> Result<bool, RuntimeError> {
        Ok(self.paused)
    }

    fn save_paused(&mut self, paused: bool) -> Result<(), RuntimeError> {
        self.paused = paused;
        Ok(())
    }

    fn load_policy_state(&self) -> Result<Option<PolicyState>, RuntimeError> {
        Ok(self.policy_state.clone())
    }

    fn save_policy_state(&mut self, state: &PolicyState) -> Result<(), RuntimeError> {
        self.policy_state = Some(state.clone());
        Ok(())
    }

    fn load_restrictions(&self) -> Result<Option<Restrictions>, RuntimeError> {
        Ok(self.restrictions.clone())
    }

    fn save_restrictions(
        &mut self,
        restrictions: &Option<Restrictions>,
    ) -> Result<(), RuntimeError> {
        self.restrictions = restrictions.clone();
        Ok(())
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
        // V1 -> V2 adds op_state + withdraw queue persistence.
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
        assert_eq!(current, StorageVersion::V2);
    }

    #[test]
    fn test_storage_version_compatibility() {
        let old = StorageVersion::new(1);
        assert!(old.is_compatible());

        let v2 = StorageVersion::new(2);
        assert!(v2.is_compatible());

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

        // V1 is older than current, so migration needed
        assert!(versioned.needs_migration());
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

        let soroban_state = SorobanVaultState::from_kernel(&kernel_state).unwrap();
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

        let soroban_state = SorobanVaultState::from_kernel(&kernel_state).unwrap();
        let restored = soroban_state.to_kernel().unwrap();

        assert_eq!(restored.total_assets, kernel_state.total_assets);
        assert_eq!(restored.total_shares, kernel_state.total_shares);
        assert_eq!(restored.idle_assets, kernel_state.idle_assets);
        assert_eq!(restored.external_assets, kernel_state.external_assets);
        assert_eq!(restored.next_op_id, kernel_state.next_op_id);
    }

    #[test]
    fn test_soroban_storage_key_variants() {
        let key1 = SorobanStorageKey::VaultState;
        let key2 = SorobanStorageKey::OpState;
        let key3 = SorobanStorageKey::WithdrawQueue;
        let key4 = SorobanStorageKey::PolicyState;
        let key5 = SorobanStorageKey::Restrictions;
        let key6 = SorobanStorageKey::Version;
        let key7 = SorobanStorageKey::Config;
        let key8 = SorobanStorageKey::Paused;

        // Keys should be distinct
        assert!(matches!(key1, SorobanStorageKey::VaultState));
        assert!(matches!(key2, SorobanStorageKey::OpState));
        assert!(matches!(key3, SorobanStorageKey::WithdrawQueue));
        assert!(matches!(key4, SorobanStorageKey::PolicyState));
        assert!(matches!(key5, SorobanStorageKey::Restrictions));
        assert!(matches!(key6, SorobanStorageKey::Version));
        assert!(matches!(key7, SorobanStorageKey::Config));
        assert!(matches!(key8, SorobanStorageKey::Paused));
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
    fn test_soroban_storage_rejects_missing_op_state_or_queue() {
        let env = Env::default();
        let contract_id = env.register(test_contract::TestContract, ());

        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);

            let state = SorobanVaultState {
                op_state_kind: 2,
                op_state_id: 7,
                withdraw_queue_len: 1,
                ..SorobanVaultState::default()
            };
            storage.save_vault_state(&state);
            storage.set_version(1);

            let result = storage.load_state();
            assert!(matches!(result, Err(RuntimeError::StorageError(_))));
        });
    }

    #[test]
    fn test_soroban_storage_roundtrip_op_state_and_queue() {
        use alloc::collections::BTreeMap;
        use templar_vault_kernel::state::queue::{PendingWithdrawal, WithdrawQueue};
        use templar_vault_kernel::{OpState, WithdrawingState};

        let env = Env::default();
        let contract_id = env.register(test_contract::TestContract, ());

        env.as_contract(&contract_id, || {
            let mut storage = SorobanStorage::new(&env);
            let mut state = VaultState::default();

            let owner = [1u8; 32];
            let receiver = [2u8; 32];
            state.op_state = OpState::Withdrawing(WithdrawingState {
                op_id: 7,
                index: 1,
                remaining: 500,
                collected: 200,
                receiver,
                owner,
                escrow_shares: 700,
            });

            let mut pending = BTreeMap::new();
            pending.insert(
                3,
                PendingWithdrawal::new(owner, receiver, 700, 800, 123),
            );
            state.withdraw_queue = WithdrawQueue::with_state(pending, 3, 4);
            state.total_assets = 1000;
            state.total_shares = 900;
            state.idle_assets = 100;
            state.external_assets = 900;
            state.next_op_id = 8;

            let versioned = VersionedState::new(state.clone());
            storage.save_state(&versioned).unwrap();

            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded.state, state);
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

    #[test]
    fn test_soroban_storage_migrates_v1_state() {
        let env = Env::default();
        let contract_id = env.register(test_contract::TestContract, ());

        env.as_contract(&contract_id, || {
            let storage = SorobanStorage::new(&env);
            let state = SorobanVaultState::default();
            storage.save_vault_state(&state);
            storage.set_version(1);

            let loaded = storage.load_state().unwrap().unwrap();
            assert_eq!(loaded.version, StorageVersion::CURRENT);
            assert!(loaded.state.op_state.is_idle());
            assert!(loaded.state.withdraw_queue.is_empty());
            assert_eq!(
                Storage::get_version(&storage).unwrap(),
                StorageVersion::CURRENT
            );
        });
    }
}
