//! Migrations from each pre-versioning layout into [`state::V1`].
//!
//! Both start at version 0 and end at 1, so each registry runs a single step rather than a chain.
//! Which variant is correct depends on the release a registry is running, not on anything it
//! reports: see [`state::legacy`].

use near_sdk::{env, near, store::IterableMap};
use templar_common::versioned_state::{Migrator, StateTransformer};

use crate::{state, VersionEntry};

/// Registry 0.1.0 / 1.0.0 → [`state::V1`].
#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct PreGlobalContracts;

impl StateTransformer for PreGlobalContracts {
    type Input = state::legacy::PreGlobalContracts;
    type Output = state::V1;
    type Error = ();

    fn transform(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        let state::legacy::PreGlobalContracts {
            mut versions,
            registry,
        } = input;

        // Every value gains borsh's enum discriminant, so the map is rebuilt rather than moved.
        // Draining and dropping the old one first is load-bearing: two live collections over one
        // prefix both flush on drop, in an order neither of them agrees on.
        let keys = versions.keys().cloned().collect::<Vec<_>>();
        let drained = keys
            .into_iter()
            .map(|key| {
                let entry = versions
                    .remove(&key)
                    .unwrap_or_else(|| env::panic_str("version key vanished mid-migration"));
                let entry = VersionEntry::Code {
                    hash: entry.hash,
                    code: entry.code,
                };
                (key, entry)
            })
            .collect::<Vec<_>>();
        drop(versions);

        let mut versions = IterableMap::new(state::VERSIONS_PREFIX);
        for (key, entry) in drained {
            versions.insert(key, entry);
        }

        Ok(state::V1 { versions, registry })
    }
}

/// Registry 1.1.0 / 1.2.x → [`state::V1`].
#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct WithGlobalContracts;

impl StateTransformer for WithGlobalContracts {
    type Input = state::legacy::WithGlobalContracts;
    type Output = state::V1;
    type Error = ();

    fn transform(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        let state::legacy::WithGlobalContracts {
            versions,
            mut global_contract_hashes,
            registry,
        } = input;

        // Dropping the field is the whole migration; both maps keep their prefixes and their
        // encodings. Clearing is for the entries no release ever wrote — dropping a non-empty
        // collection would strand them under a prefix nothing reads.
        global_contract_hashes.clear();

        Ok(state::V1 { versions, registry })
    }
}

/// The migrations `upgrade` can be asked to run, JSON-tagged by the layout they read.
#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(tag = "from_version", rename_all = "snake_case")]
pub enum Migration {
    PreGlobalContracts(PreGlobalContracts),
    WithGlobalContracts(WithGlobalContracts),
}

impl From<PreGlobalContracts> for Migration {
    fn from(value: PreGlobalContracts) -> Self {
        Self::PreGlobalContracts(value)
    }
}

impl From<WithGlobalContracts> for Migration {
    fn from(value: WithGlobalContracts) -> Self {
        Self::WithGlobalContracts(value)
    }
}

impl Migrator for Migration {
    fn input_version(&self) -> u32 {
        match self {
            Migration::PreGlobalContracts(m) => m.input_version(),
            Migration::WithGlobalContracts(m) => m.input_version(),
        }
    }

    fn output_version(&self) -> u32 {
        match self {
            Migration::PreGlobalContracts(m) => m.output_version(),
            Migration::WithGlobalContracts(m) => m.output_version(),
        }
    }

