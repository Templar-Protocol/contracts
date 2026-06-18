use near_sdk::{env, json_types::U128, near};
use templar_common::versioned_state::{Migrator, StateTransformer, StateVersion};

use crate::state;

#[near(serializers = [json])]
pub struct V0 {
    pub chain_id: U128,
}

impl StateTransformer for V0 {
    type Input = state::V0;
    type Output = state::V1;
    type Error = ();

    fn transform(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        Ok(state::V1::from_v0(input, self.chain_id.0))
    }
}

#[near(serializers = [json])]
pub struct V1;

impl StateTransformer for V1 {
    type Input = state::V1;
    type Output = state::V2;
    type Error = ();

    fn transform(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        Ok(state::V2::from_v1(input))
    }
}

#[near(serializers = [json])]
pub struct UnbrickV1;

impl StateTransformer for UnbrickV1 {
    type Input = state::V1;
    type Output = state::V2;
    type Error = ();

    fn input_version(&self) -> u32 {
        state::V0::VERSION
    }

    fn transform(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        Ok(state::V2::from_v1(input))
    }
}

#[near(serializers = [json])]
#[serde(tag = "from_version", rename_all = "snake_case")]
pub enum Migration {
    V0(V0),
    V1(V1),
    UnbrickV1(UnbrickV1),
}

impl From<V0> for Migration {
    fn from(value: V0) -> Self {
        Self::V0(value)
    }
}

impl From<V1> for Migration {
    fn from(value: V1) -> Self {
        Self::V1(value)
    }
}

impl From<UnbrickV1> for Migration {
    fn from(value: UnbrickV1) -> Self {
        Self::UnbrickV1(value)
    }
}

impl Migrator for Migration {
    fn run(self) {
        match self {
            Migration::V0(v0) => {
                v0.run()
                    .unwrap_or_else(|e| env::panic_str(&format!("Failed to migrate V0: {e}")));
            }
            Migration::V1(v1) => {
                v1.run()
                    .unwrap_or_else(|e| env::panic_str(&format!("Failed to migrate V1: {e}")));
            }
            Migration::UnbrickV1(unbrick_v1) => {
                unbrick_v1.run().unwrap_or_else(|e| {
                    env::panic_str(&format!("Failed to migrate UnbrickV1: {e}"))
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::{env, store::IterableMap, test_utils::VMContextBuilder, testing_env};
    use templar_common::versioned_state::{read_state_version, write_state_version};

    use crate::{
        authentication::{
            ed25519::raw,
            passkey::{self},
        },
        KeyId, KeyParameters,
    };

    use super::*;

    fn context() {
        testing_env!(VMContextBuilder::new().build());
    }

    #[test]
    fn v0_to_v1_to_v2() {
        context();

        let mut old = state::V0 {
            next_key_index: 42,
            keys: IterableMap::new(b"k"),
        };

        let passkey = p256::SecretKey::from_bytes(&[0x88_u8; 32].into()).unwrap();

        old.keys.insert(
            KeyId::Passkey(passkey::VerifyKey(passkey.public_key().into())),
            KeyParameters {
                block_height: 1111.into(),
                index: 2222.into(),
                nonce: 3333.into(),
            },
        );
        old.keys.insert(
            KeyId::Ed25519Raw(raw::VerifyKey([0xee_u8; 32].into())),
            KeyParameters {
                block_height: 4444.into(),
                index: 5555.into(),
                nonce: 6666.into(),
            },
        );

        env::state_write(&old);

        let migration = V0 {
            chain_id: 1234.into(),
        };

        let new = migration.run().unwrap();

        assert_eq!(read_state_version().unwrap(), 1);
        assert_eq!(new.chain_id, 1234);
        assert_eq!(new.next_key_index, 42);
        assert_eq!(new.keys.len(), 2);

        env::state_write(&state::V1 {
            next_key_index: new.next_key_index,
            keys: new.keys,
            chain_id: new.chain_id,
        });

        let new = V1.run().unwrap();

        assert_eq!(read_state_version().unwrap(), 2);
        assert_eq!(new.chain_id, 1234);
        assert_eq!(new.next_key_index, 42);
        assert_eq!(new.keys.len(), 2);
    }

    #[test]
    fn unbrick_v1_reads_broken_v1_state_from_version_zero() {
        context();

        let mut old = state::V1 {
            next_key_index: 42,
            keys: IterableMap::new(b"k"),
            chain_id: 1234,
        };

        old.keys.insert(
            KeyId::Ed25519Raw(raw::VerifyKey([0xee_u8; 32].into())),
            KeyParameters {
                block_height: 4444.into(),
                index: 5555.into(),
                nonce: 6666.into(),
            },
        );

        env::state_write(&old);
        write_state_version(0);

        let new = UnbrickV1.run().unwrap();

        assert_eq!(read_state_version().unwrap(), 2);
        assert_eq!(new.chain_id, 1234);
        assert_eq!(new.next_key_index, 42);
        assert_eq!(new.keys.len(), 1);
    }
}

#[cfg(kani)]
mod kani_proofs {
    use near_sdk::{json_types::U128, store::IterableMap};
    use templar_common::versioned_state::StateTransformer;

    use crate::KeyId;

    use super::*;

    #[kani::proof]
    fn v0_migration_preserves_scalar_safety_state() {
        let next_key_index = kani::any::<u64>();
        let chain_id = kani::any::<u128>();
        let old = state::V0 {
            next_key_index,
            keys: IterableMap::<KeyId, crate::KeyParameters>::new(b"k"),
        };

        let new = V0 {
            chain_id: U128(chain_id),
        }
        .transform(old)
        .unwrap();

        assert_eq!(new.next_key_index, next_key_index);
        assert_eq!(new.chain_id, chain_id);
    }

    #[kani::proof]
    fn v1_migration_preserves_scalar_safety_state() {
        let next_key_index = kani::any::<u64>();
        let chain_id = kani::any::<u128>();
        let old = state::V1 {
            next_key_index,
            keys: IterableMap::<KeyId, crate::KeyParameters>::new(b"k"),
            chain_id,
        };

        let new = V1.transform(old).unwrap();

        assert_eq!(new.next_key_index, next_key_index);
        assert_eq!(new.chain_id, chain_id);
    }

    #[kani::proof]
    fn unbrick_v1_migration_preserves_scalar_safety_state() {
        let next_key_index = kani::any::<u64>();
        let chain_id = kani::any::<u128>();
        let old = state::V1 {
            next_key_index,
            keys: IterableMap::<KeyId, crate::KeyParameters>::new(b"k"),
            chain_id,
        };

        let new = UnbrickV1.transform(old).unwrap();

        assert_eq!(new.next_key_index, next_key_index);
        assert_eq!(new.chain_id, chain_id);
    }
}
