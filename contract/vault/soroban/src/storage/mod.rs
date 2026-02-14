//! Storage versioning and persistence for Soroban ledger.
//!
//! This module provides versioned storage wrappers for persisting vault state
//! to the Soroban ledger. It handles schema migrations and forward compatibility.

use alloc::vec::Vec;
use derive_more::{From, Into};
use soroban_sdk::{contracttype, Address as SdkAddress, Bytes, BytesN, Env};
use templar_curator_primitives::policy::cap_group::{CapGroupId, CapGroupRecord};
use templar_curator_primitives::policy::market_lock::MarketLockSet;
use templar_curator_primitives::policy::state::{MarketConfig, OrderedMap};
use templar_curator_primitives::policy::supply_queue::SupplyQueue;
use templar_curator_primitives::PolicyState;
use templar_vault_kernel::{Address, AddressBook, Restrictions, TargetId, VaultState};

use crate::error::RuntimeError;

/// Re-extend TTL when remaining TTL drops below ~30 days (at ~5s/ledger).
pub(crate) const DEFAULT_TTL_THRESHOLD: u32 = 518_400;
/// Extend TTL to the Soroban maximum (~6 months at ~5s/ledger).
/// For a vault contract holding real assets, maximum TTL prevents state
/// loss during extended pauses or periods of inactivity.
pub(crate) const DEFAULT_TTL_EXTEND_TO: u32 = 3_110_400;

#[contracttype]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone)]
pub enum SorobanStorageKey {
    StateBlob,
    PolicyLocks,
    PolicySupplyQueue,
    PolicyMarkets,
    PolicyPrincipals,
    PolicyCapGroups,
    Restrictions,
    AddressBook(BytesN<32>),
    Version,
    Config,
    Paused,
}

fn pc_serialize<T: serde::Serialize>(
    value: &T,
    msg: &'static str,
) -> Result<Vec<u8>, RuntimeError> {
    match postcard::to_allocvec(value) {
        Ok(bytes) => Ok(bytes),
        Err(_) => Err(RuntimeError::storage_error(msg)),
    }
}