    fn run(self) {
        let (result, label) = match self {
            Migration::PreGlobalContracts(m) => (m.run().map(|_| ()), "PreGlobalContracts"),
            Migration::WithGlobalContracts(m) => (m.run().map(|_| ()), "WithGlobalContracts"),
        };

        result.unwrap_or_else(|e| env::panic_str(&format!("Failed to migrate {label}: {e}")));
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::{
        env,
        json_types::Base58CryptoHash,
        serde_json::{self, json},
        store::{IterableMap, IterableSet},
        test_utils::VMContextBuilder,
        testing_env,
    };
    use templar_common::{
        registry::Deployment,
        versioned_state::{read_state_version, run_migration_chain, MigrationChainError},
    };

    use crate::{state::legacy, RegistryEntry};

    use super::*;

    const MARKET: &str = "market@1.5.0";
    const REMOVED: &str = "market@1.0.0";

    fn context() {
        testing_env!(VMContextBuilder::new().build());
    }

    fn reserved_id() -> near_sdk::AccountId {
        "a.registry.near".parse().unwrap()
    }

    fn deployed_id() -> near_sdk::AccountId {
        "b.registry.near".parse().unwrap()
    }

    fn deployment() -> Deployment {
        Deployment {
            version_key: MARKET.to_string(),
            code_hash: Base58CryptoHash::from([9u8; 32]),
            block_height: 100.into(),
        }
    }

    fn write_pre_global_contracts() {
        let mut state = legacy::PreGlobalContracts {
            versions: IterableMap::new(state::VERSIONS_PREFIX),
            registry: IterableMap::new(state::REGISTRY_PREFIX),
        };
        state.versions.insert(
            MARKET.to_string(),
            legacy::StoredVersionEntry {
                hash: [1u8; 32],
                code: Some(vec![0xde, 0xad, 0xbe, 0xef]),
            },
        );
        // remove_version soft-deletes by clearing the blob; the entry stays.
        state.versions.insert(
            REMOVED.to_string(),
            legacy::StoredVersionEntry {
                hash: [2u8; 32],
                code: None,
            },
        );
        state
            .registry
            .insert(reserved_id(), RegistryEntry::Reserved);
        state
            .registry
            .insert(deployed_id(), RegistryEntry::Deployed(deployment()));
        env::state_write(&state);
        drop(state);
    }

    fn write_with_global_contracts() {
        let mut state = legacy::WithGlobalContracts {
            versions: IterableMap::new(state::VERSIONS_PREFIX),
            global_contract_hashes: IterableSet::new(b"g"),
            registry: IterableMap::new(state::REGISTRY_PREFIX),
        };
        state.versions.insert(
            MARKET.to_string(),
            VersionEntry::Code {
                hash: [1u8; 32],
                code: Some(vec![0xde, 0xad, 0xbe, 0xef]),
            },
        );
        state.versions.insert(
            "oracle@0.4.1".to_string(),
            VersionEntry::GlobalHash([3u8; 32]),
        );
        state
            .registry
            .insert(deployed_id(), RegistryEntry::Deployed(deployment()));
        env::state_write(&state);
        drop(state);
    }

    #[test]
    fn pre_global_contracts_rewrites_entries_and_keeps_everything() {
        context();
        write_pre_global_contracts();

        let new = PreGlobalContracts.run().unwrap();

        assert_eq!(read_state_version().unwrap(), 1);
        assert_eq!(new.versions.len(), 2);
        assert!(matches!(
            new.versions.get(MARKET),
            Some(VersionEntry::Code { hash, code: Some(code) }) if *hash == [1u8; 32] && code == &[0xde, 0xad, 0xbe, 0xef],
        ));
        // The soft-delete has to survive as one, not become a deployable version.
        assert!(matches!(
            new.versions.get(REMOVED),
            Some(VersionEntry::Code { code: None, .. }),
        ));
        assert_eq!(new.registry.len(), 2);
        assert!(matches!(
            new.registry.get(&reserved_id()),
            Some(RegistryEntry::Reserved),
        ));
    }

    #[test]
    fn with_global_contracts_keeps_both_version_kinds() {
        context();
        write_with_global_contracts();

        let new = WithGlobalContracts.run().unwrap();

        assert_eq!(read_state_version().unwrap(), 1);
        assert_eq!(new.versions.len(), 2);
        assert!(matches!(
            new.versions.get(MARKET),
            Some(VersionEntry::Code { code: Some(_), .. }),
        ));
        assert!(matches!(
            new.versions.get("oracle@0.4.1"),
            Some(VersionEntry::GlobalHash(hash)) if *hash == [3u8; 32],
        ));
        assert_eq!(new.registry.len(), 1);
    }

    /// Both layouts report a stored version of 0, so nothing but the operator's choice selects
    /// between them, and a wrong choice has to fail rather than misread. It does: the layouts
    /// differ by a whole collection, so reading either as the other runs out of bytes or leaves
    /// some over, and borsh rejects both. The abort takes the batched deploy down with it, which
    /// is why no separate confirmation argument is needed to make the choice safe.
    #[rstest::rstest]
    #[case::three_field_read_as_two(write_with_global_contracts, || { PreGlobalContracts.run().ok(); })]
    #[case::two_field_read_as_three(write_pre_global_contracts, || { WithGlobalContracts.run().ok(); })]
    #[should_panic(expected = "Cannot deserialize the contract state.")]
    fn wrong_variant_refuses_to_read_state(
        #[case] write_state: fn(),
        #[case] run_mismatched: fn(),
    ) {
        context();
        write_state();

        run_mismatched();
    }

    #[test]
    fn both_migrations_span_zero_to_one() {
        context();
        for migration in [
            Migration::from(PreGlobalContracts),
            Migration::from(WithGlobalContracts),
        ] {
            assert_eq!(migration.input_version(), 0);
            assert_eq!(migration.output_version(), 1);
        }
    }

    /// The tag an operator types is a wire format; a rename would silently invalidate a runbook.
    #[test]
    fn migration_wire_format() {
        assert_eq!(
            serde_json::to_value(Migration::from(PreGlobalContracts)).unwrap(),
            json!({ "from_version": "pre_global_contracts" }),
        );
        assert_eq!(
            serde_json::to_value(Migration::from(WithGlobalContracts)).unwrap(),
            json!({ "from_version": "with_global_contracts" }),
        );
    }

    #[test]
    fn chain_rejects_a_migration_that_does_not_reach_the_target() {
        context();
        write_pre_global_contracts();

        assert!(matches!(
            run_migration_chain(vec![Migration::from(PreGlobalContracts)], 2),
            Err(MigrationChainError::EndMismatch {
                output: 1,
                target: 2
            }),
        ));
    }
}
