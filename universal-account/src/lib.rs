use alloy::{primitives::U256, sol_types::Eip712Domain};
use near_sdk::{
    json_types::{Base58CryptoHash, U64},
    near, CryptoHash,
};

pub const NEAR_MAINNET_CHAIN_ID: u128 = 397;
pub const NEAR_TESTNET_CHAIN_ID: u128 = 398;

pub mod authentication;
pub mod contract_state;
pub mod encoding;
mod event;
pub use event::Event;
mod execute_args;
pub use execute_args::*;
pub mod init_args;
pub use init_args::InitArgs;
pub mod key_id;
pub use key_id::KeyId;
pub mod transaction;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct PayloadExecutionParameters {
    /// Static. If a universal account is deleted and recreated with the same
    /// keys, this ensures that old signatures are not replayable.
    pub block_height: U64,
    /// Static. If a key is deleted and re-added to the same account, this
    /// ensures that that old signatures are not replayable.
    pub index: U64,
    /// Increments for each message executed by this key.
    pub nonce: U64,
    pub name: Option<String>,
    pub version: Option<String>,
    pub chain_id: Option<near_sdk::json_types::U128>,
    pub verifying_contract: near_sdk::AccountId,
    pub salt: Option<Base58CryptoHash>,
}

impl From<PayloadExecutionParameters> for Eip712Domain {
    fn from(value: PayloadExecutionParameters) -> Self {
        Self {
            name: value.name.map(Into::into),
            version: value.version.map(Into::into),
            chain_id: value.chain_id.map(|i| U256::from(i.0)),
            verifying_contract: Some(
                #[allow(
                    clippy::unwrap_used,
                    reason = "hash len 32 >= 20 && slice len == array len"
                )]
                <[u8; 20]>::try_from(
                    &near_sdk::env::keccak256_array(value.verifying_contract.as_bytes())[0..20],
                )
                .unwrap()
                .into(),
            ),
            salt: value.salt.map(|c| CryptoHash::from(c).into()),
        }
    }
}

impl PayloadExecutionParameters {
    pub fn new_auto(
        verifying_contract: near_sdk::AccountId,
        key_parameters: KeyParameters,
        chain_id: u128,
    ) -> Self {
        Self::new_empty(verifying_contract)
            .with_key_parameters(key_parameters)
            .chain_id(chain_id)
            .auto()
    }

    pub fn new_empty(verifying_contract: near_sdk::AccountId) -> Self {
        Self {
            block_height: U64(0),
            index: U64(0),
            nonce: U64(0),
            name: None,
            version: None,
            chain_id: None,
            salt: None,
            verifying_contract,
        }
    }

    #[must_use]
    pub fn next_nonce(self) -> Self {
        Self {
            nonce: U64(self.nonce.0 + 1),
            ..self
        }
    }

    #[must_use]
    pub fn with_key_parameters(self, key_parameters: KeyParameters) -> Self {
        Self {
            block_height: key_parameters.block_height,
            index: key_parameters.index,
            nonce: key_parameters.nonce,
            ..self
        }
    }

    #[must_use]
    pub fn auto(self) -> Self {
        self.auto_name().auto_version().auto_salt()
    }

    #[must_use]
    pub fn auto_salt(self) -> Self {
        let salt = Base58CryptoHash::from(near_sdk::env::keccak256_array(
            #[allow(clippy::unwrap_used, reason = "Infallible")]
            &near_sdk::borsh::to_vec(&(self.block_height, self.index)).unwrap(),
        ));
        Self {
            salt: Some(salt),
            ..self
        }
    }

    #[must_use]
    pub fn auto_name(self) -> Self {
        Self {
            name: Some("Templar Universal Account".to_string()),
            ..self
        }
    }

    #[must_use]
    pub fn auto_version(self) -> Self {
        Self {
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            ..self
        }
    }

    #[must_use]
    pub fn name(self, name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..self
        }
    }

    #[must_use]
    pub fn version(self, version: impl Into<String>) -> Self {
        Self {
            version: Some(version.into()),
            ..self
        }
    }

    #[must_use]
    pub fn chain_id(self, chain_id: u128) -> Self {
        Self {
            chain_id: Some(near_sdk::json_types::U128(chain_id)),
            ..self
        }
    }

    #[must_use]
    pub fn salt(self, salt: impl Into<Base58CryptoHash>) -> Self {
        Self {
            salt: Some(salt.into()),
            ..self
        }
    }

    #[must_use]
    pub fn nonce(self, nonce: impl Into<U64>) -> Self {
        Self {
            nonce: nonce.into(),
            ..self
        }
    }

    #[must_use]
    pub fn index(self, index: impl Into<U64>) -> Self {
        Self {
            index: index.into(),
            ..self
        }
    }

    #[must_use]
    pub fn block_height(self, block_height: impl Into<U64>) -> Self {
        Self {
            block_height: block_height.into(),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
#[serde(deny_unknown_fields)]
pub struct KeyParameters {
    /// Static. If a universal account is deleted and recreated with the same
    /// keys, this ensures that old signatures are not replayable.
    pub block_height: U64,
    /// Static. If a key is deleted and re-added to the same account, this
    /// ensures that that old signatures are not replayable.
    pub index: U64,
    /// Increments for each message executed by this key.
    pub nonce: U64,
}

impl KeyParameters {
    #[must_use]
    pub fn next(&self) -> Self {
        Self {
            nonce: U64(self.nonce.0 + 1),
            ..*self
        }
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::bs58;
    use p256::elliptic_curve::rand_core::OsRng;
    use solana_sdk::{signature::Keypair, signer::Signer};

    use crate::authentication::{ed25519_raw, passkey::Passkey};

    use super::*;

    #[test]
    fn keyid_serialization() {
        let sk_p256 = p256::SecretKey::random(&mut OsRng);
        let passkey = Passkey(sk_p256.public_key().into());
        let passkey_id: KeyId = passkey.into();
        let passkey_id_str = passkey_id.to_string();

        let Some(b) = passkey_id_str.strip_prefix("p256:") else {
            panic!("invalid prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 65);

        let sk_ed25519 = Keypair::new();
        let ed25519_raw = ed25519_raw::VerifyKey(sk_ed25519.pubkey().to_bytes().into());
        let ed25519_raw_id: KeyId = ed25519_raw.into();
        let ed25519_raw_id_str = ed25519_raw_id.to_string();

        let Some(b) = ed25519_raw_id_str.strip_prefix("ed25519:") else {
            panic!("invalid prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 32);
    }
}
