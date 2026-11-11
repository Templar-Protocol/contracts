mod v0_to_v1;

use near_sdk::near;
use templar_common::{
    panic_with_message,
    versioned_state::{Migrator, StateTransformer as _},
};
pub use v0_to_v1::V0ToV1;

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(tag = "from_version", rename_all = "snake_case")]
pub enum Migration {
    V0(V0ToV1),
}

impl From<V0ToV1> for Migration {
    fn from(value: V0ToV1) -> Self {
        Self::V0(value)
    }
}

impl Migrator for Migration {
    fn input_version(&self) -> u32 {
        match self {
            Self::V0(migration) => migration.input_version(),
        }
    }

    fn output_version(&self) -> u32 {
        match self {
            Self::V0(migration) => migration.output_version(),
        }
    }

    fn run(self) {
        match self {
            Self::V0(migration) => {
                migration
                    .run()
                    .unwrap_or_else(|e| panic_with_message(&format!("Failed to migrate V0: {e}")));
            }
        }
    }
}
