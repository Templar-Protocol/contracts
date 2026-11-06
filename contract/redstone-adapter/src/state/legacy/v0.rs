use near_sdk::near;
use templar_common::{
    oracle::redstone::{Config, RedStoneAdapter},
    versioned_state::{StateVersion, VersionedState},
};

/// The exact adapter-only root used before state-version metadata was introduced.
#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct State {
    pub adapter: RedStoneAdapter,
}

impl StateVersion for State {
    const VERSION: u32 = 0;
    type NewArgs = Config;

    fn new(config: Config) -> VersionedState<Self> {
        VersionedState::new(Self {
            adapter: RedStoneAdapter::new(b"a", config),
        })
    }
}
