use near_sdk::near;
use templar_common::{
    panic_with_message,
    versioned_state::{Migrator, StateTransformer},
};

use super::{V1, V2};

/// Move the existing collection handles without rewriting historical entries or owner slots.
#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct V1ToV2;

impl StateTransformer for V1ToV2 {
    type Input = V1;
    type Output = V2;
    type Error = ();

    fn transform(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        Ok(V2::with_collections(input.versions, input.registry))
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(tag = "from_version", rename_all = "snake_case")]
pub enum Migration {
    V1(V1ToV2),
}

impl Migrator for Migration {
    fn input_version(&self) -> u32 {
        match self {
            Self::V1(migration) => migration.input_version(),
        }
    }

    fn output_version(&self) -> u32 {
        match self {
            Self::V1(migration) => migration.output_version(),
        }
    }

    fn run(self) {
        match self {
            Self::V1(migration) => {
                migration
                    .run()
                    .unwrap_or_else(|e| panic_with_message(&format!("Failed to migrate V1: {e}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::{
        env, json_types::U64, serde_json, test_utils::VMContextBuilder, testing_env, AccountId,
    };
    use near_sdk_contract_tools::{owner::OwnerInternal, rbac::Rbac};
    use templar_common::{
        registry::{Deployment, Role},
        versioned_state::{
            parse_one_or_many, read_state_version, run_migration_chain, write_state_version,
            MigrationChainError, StateVersion,
        },
    };

    use super::*;
    use crate::{Contract, RegistryEntry, VersionEntry};

    fn account(id: &str) -> AccountId {
        id.parse().unwrap()
    }

    fn fixture() {
        testing_env!(VMContextBuilder::new().build());
        let mut state = V1::new(());
        state.versions.insert(
            "stored".to_owned(),
            VersionEntry::Code {
                hash: [1; 32],
                code: Some(vec![0, 1, 2, 255]),
            },
        );
        state.versions.insert(
            "removed".to_owned(),
            VersionEntry::Code {
                hash: [2; 32],
                code: None,
            },
        );
        state
            .versions
            .insert("global".to_owned(), VersionEntry::GlobalHash([3; 32]));
        state
            .registry
            .insert(account("reserved.near"), RegistryEntry::Reserved);
        state.registry.insert(
            account("deployed.near"),
            RegistryEntry::Deployed(Deployment {
                version_key: "stored".to_owned(),
                code_hash: [1; 32].into(),
                block_height: U64(42),
            }),
        );
        state.versions.flush();
        state.registry.flush();
        env::state_write(&state);
        Contract::slot_is_initialized().write(&true);
        Contract::slot_owner().write(&account("owner.near"));
        Contract::slot_proposed_owner().write(&account("proposed.near"));
    }

    fn owner_slots() -> Vec<Option<Vec<u8>>> {
        [
            Contract::slot_is_initialized().key,
            Contract::slot_owner().key,
            Contract::slot_proposed_owner().key,
        ]
        .iter()
        .map(|key| env::storage_read(key))
        .collect()
    }

    #[test]
    fn serialized_v1_migration_preserves_history_and_owner_slots() {
        fixture();
        let owners = owner_slots();
        let old: V1 = env::state_read().unwrap();
        let versions_handle = near_sdk::borsh::to_vec(&old.versions).unwrap();
        let registry_handle = near_sdk::borsh::to_vec(&old.registry).unwrap();
        drop(old);
        let migrations = parse_one_or_many::<Migration>(br#"{"from_version":"v1"}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&migrations[0]).unwrap(),
            serde_json::json!({"from_version": "v1"})
        );
        run_migration_chain(migrations, V2::VERSION).unwrap();

        let migrated: V2 = env::state_read().unwrap();
        assert_eq!(read_state_version().unwrap(), 2);
        assert!(!V2::needs_migration().unwrap());
        assert_eq!(owner_slots(), owners);
        assert_eq!(
            near_sdk::borsh::to_vec(&migrated.versions).unwrap(),
            versions_handle
        );
        assert_eq!(
            near_sdk::borsh::to_vec(&migrated.registry).unwrap(),
            registry_handle
        );
        assert_eq!(migrated.versions.len(), 3);
        assert_eq!(migrated.registry.len(), 2);
        assert!(matches!(
            migrated.versions.get("stored"),
            Some(VersionEntry::Code { hash, code: Some(code) })
                if *hash == [1; 32] && code == &[0, 1, 2, 255]
        ));
        assert!(matches!(
            migrated.versions.get("removed"),
            Some(VersionEntry::Code { hash, code: None }) if *hash == [2; 32]
        ));
        assert!(matches!(
            migrated.versions.get("global"),
            Some(VersionEntry::GlobalHash(hash)) if *hash == [3; 32]
        ));
        assert!(matches!(
            migrated.registry.get(&account("reserved.near")),
            Some(RegistryEntry::Reserved)
        ));
        assert!(matches!(
            migrated.registry.get(&account("deployed.near")),
            Some(RegistryEntry::Deployed(deployment))
                if deployment == &Deployment {
                    version_key: "stored".to_owned(),
                    code_hash: [1; 32].into(),
                    block_height: U64(42),
                }
        ));
        assert!(migrated.uploads.is_empty());
        for role in [Role::Uploader, Role::Deployer] {
            assert_eq!(<Contract as Rbac>::iter_members_of(&role).count(), 0);
        }
    }

    #[test]
    fn invalid_migration_chains_do_not_change_serialized_state() {
        fixture();
        let state = env::storage_read(b"STATE").unwrap();
        let owners = owner_slots();
        assert!(matches!(
            run_migration_chain(vec![Migration::V1(V1ToV2), Migration::V1(V1ToV2)], 2),
            Err(MigrationChainError::LinkMismatch { .. })
        ));
        assert_eq!(env::storage_read(b"STATE").unwrap(), state);
        assert_eq!(read_state_version().unwrap(), 1);
        assert_eq!(owner_slots(), owners);

        write_state_version(0);
        assert!(matches!(
            run_migration_chain(vec![Migration::V1(V1ToV2)], 2),
            Err(MigrationChainError::StartMismatch { .. })
        ));
        assert_eq!(env::storage_read(b"STATE").unwrap(), state);
        assert_eq!(read_state_version().unwrap(), 0);
        assert_eq!(owner_slots(), owners);
        assert!(serde_json::from_slice::<Migration>(br#"{"from_version":"v2"}"#).is_err());
    }

    #[test]
    fn fresh_v2_starts_at_version_two_with_empty_collections() {
        testing_env!(VMContextBuilder::new().build());
        let state = V2::new(());
        assert_eq!(read_state_version().unwrap(), 2);
        assert_eq!(state.version(), 2);
        assert!(!V2::needs_migration().unwrap());
        assert!(state.versions.is_empty());
        assert!(state.registry.is_empty());
        assert!(state.uploads.is_empty());
    }
}
