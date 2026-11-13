pub mod legacy;
pub mod migration;

use near_sdk::near;
use templar_common::{
    oracle::redstone::{Config, RedStoneAdapter},
    versioned_state::{StateVersion, VersionedState},
};

/// Persistent adapter state. Feed and RBAC storage prefixes are unchanged from V0.
#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct State {
    pub adapter: RedStoneAdapter,
}

impl StateVersion for State {
    const VERSION: u32 = 1;
    type NewArgs = Config;

    fn new(config: Config) -> VersionedState<Self> {
        VersionedState::new(Self {
            adapter: RedStoneAdapter::new(b"a", config),
        })
    }
}
