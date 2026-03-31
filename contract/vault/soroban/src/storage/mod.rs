//! Storage versioning and persistence for Soroban ledger.
//!
//! This module provides versioned storage wrappers for persisting vault state
//! to the Soroban ledger. It handles schema migrations and forward compatibility.

use alloc::vec::Vec;
use derive_more::{From, Into};
use soroban_sdk::{symbol_short, Address as SdkAddress, Bytes, BytesN, Env, Symbol};
use templar_curator_primitives::policy::cap_group::{CapGroupId, CapGroupRecord};
use templar_curator_primitives::policy::market_lock::MarketLockSet;
use templar_curator_primitives::policy::state::{MarketConfig, OrderedMap};
use templar_curator_primitives::policy::supply_queue::SupplyQueue;
use templar_curator_primitives::PolicyState;
use templar_vault_kernel::{Address, Restrictions, TargetId, VaultState};

use crate::error::RuntimeError;

/// Re-extend TTL when remaining TTL drops below ~30 days (at ~5s/ledger).
pub(crate) const DEFAULT_TTL_THRESHOLD: u32 = 518_400;
/// Extend TTL to the Soroban maximum (~6 months at ~5s/ledger).
/// For a vault contract holding real assets, maximum TTL prevents state
/// loss during extended pauses or periods of inactivity.
pub(crate) const DEFAULT_TTL_EXTEND_TO: u32 = 3_110_400;

/// Internal persistent storage keys. Using Symbol constants instead of a
/// `#[contracttype]` enum to avoid contractspec bloat and enum conversion codegen.
#[allow(non_upper_case_globals)]
pub struct SorobanStorageKey;

#[allow(non_upper_case_globals)]
impl SorobanStorageKey {
    pub const StateBlob: Symbol = symbol_short!("stblob");
    pub const PolicyLocks: Symbol = symbol_short!("plocks");
    pub const PolicySupplyQueue: Symbol = symbol_short!("psupplyq");
    pub const PolicyMarkets: Symbol = symbol_short!("pmkts");
    pub const PolicyPrincipals: Symbol = symbol_short!("pprncpls");
    pub const PolicyCapGroups: Symbol = symbol_short!("pcapgrps");
    pub const Restrictions: Symbol = symbol_short!("restrict");
    pub const Version: Symbol = symbol_short!("version");
    pub const Paused: Symbol = symbol_short!("paused_l"); // legacy pause key (migration)
    pub const PausedState: Symbol = symbol_short!("paused_s");
}

fn pc_serialize<T: serde::Serialize>(
    value: &T,
    msg: &'static str,
) -> Result<Vec<u8>, RuntimeError> {
    postcard::to_allocvec(value).map_err(|_| RuntimeError::storage_error(msg))
}

fn pc_deserialize<'a, T: serde::Deserialize<'a>>(
    bytes: &'a [u8],
    msg: &'static str,
) -> Result<T, RuntimeError> {
    postcard::from_bytes(bytes).map_err(|_| RuntimeError::storage_error(msg))
}

