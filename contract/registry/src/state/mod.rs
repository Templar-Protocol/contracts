//! Versioned contract state.
//!
//! Versioning was adopted after three registries were already live on 0.1.0, 1.0.0 and 1.1.0,
//! none of which wrote the version key — so [`read_state_version`] reports 0 for all of them and
//! every migration into [`V1`] starts from 0 regardless of which layout it is actually reading.
//! See [`legacy`] for the two layouts that exist and how the right one is chosen.
//!
//! [`read_state_version`]: templar_common::versioned_state::read_state_version

pub mod legacy;
pub mod migration;

pub use migration::Migration;

use near_sdk::{near, store::IterableMap, AccountId};
use templar_common::versioned_state::{StateVersion, VersionedState};

use crate::{RegistryEntry, VersionEntry};

pub(crate) const VERSIONS_PREFIX: &[u8] = b"v";
pub(crate) const REGISTRY_PREFIX: &[u8] = b"r";

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
