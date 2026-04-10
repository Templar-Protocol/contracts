use std::io::{Error, ErrorKind};

use borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::env;

const VERSION_KEY: &[u8] = b"__v";

pub fn write_state_version(version: u32) {
    env::storage_write(VERSION_KEY, &version.to_le_bytes());
}

pub fn read_state_version() -> Result<u32, std::io::Error> {
    let Some(bytes) = env::storage_read(VERSION_KEY) else {
        return Ok(0);
    };

    borsh::from_slice(&bytes)
}

#[derive(Debug)]
#[near_sdk::near(serializers = [borsh])]
pub struct VersionedState<T: StateVersion>(T);

impl<T: StateVersion> VersionedState<T> {
    pub fn new(state: T) -> Self {
        write_state_version(T::VERSION);
        Self(state)
    }

    pub fn version(&self) -> u32 {
        T::VERSION
    }
}

impl<T: StateVersion> std::ops::Deref for VersionedState<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: StateVersion> std::ops::DerefMut for VersionedState<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub trait StateVersion {
    const VERSION: u32;
    type NewArgs;

    fn new(args: Self::NewArgs) -> VersionedState<Self>
    where
        Self: Sized;

    fn needs_migration() -> Result<bool, std::io::Error> {
        let stored = read_state_version()?;
        if stored > Self::VERSION {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Stored state version {stored} is newer than supported version {}",
                    Self::VERSION
                ),
            ));
        }

        Ok(stored < Self::VERSION)
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
        let stored = read_state_version()?;
        let expected = self.input_version();
        if stored != expected {
            return Err(MigrationError::StoredVersionMismatch { stored, expected });
        }
        let old_state =
            env::state_read::<Self::Input>().ok_or(MigrationError::FailedToDeserializeOldState)?;
        let new_state = self
            .transform(old_state)
            .map_err(MigrationError::Transformation)?;
        env::state_write(&new_state);
        write_state_version(self.output_version());
        Ok(new_state)
    }

    fn transform(&self, input: Self::Input) -> Result<Self::Output, Self::Error>;
}

#[derive(thiserror::Error, Debug)]
pub enum MigrationError<E> {
    #[error("Failed to deserialize stored state version: {0}")]
    StoredVersionDeserialization(#[from] std::io::Error),
    #[error("Stored state version {stored} != args `from_version` {expected}")]
    StoredVersionMismatch { stored: u32, expected: u32 },
    #[error("Failed to deserialize old state")]
    FailedToDeserializeOldState,
    #[error("Failed to transform old state")]
    Transformation(E),
}

pub trait Migrator {
    fn run(self);
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
        assert_eq!(read_state_version().unwrap(), 0);
    }

    #[test]
    fn malformed_stored_version_errors() {
        context();
        write_state_version(7);
        env::storage_write(VERSION_KEY, &[1, 2, 3]);

        assert!(read_state_version().is_err());
    }

    #[test]
    fn future_stored_version_errors() {
        context();
        write_state_version(9);

        let error = TestState::needs_migration().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("Stored state version 9 is newer"));
    }

    struct TestState;

    impl StateVersion for TestState {
        const VERSION: u32 = 2;
        type NewArgs = ();

        fn new((): Self::NewArgs) -> VersionedState<Self> {
            VersionedState::new(Self)
        }
    }
}
