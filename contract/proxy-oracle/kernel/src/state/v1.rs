use near_sdk::{collections::UnorderedMap, near};
use templar_common::{
    governance::Governance,
    oracle::pyth::PriceIdentifier,
    versioned_state::{StateVersion, VersionedState},
};

use super::storage::StorageKey;
use crate::proxy::{governance::Operation, Proxy};

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct V1 {
    pub governance: Governance<Operation>,
    pub proxies: UnorderedMap<PriceIdentifier, Proxy>,
}

impl StateVersion for V1 {
    const VERSION: u32 = 1;

    type NewArgs = ();

    fn new((): Self::NewArgs) -> VersionedState<Self> {
        VersionedState::new(Self {
            governance: Governance::new(StorageKey::Governance),
            proxies: UnorderedMap::new(StorageKey::Proxies),
        })
    }
}
