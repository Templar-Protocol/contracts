pub mod migration;

pub use migration::Migration;

use near_sdk::{near, store::IterableMap, AccountId};
use templar_common::versioned_state::{StateVersion, VersionedState};

use crate::{RegistryEntry, Upload, VersionEntry};

const VERSIONS_PREFIX: &[u8] = b"v";
const REGISTRY_PREFIX: &[u8] = b"r";
const UPLOADS_PREFIX: &[u8] = b"u";

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
            versions: IterableMap::new(VERSIONS_PREFIX),
            registry: IterableMap::new(REGISTRY_PREFIX),
        })
    }
}

#[near(serializers = [borsh])]
pub struct V2 {
    pub versions: IterableMap<String, VersionEntry>,
    pub registry: IterableMap<AccountId, RegistryEntry>,
    pub uploads: IterableMap<near_sdk::CryptoHash, Upload>,
}

impl StateVersion for V2 {
    const VERSION: u32 = 2;

    type NewArgs = ();

    fn new((): ()) -> VersionedState<Self> {
        VersionedState::new(Self {
            versions: IterableMap::new(VERSIONS_PREFIX),
            registry: IterableMap::new(REGISTRY_PREFIX),
            uploads: IterableMap::new(UPLOADS_PREFIX),
        })
    }
}
