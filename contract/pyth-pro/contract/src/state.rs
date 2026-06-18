//! Versioned contract state. The live fields (config plus the feed/id maps) live here behind
//! [`templar_common::versioned_state::VersionedState`], so an upgrade runs a typed,
//! version-checked `migrate` (see [`migration`]) instead of an ad-hoc `state_read`. Adopting this
//! from v1 — before the contract is deployed — avoids a later unversioned→versioned migration.

pub mod migration;

use near_sdk::{borsh::BorshSerialize, near, store::IterableMap, BorshStorageKey};
use templar_common::{
    oracle::pyth::PriceIdentifier,
    versioned_state::{StateVersion, VersionedState},
};

use crate::{Config, FeedData};

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Feeds,
    Ids,
}

/// The adapter's persistent state (version 1). Its Borsh layout is the on-chain schema; any change
/// requires bumping [`StateVersion::VERSION`] and adding a [`migration`] transformer for it.
#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct State {
    pub(crate) config: Config,
    /// Latest data keyed by the natural Lazer `u32` feed id.
    pub(crate) feeds: IterableMap<u32, FeedData>,
    /// Mapping layer (see `feed_map`): consumer `PriceIdentifier` -> Lazer feed id.
    pub(crate) ids: IterableMap<PriceIdentifier, u32>,
}

impl StateVersion for State {
    const VERSION: u32 = 1;
    type NewArgs = Config;

    fn new(config: Config) -> VersionedState<Self> {
        VersionedState::new(Self {
            config,
            feeds: IterableMap::new(StorageKey::Feeds),
            ids: IterableMap::new(StorageKey::Ids),
        })
    }
}
