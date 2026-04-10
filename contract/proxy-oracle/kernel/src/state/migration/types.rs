use near_sdk::near;
use templar_common::{
    panic_with_message,
    versioned_state::{Migrator, StateTransformer},
};

use super::v0_to_v1::V0;

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(tag = "from_version", rename_all = "snake_case")]
pub enum Migration {
    V0(V0),
}

impl From<V0> for Migration {
    fn from(value: V0) -> Self {
        Self::V0(value)
    }
}

impl Migrator for Migration {
    fn run(self) {
        match self {
            Migration::V0(v0) => {
                v0.run()
                    .unwrap_or_else(|e| panic_with_message(&format!("Failed to migrate V0: {e}")));
            }
        }
    }
}