pub(crate) fn compose_policy_state(
    markets: Option<OrderedMap<TargetId, MarketConfig>>,
    principals: Option<OrderedMap<TargetId, u128>>,
    cap_groups: Option<OrderedMap<CapGroupId, CapGroupRecord>>,
    locks: Option<MarketLockSet>,
    supply_queue: Option<SupplyQueue>,
) -> Option<PolicyState> {
    if markets.is_none()
        && principals.is_none()
        && cap_groups.is_none()
        && locks.is_none()
        && supply_queue.is_none()
    {
        return None;
    }

    let mut state = PolicyState::default();
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
    Some(state)
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

    const SK_ADDRBOOK: Symbol = symbol_short!("addrbook");

    fn address_key(&self, kernel_addr: &Address) -> (Symbol, BytesN<32>) {
        (Self::SK_ADDRBOOK, BytesN::from_array(self.env, kernel_addr))
    }

    fn load_blob(&self, key: &Symbol) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(key)
            .map(|bytes| bytes.to_alloc_vec())
    }

    fn save_blob(&self, key: &Symbol, bytes: &[u8]) {
        self.env
            .storage()
            .persistent()
            .set(key, &Bytes::from_slice(self.env, bytes));
    }

    fn load_decoded<T>(&self, key: &Symbol, msg: &'static str) -> Result<Option<T>, RuntimeError>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        self.load_blob(key)
            .map(|stored| pc_deserialize(&stored, msg))
            .transpose()
    }

    fn save_encoded<T>(
        &self,
        key: &Symbol,
        value: &T,
        msg: &'static str,
    ) -> Result<(), RuntimeError>
    where
        T: serde::Serialize,
    {
        let bytes = pc_serialize(value, msg)?;
        self.save_blob(key, &bytes);
        Ok(())
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

    pub(crate) fn load_state_blob(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::StateBlob)
    }

    pub(crate) fn save_state_blob(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::StateBlob, state);
    }

    pub fn load_policy_locks(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::PolicyLocks)
    }

    pub fn save_policy_locks(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::PolicyLocks, state);
    }

    pub fn load_policy_supply_queue(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::PolicySupplyQueue)
    }

    pub fn save_policy_supply_queue(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::PolicySupplyQueue, state);
    }

    pub fn load_policy_markets(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::PolicyMarkets)
    }

    pub fn save_policy_markets(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::PolicyMarkets, state);
    }

    pub fn load_policy_principals(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::PolicyPrincipals)
    }

    pub fn save_policy_principals(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::PolicyPrincipals, state);
    }

    pub fn load_policy_cap_groups(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::PolicyCapGroups)
    }

    pub fn save_policy_cap_groups(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::PolicyCapGroups, state);
    }

    /// Load restrictions from persistent storage.
    pub fn load_restrictions(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::Restrictions)
    }

    /// Save restrictions to persistent storage.
    pub fn save_restrictions(&self, restrictions: &[u8]) {
        self.save_blob(&SorobanStorageKey::Restrictions, restrictions);
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
            .get(&SorobanStorageKey::PausedState)
            .unwrap_or(false)
    }

    /// Set the pause state in instance storage.
    pub fn set_paused(&self, paused: bool) {
        self.env
            .storage()
            .instance()
            .set(&SorobanStorageKey::PausedState, &paused);
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
        let p = self.env.storage().persistent();
        // Extend each persistent key if it exists.
        for key in &[
            SorobanStorageKey::StateBlob,
            SorobanStorageKey::PolicyLocks,
            SorobanStorageKey::PolicySupplyQueue,
            SorobanStorageKey::PolicyMarkets,
            SorobanStorageKey::PolicyPrincipals,
            SorobanStorageKey::PolicyCapGroups,
            SorobanStorageKey::Restrictions,
        ] {
            if p.has(key) {
                p.extend_ttl(key, threshold, extend_to);
            }
        }
        p.extend_ttl(&SorobanStorageKey::Version, threshold, extend_to);
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
        let locks = self.load_decoded(
            &SorobanStorageKey::PolicyLocks,
            "policy_locks deserialize failed",
        )?;
        let supply_queue = self.load_decoded(
            &SorobanStorageKey::PolicySupplyQueue,
            "policy_supply_queue deserialize failed",
        )?;
        let markets = self.load_decoded(
            &SorobanStorageKey::PolicyMarkets,
            "policy_markets deserialize failed",
        )?;
        let principals = self.load_decoded(
            &SorobanStorageKey::PolicyPrincipals,
            "policy_principals deserialize failed",
        )?;
        let cap_groups = self.load_decoded(
            &SorobanStorageKey::PolicyCapGroups,
            "policy_cap_groups deserialize failed",
        )?;

        Ok(compose_policy_state(
            markets,
            principals,
            cap_groups,
            locks,
            supply_queue,
        ))
    }

    fn save_policy_state(&mut self, state: &PolicyState) -> Result<(), RuntimeError> {
        self.save_encoded(
            &SorobanStorageKey::PolicyLocks,
            &state.locks,
            "policy_locks serialize failed",
        )?;
        self.save_encoded(
            &SorobanStorageKey::PolicySupplyQueue,
            &state.supply_queue,
            "policy_supply_queue serialize failed",
        )?;
        self.save_encoded(
            &SorobanStorageKey::PolicyMarkets,
            &state.markets,
            "policy_markets serialize failed",
        )?;
        self.save_encoded(
            &SorobanStorageKey::PolicyPrincipals,
            &state.principals,
            "policy_principals serialize failed",
        )?;
        self.save_encoded(
            &SorobanStorageKey::PolicyCapGroups,
            &state.cap_groups,
            "policy_cap_groups serialize failed",
        )?;
        self.extend_default_ttl();
        Ok(())
    }

    fn load_restrictions(&self) -> Result<Option<Restrictions>, RuntimeError> {
        self.load_decoded(
            &SorobanStorageKey::Restrictions,
            "restrictions deserialize failed",
        )
    }

    fn save_restrictions(
        &mut self,
        restrictions: &Option<Restrictions>,
    ) -> Result<(), RuntimeError> {
        if let Some(restrictions) = restrictions {
            self.save_encoded(
                &SorobanStorageKey::Restrictions,
                restrictions,
                "restrictions serialize failed",
            )?;
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
