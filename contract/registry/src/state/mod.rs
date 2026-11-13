pub mod migration;

pub use migration::Migration;

use near_sdk::{
    near,
    store::{IterableMap, LookupMap},
    AccountId, BorshStorageKey, CryptoHash, NearToken,
};
use templar_common::versioned_state::{StateVersion, VersionedState};

use crate::{Contract, RegistryEntry, StorageCharge, Upload, VersionEntry};

/// Explicit tags preserve the deployed collection and blob prefixes.
#[derive(BorshStorageKey)]
#[near(serializers = [borsh(use_discriminant = true)])]
#[repr(u8)]
pub(crate) enum StorageKey {
    StorageBounds = 0x62,
    UploadCode { hash: CryptoHash } = 0x63,
    StorageAccount { hash: CryptoHash } = 0x64,
    VersionStorage = 0x70,
    Registry = 0x72,
    Uploads = 0x75,
    Versions = 0x76,
}

#[near(serializers = [borsh])]
pub struct V1 {
    pub versions: IterableMap<String, VersionEntry>,
    pub registry: IterableMap<AccountId, RegistryEntry>,
}

impl StateVersion for V1 {
    const VERSION: u32 = 1;

    type NewArgs = ();

    fn new((): ()) -> VersionedState<Self> {
        VersionedState::new(Self {
            versions: IterableMap::new(StorageKey::Versions),
            registry: IterableMap::new(StorageKey::Registry),
        })
    }
}

#[near(serializers = [borsh])]
pub struct V2 {
    pub versions: IterableMap<String, VersionEntry>,
    pub registry: IterableMap<AccountId, RegistryEntry>,
    pub uploads: IterableMap<CryptoHash, Upload>,
    pub version_storage: LookupMap<String, StorageCharge>,
    pub available_storage_balance: NearToken,
    pub pending_deployment_storage: NearToken,
    pub storage_entry_overhead: u64,
}

impl V2 {
    fn with_collections(
        versions: IterableMap<String, VersionEntry>,
        registry: IterableMap<AccountId, RegistryEntry>,
    ) -> Self {
        let storage_entry_overhead = Contract::initialize_storage_management();
        Self {
            versions,
            registry,
            uploads: IterableMap::new(StorageKey::Uploads),
            version_storage: LookupMap::new(StorageKey::VersionStorage),
            available_storage_balance: NearToken::ZERO,
            pending_deployment_storage: NearToken::ZERO,
            storage_entry_overhead,
        }
    }
}

impl StateVersion for V2 {
    const VERSION: u32 = 2;

    type NewArgs = ();

    fn new((): ()) -> VersionedState<Self> {
        VersionedState::new(Self::with_collections(
            IterableMap::new(StorageKey::Versions),
            IterableMap::new(StorageKey::Registry),
        ))
    }
}
