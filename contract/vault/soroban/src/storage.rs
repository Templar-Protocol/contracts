//! Storage versioning and persistence for Soroban ledger.
//!
//! This module provides versioned storage wrappers for persisting vault state
//! to the Soroban ledger. It handles schema migrations and forward compatibility.

use alloc::{collections::BTreeMap, vec::Vec};
use derive_more::{From, Into};
use soroban_sdk::{contracttype, Address as SdkAddress, Bytes, BytesN, Env};
use templar_curator_primitives::PolicyState;
use templar_vault_kernel::{Address, Restrictions, VaultState};

use crate::error::RuntimeError;

/// Re-extend TTL when remaining TTL drops below ~30 days (at ~5s/ledger).
pub(crate) const DEFAULT_TTL_THRESHOLD: u32 = 518_400;
/// Extend TTL to the Soroban maximum (~6 months at ~5s/ledger).
/// For a vault contract holding real assets, maximum TTL prevents state
/// loss during extended pauses or periods of inactivity.
pub(crate) const DEFAULT_TTL_EXTEND_TO: u32 = 3_110_400;

/// Storage keys for Soroban ledger storage.
///
/// Using `#[contracttype]` allows the key enum to be used with Soroban's
/// native storage API.
#[contracttype]
#[derive(Clone, Debug)]
pub enum SorobanStorageKey {
    StateBlob,
    /// Policy state (locks, caps, supply queue).
    PolicyState,
    /// Kernel restrictions (pause/allowlist/denylist).
    Restrictions,
    /// Address book entry mapping kernel address to Soroban address.
    AddressBook(BytesN<32>),
    /// Storage version number.
    Version,
    /// Contract configuration.
    Config,
    /// Pause flag.
    Paused,
}

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

    fn address_key(&self, kernel_addr: &Address) -> SorobanStorageKey {
        SorobanStorageKey::AddressBook(BytesN::from_array(self.env, kernel_addr))
    }

    /// Load a kernel-to-Soroban address mapping from persistent storage.
    pub fn load_address(&self, kernel_addr: &Address) -> Option<SdkAddress> {
        let key = self.address_key(kernel_addr);
        self.env.storage().persistent().get(&key)
    }

    /// Save a kernel-to-Soroban address mapping to persistent storage.
    pub fn save_address(&self, kernel_addr: &Address, soroban_addr: &SdkAddress) {
        let key = self.address_key(kernel_addr);
        self.env.storage().persistent().set(&key, soroban_addr);
        self.env.storage().persistent().extend_ttl(
            &key,
            DEFAULT_TTL_THRESHOLD,
            DEFAULT_TTL_EXTEND_TO,
        );
        self.extend_default_ttl();
    }

    fn load_state_blob(&self) -> Option<Vec<u8>> {
        if !self
            .env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::StateBlob)
        {
            return None;
        }
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(&SorobanStorageKey::StateBlob)
            .map(|b| b.to_alloc_vec())
    }

    fn save_state_blob(&self, state: &Vec<u8>) {
        self.env.storage().persistent().set(
            &SorobanStorageKey::StateBlob,
            &Bytes::from_slice(self.env, state),
        );
    }

    /// Load the policy state from persistent storage.
    pub fn load_policy_state(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(&SorobanStorageKey::PolicyState)
            .map(|b| b.to_alloc_vec())
    }

    /// Save the policy state to persistent storage.
    pub fn save_policy_state(&self, state: &Vec<u8>) {
        self.env.storage().persistent().set(
            &SorobanStorageKey::PolicyState,
            &Bytes::from_slice(self.env, state),
        );
    }

    /// Load restrictions from persistent storage.
    pub fn load_restrictions(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(&SorobanStorageKey::Restrictions)
            .map(|b| b.to_alloc_vec())
    }

    /// Save restrictions to persistent storage.
    pub fn save_restrictions(&self, restrictions: &Vec<u8>) {
        self.env.storage().persistent().set(
            &SorobanStorageKey::Restrictions,
            &Bytes::from_slice(self.env, restrictions),
        );
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
    ///
    /// Uses OpenZeppelin's Pausable storage for compatibility.
    pub fn is_paused(&self) -> bool {
        stellar_contract_utils::pausable::paused(self.env)
    }

    /// Set the pause state.
    ///
    /// Uses a storage key compatible with OpenZeppelin's Pausable module.
    /// The key must match OZ's `PausableStorageKey::Paused` for interoperability.
    pub fn set_paused(&self, paused: bool) {
        // OZ PausableStorageKey is: #[contracttype] enum PausableStorageKey { Paused }
        // We define a compatible key here since OZ's storage module is private.
        #[soroban_sdk::contracttype]
        enum OzPausableKey {
            Paused,
        }
        self.env
            .storage()
            .instance()
            .set(&OzPausableKey::Paused, &paused);
    }

    /// Check if the contract has the legacy pause key (for migration).
    pub fn has_legacy_paused(&self) -> bool {
        self.env
            .storage()
            .instance()
            .has(&SorobanStorageKey::Paused)
    }

    /// Get the legacy pause value and remove it (for migration).
    pub fn take_legacy_paused(&self) -> Option<bool> {
        if self.has_legacy_paused() {
            let paused: bool = self
                .env
                .storage()
                .instance()
                .get(&SorobanStorageKey::Paused)
                .unwrap_or(false);
            self.env
                .storage()
                .instance()
                .remove(&SorobanStorageKey::Paused);
            Some(paused)
        } else {
            None
        }
    }

    /// Check if storage has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::StateBlob)
    }

    /// Extend the TTL of storage entries.
    ///
    /// Call this periodically to prevent state from expiring.
    pub fn extend_ttl(&self, threshold: u32, extend_to: u32) {
        self.env
            .storage()
            .instance()
            .extend_ttl(threshold, extend_to);
        if self
            .env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::StateBlob)
        {
            self.env.storage().persistent().extend_ttl(
                &SorobanStorageKey::StateBlob,
                threshold,
                extend_to,
            );
        }
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
        if let Some(stored) = self.load_state_blob() {
            let versioned =
                borsh_deserialize::<VersionedState>(&stored, "state blob deserialize failed")?;

            let version = SorobanStorage::get_version(self)
                .ok_or_else(|| RuntimeError::storage_error("state version missing"))?;
            let stored_version = StorageVersion::new(version);

            if versioned.version != stored_version {
                return Err(RuntimeError::storage_error("state version mismatch"));
            }

            if !versioned.version.is_compatible() {
                return Err(RuntimeError::storage_error("unsupported state version"));
            }

            return Ok(Some(versioned));
        }

        Ok(None)
    }

    fn save_state(&mut self, state: &VersionedState) -> Result<(), RuntimeError> {
        let state_blob = borsh_serialize(state, "state blob serialize failed")?;
        self.save_state_blob(&state_blob);
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

    fn load_address(&self, kernel_addr: &Address) -> Result<Option<SdkAddress>, RuntimeError> {
        Ok(SorobanStorage::load_address(self, kernel_addr))
    }

    fn save_address(
        &mut self,
        kernel_addr: &Address,
        soroban_addr: &SdkAddress,
    ) -> Result<(), RuntimeError> {
        SorobanStorage::save_address(self, kernel_addr, soroban_addr);
        Ok(())
    }
}

