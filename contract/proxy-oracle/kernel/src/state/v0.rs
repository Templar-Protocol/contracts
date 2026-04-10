use near_sdk::{collections::UnorderedMap, near};
use templar_common::{
    governance::Governance,
    oracle::pyth::PriceIdentifier,
    versioned_state::{StateVersion, VersionedState},
};

use super::storage::StorageKey;
use crate::proxy::legacy::v0::Proxy;

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct V0 {
    pub governance: Governance<crate::proxy::legacy::v0::governance::Operation>,
    pub proxies: UnorderedMap<PriceIdentifier, Proxy>,
}

impl StateVersion for V0 {
    const VERSION: u32 = 0;

    type NewArgs = ();

    fn new((): Self::NewArgs) -> VersionedState<Self> {
        VersionedState::new(Self {
            governance: Governance::new(StorageKey::Governance),
            proxies: UnorderedMap::new(StorageKey::Proxies),
        })
    }
}
