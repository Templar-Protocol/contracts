use alloy::{primitives::U256, sol_types::Eip712Domain};
use authentication::{
    ed25519_raw, eip712,
    passkey::{self, Passkey},
    CheckSignatureError, ExecutionContextProvider, ExecutionError, Key, MessageWithSignature,
};
use near_sdk::{
    json_types::{Base58CryptoHash, U64},
    near,
    serde::{self, de::DeserializeOwned},
    CryptoHash,
};

pub const NEAR_MAINNET_CHAIN_ID: u128 = 397;
pub const NEAR_TESTNET_CHAIN_ID: u128 = 398;

pub mod authentication;
pub mod encoding;
mod event;
pub use event::Event;
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

#[derive(Debug, Clone)]
#[near(serializers = [json])]
#[serde(bound = "T: DeserializeOwned")]
pub enum ExecuteArgs<T: serde::Serialize> {
    Passkey {
        key: Passkey,
        message: Box<MessageWithSignature<passkey::Message<T>>>,
    },
    Ed25519Raw {
        key: ed25519_raw::VerifyKey,
        message: Box<MessageWithSignature<ed25519_raw::Message<T>>>,
    },
    Eip712 {
        key: eip712::VerifyKey,
        message: Box<MessageWithSignature<eip712::Message<T>>>,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq, PartialOrd, Ord)]
