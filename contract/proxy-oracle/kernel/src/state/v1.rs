use near_sdk::{borsh::BorshSerialize, collections::UnorderedMap, near, BorshStorageKey};
use templar_common::{
    governance::Governance,
    oracle::pyth::PriceIdentifier,
    versioned_state::{StateVersion, VersionedState},
};

use crate::proxy::{governance::Operation, Proxy};

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
pub enum StorageKey {
    Governance,
    Proxies,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct State {
    pub governance: Governance<Operation>,
    pub proxies: UnorderedMap<PriceIdentifier, Proxy>,
}

impl StateVersion for State {
    const VERSION: u32 = 1;

    type NewArgs = ();

    fn new((): Self::NewArgs) -> VersionedState<Self> {
        VersionedState::new(Self {
            governance: Governance::new(StorageKey::Governance),
            proxies: UnorderedMap::new(StorageKey::Proxies),
        })
    }
}
