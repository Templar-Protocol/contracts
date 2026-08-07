//! The state layouts of registries deployed before versioning existed.
//!
//! Both declare `VERSION = 0`, because none of the releases that produced them wrote the version
//! key: a registry holding either layout reports a stored version of 0, so that is the only input
//! version a migration out of one can declare. Which layout a given registry holds is therefore
//! not something the contract can infer — the operator names it.
//!
//! What refuses a wrong name is the stored version, not borsh: a transform only runs from the
//! version it declares, so nothing here can run against state that has already moved on. Borsh
//! separates these two from *each other* — they differ by a whole collection, and it rejects the
//! leftover or missing bytes that produces — but it cannot separate either from [`super::V1`],
//! which is likewise two `IterableMap`s. A map's borsh footprint is its prefix and length, never
//! its values.

use near_sdk::{
    near,
    store::{IterableMap, IterableSet},
    AccountId, CryptoHash,
};
use templar_common::versioned_state::{StateVersion, VersionedState};

use crate::{RegistryEntry, VersionEntry};

/// Retired with [`WithGlobalContracts`]. Recorded so it is never reused for something else.
const GLOBAL_CONTRACT_HASHES_PREFIX: &[u8] = b"g";

/// A `versions` value before 1.1.0, when stored code was the only kind of version there was.
///
/// [`crate::VersionEntry`] is an enum and borsh prefixes it with a discriminant, so the two
/// encodings differ by a leading byte and every entry has to be rewritten rather than
/// reinterpreted. Which byte it is cannot be detected either: the legacy encoding opens with the
/// first byte of a sha256, and that aliases both of the enum's tags.
#[near(serializers = [borsh])]
pub struct StoredVersionEntry {
    pub hash: CryptoHash,
    pub code: Option<Vec<u8>>,
}

/// Registry 0.1.0 and 1.0.0.
#[near(serializers = [borsh])]
pub struct PreGlobalContracts {
    pub versions: IterableMap<String, StoredVersionEntry>,
    pub registry: IterableMap<AccountId, RegistryEntry>,
}

impl StateVersion for PreGlobalContracts {
    const VERSION: u32 = 0;

    type NewArgs = ();

    fn new((): ()) -> VersionedState<Self> {
        VersionedState::new(Self {
            versions: IterableMap::new(super::VERSIONS_PREFIX),
            registry: IterableMap::new(super::REGISTRY_PREFIX),
        })
    }
}

/// Registry 1.1.0 through 1.2.4.
#[near(serializers = [borsh])]
pub struct WithGlobalContracts {
    pub versions: IterableMap<String, VersionEntry>,
    /// Initialised by every release that had it and never written to again.
    pub global_contract_hashes: IterableSet<CryptoHash>,
    pub registry: IterableMap<AccountId, RegistryEntry>,
}

impl StateVersion for WithGlobalContracts {
    const VERSION: u32 = 0;

    type NewArgs = ();

    fn new((): ()) -> VersionedState<Self> {
        VersionedState::new(Self {
            versions: IterableMap::new(super::VERSIONS_PREFIX),
            global_contract_hashes: IterableSet::new(GLOBAL_CONTRACT_HASHES_PREFIX),
            registry: IterableMap::new(super::REGISTRY_PREFIX),
        })
    }
}