fn pc_deserialize<'a, T: serde::Deserialize<'a>>(
    bytes: &'a [u8],
    msg: &'static str,
) -> Result<T, RuntimeError> {
    match postcard::from_bytes(bytes) {
        Ok(value) => Ok(value),
        Err(_) => Err(RuntimeError::storage_error(msg)),
    }
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

    pub fn load_policy_locks(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(&SorobanStorageKey::PolicyLocks)
            .map(|b| b.to_alloc_vec())
    }

    pub fn save_policy_locks(&self, state: &Vec<u8>) {
        self.env.storage().persistent().set(
            &SorobanStorageKey::PolicyLocks,
            &Bytes::from_slice(self.env, state),
        );
    }

    pub fn load_policy_supply_queue(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(&SorobanStorageKey::PolicySupplyQueue)
            .map(|b| b.to_alloc_vec())
    }

    pub fn save_policy_supply_queue(&self, state: &Vec<u8>) {
        self.env.storage().persistent().set(
            &SorobanStorageKey::PolicySupplyQueue,
            &Bytes::from_slice(self.env, state),
        );
    }

    pub fn load_policy_markets(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(&SorobanStorageKey::PolicyMarkets)
            .map(|b| b.to_alloc_vec())
    }

    pub fn save_policy_markets(&self, state: &Vec<u8>) {
        self.env.storage().persistent().set(
            &SorobanStorageKey::PolicyMarkets,
            &Bytes::from_slice(self.env, state),
        );
    }

    pub fn load_policy_principals(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(&SorobanStorageKey::PolicyPrincipals)
            .map(|b| b.to_alloc_vec())
    }

    pub fn save_policy_principals(&self, state: &Vec<u8>) {
        self.env.storage().persistent().set(
            &SorobanStorageKey::PolicyPrincipals,
            &Bytes::from_slice(self.env, state),
        );
    }

    pub fn load_policy_cap_groups(&self) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(&SorobanStorageKey::PolicyCapGroups)
            .map(|b| b.to_alloc_vec())
    }

    pub fn save_policy_cap_groups(&self, state: &Vec<u8>) {
        self.env.storage().persistent().set(
            &SorobanStorageKey::PolicyCapGroups,
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
            .has(&SorobanStorageKey::PolicyLocks)
        {
            self.env.storage().persistent().extend_ttl(
                &SorobanStorageKey::PolicyLocks,
                threshold,
                extend_to,
            );
        }
        if self
            .env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::PolicySupplyQueue)
        {
            self.env.storage().persistent().extend_ttl(
                &SorobanStorageKey::PolicySupplyQueue,
                threshold,
                extend_to,
            );
        }
        if self
            .env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::PolicyMarkets)
        {
            self.env.storage().persistent().extend_ttl(
                &SorobanStorageKey::PolicyMarkets,
                threshold,
                extend_to,
            );
        }
        if self
            .env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::PolicyPrincipals)
        {
            self.env.storage().persistent().extend_ttl(
                &SorobanStorageKey::PolicyPrincipals,
                threshold,
                extend_to,
            );
        }
        if self
            .env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::PolicyCapGroups)
        {
            self.env.storage().persistent().extend_ttl(
                &SorobanStorageKey::PolicyCapGroups,
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
                pc_deserialize::<VersionedState>(&stored, "state blob deserialize failed")?;

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
        let state_blob = pc_serialize(state, "state blob serialize failed")?;
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
        let locks = match SorobanStorage::load_policy_locks(self) {
            Some(stored) => Some(pc_deserialize::<MarketLockSet>(
                &stored,
                "policy_locks deserialize failed",
            )?),
            None => None,
        };
        let supply_queue = match SorobanStorage::load_policy_supply_queue(self) {
            Some(stored) => Some(pc_deserialize::<SupplyQueue>(
                &stored,
                "policy_supply_queue deserialize failed",
            )?),
            None => None,
        };
        let markets = match SorobanStorage::load_policy_markets(self) {
            Some(stored) => Some(pc_deserialize::<OrderedMap<TargetId, MarketConfig>>(
                &stored,
                "policy_markets deserialize failed",
            )?),
            None => None,
        };
        let principals = match SorobanStorage::load_policy_principals(self) {
            Some(stored) => Some(pc_deserialize::<OrderedMap<TargetId, u128>>(
                &stored,
                "policy_principals deserialize failed",
            )?),
            None => None,
        };
        let cap_groups = match SorobanStorage::load_policy_cap_groups(self) {
            Some(stored) => Some(pc_deserialize::<OrderedMap<CapGroupId, CapGroupRecord>>(
                &stored,
                "policy_cap_groups deserialize failed",
            )?),
            None => None,
        };

        if locks.is_none()
            && supply_queue.is_none()
            && markets.is_none()
            && principals.is_none()
            && cap_groups.is_none()
        {
            return Ok(None);
        }

        let mut state = PolicyState::new();
        if let Some(markets) = markets {
            state.markets = markets;
        }
        if let Some(principals) = principals {
            state.principals = principals;
        }
        if let Some(cap_groups) = cap_groups {
            state.cap_groups = cap_groups;
        }
        if let Some(locks) = locks {
            state.locks = locks;
        }
        if let Some(supply_queue) = supply_queue {
            state.supply_queue = supply_queue;
        }
        Ok(Some(state))
    }

    fn save_policy_state(&mut self, state: &PolicyState) -> Result<(), RuntimeError> {
        let lock_bytes = pc_serialize(&state.locks, "policy_locks serialize failed")?;
        SorobanStorage::save_policy_locks(self, &lock_bytes);
        let queue_bytes =
            pc_serialize(&state.supply_queue, "policy_supply_queue serialize failed")?;
        SorobanStorage::save_policy_supply_queue(self, &queue_bytes);
        let market_bytes = pc_serialize(&state.markets, "policy_markets serialize failed")?;
        SorobanStorage::save_policy_markets(self, &market_bytes);
        let principal_bytes =
            pc_serialize(&state.principals, "policy_principals serialize failed")?;
        SorobanStorage::save_policy_principals(self, &principal_bytes);
        let cap_group_bytes =
            pc_serialize(&state.cap_groups, "policy_cap_groups serialize failed")?;
        SorobanStorage::save_policy_cap_groups(self, &cap_group_bytes);
        self.extend_default_ttl();
        Ok(())
    }

    fn load_restrictions(&self) -> Result<Option<Restrictions>, RuntimeError> {
        match SorobanStorage::load_restrictions(self) {
            Some(stored) => Ok(Some(pc_deserialize::<Restrictions>(
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
            let bytes = pc_serialize(restrictions, "restrictions serialize failed")?;
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
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, From, Into,
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
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
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
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
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
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Default)]
pub struct MemoryStorage {
    state: Option<VersionedState>,
    initialized: bool,
    paused: bool,
    policy_locks: Option<templar_curator_primitives::policy::market_lock::MarketLockSet>,
    policy_supply_queue: Option<templar_curator_primitives::policy::supply_queue::SupplyQueue>,
    policy_markets: Option<OrderedMap<TargetId, MarketConfig>>,
    policy_principals: Option<OrderedMap<TargetId, u128>>,
    policy_cap_groups: Option<OrderedMap<CapGroupId, CapGroupRecord>>,
    restrictions: Option<Restrictions>,
    address_book: AddressBook<SdkAddress>,
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
            policy_locks: None,
            policy_supply_queue: None,
            policy_markets: None,
            policy_principals: None,
            policy_cap_groups: None,
            restrictions: None,
            address_book: AddressBook::new(),
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
        self.policy_locks = None;
        self.policy_supply_queue = None;
        self.policy_markets = None;
        self.policy_principals = None;
        self.policy_cap_groups = None;
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
        if self.policy_locks.is_none()
            && self.policy_supply_queue.is_none()
            && self.policy_markets.is_none()
            && self.policy_principals.is_none()
            && self.policy_cap_groups.is_none()
        {
            return Ok(None);
        }

        let mut state = PolicyState::new();
        if let Some(markets) = self.policy_markets.clone() {
            state.markets = markets;
        }
        if let Some(principals) = self.policy_principals.clone() {
            state.principals = principals;
        }
        if let Some(cap_groups) = self.policy_cap_groups.clone() {
            state.cap_groups = cap_groups;
        }
        if let Some(locks) = self.policy_locks.clone() {
            state.locks = locks;
        }
        if let Some(supply_queue) = self.policy_supply_queue.clone() {
            state.supply_queue = supply_queue;
        }
        Ok(Some(state))
    }

    fn save_policy_state(&mut self, state: &PolicyState) -> Result<(), RuntimeError> {
        self.policy_locks = Some(state.locks.clone());
        self.policy_supply_queue = Some(state.supply_queue.clone());
        self.policy_markets = Some(state.markets.clone());
        self.policy_principals = Some(state.principals.clone());
        self.policy_cap_groups = Some(state.cap_groups.clone());
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
        Ok(self.address_book.resolve(kernel_addr).cloned())
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
