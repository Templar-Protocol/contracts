use std::io::{Error, ErrorKind};

use borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::{env, serde::de::DeserializeOwned, serde_json};

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
    /// Input state version this migration expects to read (the underlying
    /// [`StateTransformer::input_version`]).
    fn input_version(&self) -> u32;
    /// Output state version this migration writes (the underlying
    /// [`StateTransformer::output_version`]).
    fn output_version(&self) -> u32;
    /// Apply the migration, advancing the stored state version to
    /// [`Migrator::output_version`]. Consumes `self`; run only after the chain is validated.
    fn run(self);
}

/// Deserialize `migrate_args` as either a single migration (legacy wire shape, a JSON object) or an
/// ordered list (a JSON array). A migration is JSON-tagged by `from_version`, so the first
/// non-whitespace byte disambiguates: `[` is a list, anything else a single object wrapped into a
/// one-element chain. Keeps the pre-list wire format working unchanged.
pub fn parse_one_or_many<T: DeserializeOwned>(bytes: &[u8]) -> Result<Vec<T>, serde_json::Error> {
    let is_array = bytes
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b == b'[');

    if is_array {
        serde_json::from_slice::<Vec<T>>(bytes)
    } else {
        serde_json::from_slice::<T>(bytes).map(|migration| vec![migration])
    }
}

#[derive(thiserror::Error, Debug)]
pub enum MigrationChainError {
    #[error("Failed to read stored state version: {0}")]
    StoredVersion(#[from] std::io::Error),
    #[error("state migration is required but no migrations were provided")]
    Empty,
    #[error("first migration input version {input} != stored state version {stored}")]
    StartMismatch { input: u32, stored: u32 },
    #[error("migration output version {output} != next migration input version {input}")]
    LinkMismatch { output: u32, input: u32 },
    #[error("final migration output version {output} != target state version {target}")]
    EndMismatch { output: u32, target: u32 },
}

/// Validate an ordered migration chain end-to-end, then run each step in sequence.
///
/// Every invariant is checked *before* any transform runs, so an invalid chain reverts without
/// writing state:
/// 1. the list is non-empty,
/// 2. the first migration's input version equals the stored state version,
/// 3. each migration's output version equals the next migration's input version, and
/// 4. the last migration's output version equals `target`.
///
/// Only then are the migrations run in order; each [`Migrator::run`] independently re-asserts
/// `stored == input_version()` and bumps the stored version to its output.
pub fn run_migration_chain<M: Migrator>(
    migrations: Vec<M>,
    target: u32,
) -> Result<(), MigrationChainError> {
    let Some((first, rest)) = migrations.split_first() else {
        return Err(MigrationChainError::Empty);
    };

    // Validate the whole chain before running any step: the first migration must start at the stored
    // version, each later migration's input must equal the previous step's output, and the final
    // output must land on target. An invalid chain thus reverts without writing state.
    let stored = read_state_version()?;
    if first.input_version() != stored {
        return Err(MigrationChainError::StartMismatch {
            input: first.input_version(),
            stored,
        });
    }

    let mut expected_input = first.output_version();
    for migration in rest {
        let input = migration.input_version();
        if input != expected_input {
            return Err(MigrationChainError::LinkMismatch {
                output: expected_input,
                input,
            });
        }
        expected_input = migration.output_version();
    }

    // `expected_input` now holds the final migration's output version.
    if expected_input != target {
        return Err(MigrationChainError::EndMismatch {
            output: expected_input,
            target,
        });
    }

    for migration in migrations {
        migration.run();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use near_sdk::{test_utils::VMContextBuilder, testing_env};
    use rstest::rstest;

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

    // A `Migrator` standing in for a real transformer: it advances the stored version from `input`
    // to `output`, re-asserting `stored == input` the way `StateTransformer::run` does.
    struct MockMigration {
        input: u32,
        output: u32,
    }

    impl Migrator for MockMigration {
        fn input_version(&self) -> u32 {
            self.input
        }

        fn output_version(&self) -> u32 {
            self.output
        }

        fn run(self) {
            assert_eq!(
                read_state_version().unwrap(),
                self.input,
                "mock migration ran against the wrong stored version",
            );
            write_state_version(self.output);
        }
    }

    fn m(input: u32, output: u32) -> MockMigration {
        MockMigration { input, output }
    }

    #[derive(Debug, PartialEq)]
    #[near_sdk::near(serializers = [json])]
    #[serde(tag = "from_version", rename_all = "snake_case")]
    enum TestMigration {
        V0,
        V1,
    }

    #[test]
    fn parse_single_object_is_one_element_chain() {
        let parsed: Vec<TestMigration> = parse_one_or_many(br#"{"from_version":"v0"}"#).unwrap();
        assert_eq!(parsed, vec![TestMigration::V0]);
    }

    #[test]
    fn parse_array_is_multi_element_chain() {
        let parsed: Vec<TestMigration> =
            parse_one_or_many(br#"[{"from_version":"v0"},{"from_version":"v1"}]"#).unwrap();
        assert_eq!(parsed, vec![TestMigration::V0, TestMigration::V1]);
    }

    #[test]
    fn parse_tolerates_leading_whitespace_before_array() {
        let parsed: Vec<TestMigration> =
            parse_one_or_many(b"  \n [{\"from_version\":\"v0\"}]").unwrap();
        assert_eq!(parsed, vec![TestMigration::V0]);
    }

    #[test]
    fn chain_runs_multiple_steps_to_target() {
        context();
        write_state_version(0);

        run_migration_chain(vec![m(0, 1), m(1, 2)], 2).unwrap();

        assert_eq!(read_state_version().unwrap(), 2);
    }

    #[test]
    fn chain_runs_single_step_to_target() {
        context();
        write_state_version(1);

        run_migration_chain(vec![m(1, 2)], 2).unwrap();

        assert_eq!(read_state_version().unwrap(), 2);
    }

    // Every invalid chain is rejected before any transform runs, so the stored version is left
    // untouched. `matches` pins the exact error variant (and its versions) per case.
    #[rstest]
    #[case::empty(Vec::new(), 2, |e: &_| matches!(e, MigrationChainError::Empty))]
    #[case::wrong_start(vec![m(1, 2)], 2, |e: &_| matches!(e, MigrationChainError::StartMismatch { input: 1, stored: 0 }))]
    #[case::broken_link(vec![m(0, 1), m(2, 3)], 3, |e: &_| matches!(e, MigrationChainError::LinkMismatch { output: 1, input: 2 }))]
    #[case::not_landing_on_target(vec![m(0, 1)], 2, |e: &_| matches!(e, MigrationChainError::EndMismatch { output: 1, target: 2 }))]
    fn chain_rejects_invalid(
        #[case] migrations: Vec<MockMigration>,
        #[case] target: u32,
        #[case] matches: fn(&MigrationChainError) -> bool,
    ) {
        context();
        write_state_version(0);

        let err = run_migration_chain(migrations, target).unwrap_err();
        assert!(matches(&err), "unexpected error variant: {err:?}");
        assert_eq!(
            read_state_version().unwrap(),
            0,
            "no state written on invalid chain"
        );
    }
}
