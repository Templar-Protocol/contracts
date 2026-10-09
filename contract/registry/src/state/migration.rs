use near_sdk::near;
use templar_common::versioned_state::Migrator;

/// Empty until a layout after [`crate::state::V1`] needs one.
#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(tag = "from_version", rename_all = "snake_case")]
pub enum Migration {}

impl Migrator for Migration {
    fn input_version(&self) -> u32 {
        match *self {}
    }

    fn output_version(&self) -> u32 {
        match *self {}
    }

    fn run(self) {
        match self {}
    }
}
