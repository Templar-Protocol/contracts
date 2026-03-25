use borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::{env, ext_contract, near};

const VERSION_KEY: &[u8] = b"__v";

pub fn stored_version() -> u32 {
    env::storage_read(VERSION_KEY)
        .filter(|bytes| bytes.len() == 4)
        .map_or(0, |bytes| {
            let mut buf = [0u8; 4];
            buf.copy_from_slice(&bytes);
            u32::from_le_bytes(buf)
        })
}

pub trait StateVersion {
    const VERSION: u32;

    fn needs_migration() -> bool {
        stored_version() < Self::VERSION
    }
}

pub trait StateTransformer {
    type Input: StateVersion + BorshDeserialize;
    type Output: StateVersion + BorshSerialize;
    type Error;

    fn input_version(&self) -> u32 {
        Self::Input::VERSION
    }

    fn output_version(&self) -> u32 {
        Self::Output::VERSION
    }

    fn run(&self) -> Result<Self::Output, MigrationError<Self::Error>> {
        let stored = stored_version();
        let expected = self.input_version();
        if stored != expected {
            return Err(MigrationError::StoredVersionMismatch { stored, expected });
        }
        let old_state =
            env::state_read::<Self::Input>().ok_or(MigrationError::FailedToDeserializeOldState)?;
        let new_state = self.transform(old_state)?;
        env::state_write(&new_state);
        env::storage_write(VERSION_KEY, &self.output_version().to_le_bytes());
        Ok(new_state)
    }

    fn transform(&self, input: Self::Input) -> Result<Self::Output, Self::Error>;
}

#[derive(thiserror::Error, Debug)]
pub enum MigrationError<E> {
    #[error("Stored state version {stored} != args `from_version` {expected}")]
    StoredVersionMismatch { stored: u32, expected: u32 },
    #[error("Failed to deserialize old state")]
    FailedToDeserializeOldState,
    #[error(transparent)]
    VersionAlreadyInitialized(VersionAlreadyInitializedError),
    #[error("Failed to transform old state")]
    TransformationError(#[from] E),
}

#[derive(thiserror::Error, Debug)]
#[error("State version already initialized")]
pub struct VersionAlreadyInitializedError;

#[near(serializers = [json])]
pub struct MigrationArgs<T> {
    pub args: T,
}

pub trait Migrator {
    fn run(self);
}

#[ext_contract]
pub trait MigrateExternalInterface {
    fn migrate_stored_state_version() -> u32;
    fn migrate_target_state_version() -> u32;
    fn migrate_needs_migration() -> bool;
}

#[macro_export]
macro_rules! impl_migration {
    ($current_state: ty, $migrations: ty) => {
        #[::near_sdk::near]
        impl $crate::contract_state::MigrateExternalInterface for Contract {
            fn migrate_stored_state_version() -> u32 {
                $crate::contract_state::stored_version()
            }

            fn migrate_target_state_version() -> u32 {
                <$current_state as $crate::contract_state::StateVersion>::VERSION
            }

            fn migrate_needs_migration() -> bool {
                <$current_state as $crate::contract_state::StateVersion>::needs_migration()
            }
        }

        #[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
        #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
        pub fn migrate() {
            use ::near_sdk::env;

            env::setup_panic_hook();

            let current_id = env::current_account_id();
            assert!(
                env::predecessor_account_id() == current_id,
                "migrate function is private",
            );

            let input = env::input().unwrap_or_else(|| env::panic_str("no input"));

            let args: $crate::contract_state::MigrationArgs<$migrations> =
                ::near_sdk::serde_json::from_slice(&input)
                    .unwrap_or_else(|e| env::panic_str(&e.to_string()));

            $crate::contract_state::Migrator::run(args.args);
        }
    };
}

#[cfg(test)]
mod tests {
    use near_sdk::{test_utils::VMContextBuilder, testing_env};

    use super::*;

    fn context() {
        testing_env!(VMContextBuilder::new().build());
    }

    #[test]
    fn stored_version_defaults_to_zero() {
        context();
        assert_eq!(stored_version(), 0);
    }
}
