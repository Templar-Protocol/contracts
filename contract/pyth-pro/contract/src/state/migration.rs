//! State-version migrations for the adapter.
//!
//! The contract launches at [`crate::state::State`]`::VERSION == 1`, so there are no migrations
//! yet — [`Migration`] is intentionally empty. When a future `State` layout (v2+) lands, add a
//! `V1(V1ToV2)` variant backed by a [`templar_common::versioned_state::StateTransformer`] and
//! dispatch it in [`Migrator::run`], mirroring
//! `contract/proxy-oracle/near/common/src/state/migration`.

use near_sdk::near;
use templar_common::versioned_state::Migrator;

/// JSON-tagged set of supported state migrations — empty at v1. `admin_upgrade` forwards the chosen
/// variant as `migrate_args`; the macro-generated `migrate` entrypoint deserializes it into this
/// type and runs it. With no variants, any `migrate` call fails to deserialize, which is correct:
/// there is nothing to migrate to yet.
#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(tag = "from_version", rename_all = "snake_case")]
pub enum Migration {}

impl Migrator for Migration {
    fn run(self) {
        match self {}
    }
}