/// Storage version identifier.
#[derive(
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    From,
    Into,
)]
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
#[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Clone, Debug, PartialEq, Eq)]
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
    fn save_restrictions(
        &mut self,
        restrictions: &Option<Restrictions>,
    ) -> Result<(), RuntimeError>;

    /// Load a kernel-to-Soroban address mapping.
    fn load_address(&self, kernel_addr: &Address) -> Result<Option<SdkAddress>, RuntimeError>;

    /// Persist a kernel-to-Soroban address mapping.
    fn save_address(
        &mut self,
        kernel_addr: &Address,
        soroban_addr: &SdkAddress,
    ) -> Result<(), RuntimeError>;
}

/// In-memory storage implementation for testing.
#[derive(Clone, Debug, Default)]
pub struct MemoryStorage {
    state: Option<VersionedState>,
    initialized: bool,
    paused: bool,
    policy_state: Option<PolicyState>,
    restrictions: Option<Restrictions>,
    address_book: BTreeMap<Address, SdkAddress>,
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
            address_book: BTreeMap::new(),
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
        self.address_book.clear();
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

    fn load_address(&self, kernel_addr: &Address) -> Result<Option<SdkAddress>, RuntimeError> {
        Ok(self.address_book.get(kernel_addr).cloned())
    }

    fn save_address(
        &mut self,
        kernel_addr: &Address,
        soroban_addr: &SdkAddress,
    ) -> Result<(), RuntimeError> {
        self.address_book.insert(*kernel_addr, soroban_addr.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests;
