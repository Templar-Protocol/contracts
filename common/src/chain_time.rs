use std::fmt::Display;

use near_sdk::{env, json_types::U64, near};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct ChainTime(U64);

impl ChainTime {
    pub fn now() -> Self {
        Self(U64(env::block_height()))
    }
}

impl Display for ChainTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "block:{}", u64::from(self.0))
    }
}
