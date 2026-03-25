use near_sdk::{near, store::IterableMap};

use crate::{
    contract_state::{StateVersion, VersionedState},
    KeyId, KeyParameters,
};

pub mod migration;
pub use migration::Migration;

#[near(serializers = [borsh])]
pub struct V0 {
    pub next_key_index: u64,
    pub keys: IterableMap<KeyId, KeyParameters>,
}

impl StateVersion for V0 {
    const VERSION: u32 = 0;

    type NewArgs = ();

    fn new((): ()) -> VersionedState<Self> {
        VersionedState::new(Self {
            next_key_index: 0,
            keys: IterableMap::new(b"k"),
        })
    }
}

#[near(serializers = [borsh])]
pub struct V1 {
    pub next_key_index: u64,
    pub keys: IterableMap<KeyId, KeyParameters>,
    pub chain_id: u128,
}

impl V1 {
    fn from_v0(old: V0, chain_id: u128) -> Self {
        Self {
            next_key_index: old.next_key_index,
            keys: old.keys,
            chain_id,
        }
    }
}

impl StateVersion for V1 {
    const VERSION: u32 = 1;

    type NewArgs = u128;

    fn new(chain_id: u128) -> VersionedState<Self> {
        VersionedState::new(Self {
            next_key_index: 0,
            keys: IterableMap::new(b"k"),
            chain_id,
        })
    }
}

#[near(serializers = [borsh])]
pub struct V2 {
    pub next_key_index: u64,
    pub keys: IterableMap<KeyId, KeyParameters>,
    pub chain_id: u128,
}

impl V2 {
    fn from_v1(old: V1) -> Self {
        Self {
            next_key_index: old.next_key_index,
            keys: old.keys,
            chain_id: old.chain_id,
        }
    }
}

impl StateVersion for V2 {
    const VERSION: u32 = 2;

    type NewArgs = u128;

    fn new(chain_id: u128) -> VersionedState<Self> {
        VersionedState::new(Self {
            next_key_index: 0,
            keys: IterableMap::new(b"k"),
            chain_id,
        })
    }
}