pub enum VerificationError {
    #[error(transparent)]
    Signature(#[from] CheckSignatureError),
    #[error(transparent)]
    Execution(#[from] ExecutionError),
}

impl<T: serde::Serialize> ExecuteArgs<T> {
    pub fn key_id(&self) -> KeyId {
        match self {
            Self::Passkey { key, .. } => KeyId::Passkey(key.clone()),
            Self::Ed25519Raw { key, .. } => KeyId::Ed25519RawKey(key.clone()),
            Self::Eip712 { key, .. } => KeyId::Eip712(key.clone()),
        }
    }

    pub fn message_unchecked(&self) -> &T {
        match self {
            Self::Passkey { message, .. } => message.message.0.parsed.payload_ref(),
            Self::Ed25519Raw { message, .. } => message.message.0.parsed.payload_ref(),
            Self::Eip712 { message, .. } => message.message.0.parsed.payload_ref(),
        }
    }

    /// # Errors
    ///
    /// - If signature verification fails
    /// - If execution parameters do not match
    pub fn verify(
        self,
        parameters: &PayloadExecutionParameters,
        allowed_origin: impl FnOnce(Option<&str>) -> bool,
    ) -> Result<T, VerificationError> {
        Ok(match self {
            ExecuteArgs::Passkey { key, message } => key
                .verify_signature(*message)?
                .verify_execution(parameters, allowed_origin)?,
            ExecuteArgs::Ed25519Raw { key, message } => key
                .verify_signature(*message)?
                .verify_execution(parameters, allowed_origin)?,
            ExecuteArgs::Eip712 { key, message } => key
                .verify_signature(*message)?
                .verify_execution(parameters, allowed_origin)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use alloy::signers::local::PrivateKeySigner;
    use authentication::{
        ed25519_raw,
        passkey::{
            self,
            data::{AuthenticatorData, ClientDataJson},
        },
        HashForSigning, Payload,
    };
    use near_sdk::{bs58, AccountId, NearToken};
    use p256::elliptic_curve::rand_core::OsRng;
    use rstest::rstest;
    use solana_sdk::{signature::Keypair, signer::Signer};
    use transaction::Transaction;

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

    fn payload() -> Payload<Box<[Transaction]>> {
        let payload = vec![Transaction {
            receiver_id: "token.near".parse().unwrap(),
            actions: vec![transaction::FunctionCallAction::new(
                "ft_transfer",
                br#"{"receiver_id":"receiver.near","amount":"100"}"#,
                NearToken::from_yoctonear(1),
                near_sdk::Gas::from_tgas(30),
            )
            .into()]
            .into_boxed_slice(),
        }]
        .into_boxed_slice();

        Payload::new(
            PayloadExecutionParameters::new_auto(
                "my-universal-account.near".parse().unwrap(),
                KeyParameters {
                    block_height: U64(12345),
                    index: U64(1),
                    nonce: U64(44),
                },
                NEAR_TESTNET_CHAIN_ID,
            ),
            payload,
        )
    }

    fn eip712_execute_args() -> ExecuteArgs<Box<[Transaction]>> {
        let sk = PrivateKeySigner::random();

        let message = eip712::Message::from_parsed(payload());

        let signed_message = message.sign(&sk);

        ExecuteArgs::Eip712 {
            key: eip712::VerifyKey(sk.address().into()),
            message: Box::new(signed_message),
        }
    }

    fn ed25519_raw_execute_args() -> ExecuteArgs<Box<[Transaction]>> {
        let sk = Keypair::new();

        let message = ed25519_raw::Message::from_parsed(payload());
        let preimage = message.preimage_for_signing();
        let signed_message = message.with_signature(sk.sign_message(&preimage).into());

        ExecuteArgs::Ed25519Raw {
            key: ed25519_raw::VerifyKey(sk.pubkey().to_bytes().into()),
            message: Box::new(signed_message),
        }
    }

    fn passkey_execute_args() -> ExecuteArgs<Box<[Transaction]>> {
        let sk = p256::SecretKey::random(&mut OsRng);

        let message = passkey::Message::from_parsed(payload());
        let hash = message.hash_for_signing();
        let signed_message: MessageWithSignature<_> = message.sign(
            &sk,
            AuthenticatorData(vec![1u8; 32].into_boxed_slice()),
            ClientDataJson {
                r#type: "type".to_string(),
                challenge: hash.into(),
                origin: "origin".to_string(),
                cross_origin: None,
                top_origin: None,
            },
        );

        ExecuteArgs::Passkey {
            key: Passkey(sk.public_key().into()),
            message: Box::new(signed_message),
        }
    }

    #[rstest]
    #[case("my-universal-account.near", 12345, 1, 44)]
    #[should_panic = r#"Execution(Mismatch { field: "verifying_contract", expected: "my-universal-account.nearx", actual: "my-universal-account.near" })"#]
    #[case("my-universal-account.nearx", 12345, 1, 44)]
    #[should_panic = r#"Execution(Mismatch { field: "block_height", expected: "12346", actual: "12345" })"#]
    #[case("my-universal-account.near", 12346, 1, 44)]
    #[should_panic = r#"Execution(Mismatch { field: "index", expected: "0", actual: "1" })"#]
    #[case("my-universal-account.near", 12345, 0, 44)]
    #[should_panic = r#"Execution(Mismatch { field: "nonce", expected: "45", actual: "44" })"#]
    #[case("my-universal-account.near", 12345, 1, 45)]
    #[test]
    fn verify(
        #[values(
            passkey_execute_args(),
            ed25519_raw_execute_args(),
            eip712_execute_args()
        )]
        exec_args: ExecuteArgs<Box<[Transaction]>>,
        #[case] executor_account_id: AccountId,
        #[case] block_height: u64,
        #[case] index: u64,
        #[case] nonce: u64,
    ) {
        exec_args
            .verify(
                &PayloadExecutionParameters::new_auto(
                    executor_account_id,
                    KeyParameters {
                        block_height: U64(block_height),
                        index: U64(index),
                        nonce: U64(nonce),
                    },
                    NEAR_TESTNET_CHAIN_ID,
                ),
                |_| true,
            )
            .unwrap();
    }

    #[rstest]
    #[case("origin")]
    #[should_panic = "Execution(OriginUnknown)"]
    #[case("origin2")]
    #[test]
    fn verify_origin(#[case] allowed_origin: &str) {
        passkey_execute_args()
            .verify(
                &PayloadExecutionParameters::new_auto(
                    AccountId::from_str("my-universal-account.near").unwrap(),
                    KeyParameters {
                        block_height: U64(12345),
                        index: U64(1),
                        nonce: U64(44),
                    },
                    NEAR_TESTNET_CHAIN_ID,
                ),
                |o| o == Some(allowed_origin),
            )
            .unwrap();
    }
}
