use near_sdk::{env, json_types::U128, near, store::IterableMap};

use crate::{KeyId, KeyParameters};

pub const STATE_VERSION: u32 = 1;

#[near(serializers = [borsh])]
pub struct StateV0 {
    pub next_key_index: u64,
    pub keys: IterableMap<KeyId, KeyParameters>,
}

#[near(serializers = [borsh])]
pub struct StateV1 {
    pub next_key_index: u64,
    pub keys: IterableMap<KeyId, KeyParameters>,
    pub chain_id: u128,
}

impl StateV1 {
    fn from_v0(old: StateV0, chain_id: u128) -> Self {
        Self {
            next_key_index: old.next_key_index,
            keys: old.keys,
            chain_id,
        }
    }
}

#[near(serializers = [json])]
#[serde(tag = "from_version", rename_all = "snake_case")]
pub enum Migration {
    V0 { chain_id: U128 },
}

impl Migration {
    pub fn input_version(&self) -> u32 {
        match self {
            Migration::V0 { .. } => 0,
        }
    }

    pub fn output_version(&self) -> u32 {
        match self {
            Migration::V0 { .. } => 1,
        }
    }

    /// Migrates the contract state by one state version.
    ///
    /// # Errors
    ///
    /// - If deserializing the previous state fails.
    pub fn migrate(self) -> Result<StateV1, FailedToDeserializeOldState> {
        match self {
            Migration::V0 { chain_id } => {
                let old = env::state_read::<StateV0>().ok_or(FailedToDeserializeOldState)?;

                Ok(StateV1::from_v0(old, chain_id.0))
            }
        }
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Failed to deserialize old state")]
pub struct FailedToDeserializeOldState;

#[cfg(test)]
mod tests {
    use near_sdk::{test_utils::VMContextBuilder, testing_env};

    use crate::authentication::{ed25519::raw, passkey::Passkey};

    use super::*;

    #[test]
    fn v0_to_v1() {
        let ctx = VMContextBuilder::new().build();
        testing_env!(ctx.clone());

        let mut old = StateV0 {
            next_key_index: 42,
            keys: IterableMap::new(b"k"),
        };

        let passkey = p256::SecretKey::from_bytes(&[0x88_u8; 32].into()).unwrap();

        old.keys.insert(
            KeyId::Passkey(Passkey(passkey.public_key().into())),
            KeyParameters {
                block_height: 1111.into(),
                index: 2222.into(),
                nonce: 3333.into(),
            },
        );
        old.keys.insert(
            KeyId::Ed25519RawKey(raw::VerifyKey([0xee_u8; 32].into())),
            KeyParameters {
                block_height: 4444.into(),
                index: 5555.into(),
                nonce: 6666.into(),
            },
        );

        near_sdk::env::state_write(&old);

        let migration = Migration::V0 {
            chain_id: 1234.into(),
        };

        assert_eq!(migration.input_version(), 0);
        assert_eq!(migration.output_version(), 1);

        let new = migration.migrate().unwrap();

        assert_eq!(new.chain_id, 1234);
        assert_eq!(new.next_key_index, 42);
        assert_eq!(new.keys.len(), 2);
    }
}
